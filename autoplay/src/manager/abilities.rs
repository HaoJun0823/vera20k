//! Abilities: what a unit does with, rather than to, a structure.
//!
//! One file per manager; see `manager/mod.rs`. This one owns the jobs that need
//! a click on a building instead of a click on a cell: capture, garrison,
//! infiltration and transport. The engine already owns every mechanism —
//! `sim::mission::capture` walks the engineer and `sim::movement::per_cell` hands
//! the arrival to the building receiver (`0x00519948`) — so what is missing here
//! is only the decision of *when* and *which*.
//!
//! Nothing in this file names a type. A capturer is any unit whose profile
//! carries [`Roles::ENGINEER`] or [`Roles::AGENT`], and a building worth taking is
//! one the house does not already own.

use super::{Ctx, Demands, Manager};
use crate::job::{Proposal, Target};
use crate::profile::Roles;
use crate::registry::JobKind;

/// How far a capturer will walk for a structure, in cells.
///
/// The wish list asks for capture "with the least loss", which on this map means
/// the engineer reaches the building it is sent to: a walk across the whole map
/// is a walk through whatever the enemy has between. Measured earlier in the
/// project, a march covers about thirty cells before it stops, so a capture
/// farther than a march is one the engineer never arrives at.
const CAPTURE_REACH_CELLS: i32 = 40;

/// Capture, in the order a house should want it.
///
/// The wish list calls taking an enemy construction yard the most important thing
/// in the whole feature set: a yard is a production line the house did not have,
/// and `[GATECH]`-style prerequisites are satisfied by holding the side's yard
/// (`holds_a_captured_yard_of` in `can_build`). A war factory and a refinery are
/// next because both pay immediately; a power plant is worth taking for the
/// headroom, and a defence only when it is close enough to walk to.
///
/// The weights are the ordering, not a score: a yard is always worth more than a
/// refinery at the same distance, and distance only breaks ties inside one rank.
fn capture_rank(roles: Roles) -> i64 {
    if roles.contains(Roles::BUILDS_VEHICLES) {
        80
    } else if roles.contains(Roles::REFINERY) {
        70
    } else if roles.contains(Roles::TRAINS_INFANTRY) {
        60
    } else if roles.contains(Roles::SUPERWEAPON) {
        55
    } else if roles.contains(Roles::LABORATORY) {
        50
    } else if roles.contains(Roles::ARMED) {
        40
    } else {
        30
    }
}

#[derive(Debug, Default)]
pub struct Abilities {
    /// The building each capturer has been sent to.
    ///
    /// Remembered, not re-chosen every term: a target recomputed each term is an
    /// engineer that walks one step, sees the building is still there, and is
    /// pointed at whatever the scan likes next. The same failure the scouts had
    /// (status.md, six attempts) and the reason `keep_captures` exists.
    capturing: std::collections::BTreeMap<u64, u64>,
}

impl Manager for Abilities {
    fn propose(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        self.capture(ctx, out);
    }
}

impl Abilities {
    /// Send an engineer at a structure worth taking.
    ///
    /// The wish list's centrepiece: taking an enemy construction yard gives the
    /// house that side's production line. A house with no capturer does nothing
    /// here and loses nothing by it.
    fn capture(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        // Capturers the house still has. A taken engineer is consumed by the
        // building receiver, so a unit that is gone drops out of this set and its
        // remembered target is forgotten with it.
        let capturers = self.capturers(ctx);
        let targets = self.capture_targets(ctx);
        if capturers.is_empty() || targets.is_empty() {
            self.capturing.clear();
            return;
        }
        let alive: std::collections::BTreeSet<u64> =
            capturers.iter().map(|unit| unit.entity_id).collect();
        let target_ids: std::collections::BTreeSet<u64> =
            targets.iter().map(|target| target.entity_id).collect();
        self.capturing
            .retain(|unit, target| alive.contains(unit) && target_ids.contains(target));

        // One capturer per building: two engineers sent to the same yard are the
        // second one standing at a building that is already the house's.
        let taken: std::collections::BTreeSet<u64> = self.capturing.values().copied().collect();
        let free: Vec<_> = capturers
            .iter()
            .filter(|unit| !self.capturing.contains_key(&unit.entity_id))
            .copied()
            .collect();
        let unclaimed: Vec<_> = targets
            .iter()
            .filter(|target| !taken.contains(&target.entity_id))
            .copied()
            .collect();

        for (unit, target) in free.iter().zip(unclaimed.iter()) {
            // The building has to be worth the walk: an engineer sent forty cells
            // through the enemy's ground arrives dead or not at all, and the keep
            // rule is that a unit the house owns is not thrown away for nothing.
            if cell_distance(unit.cell, target.cell) > CAPTURE_REACH_CELLS {
                continue;
            }
            self.capturing.insert(unit.entity_id, target.entity_id);
            out.jobs.push(Proposal {
                kind: JobKind::Capture,
                target: Target::Object(target.entity_id),
                priority: capture_rank(self.roles_of(ctx, target.type_id)),
                actor: Some(unit.entity_id),
            });
        }

        // A capturer already on its way keeps the building it was given.
        for (unit, target) in &self.capturing {
            if free.iter().any(|free| free.entity_id == *unit) {
                continue;
            }
            out.jobs.push(Proposal {
                kind: JobKind::Capture,
                target: Target::Object(*target),
                priority: 60,
                actor: Some(*unit),
            });
        }
    }

    /// The units that can take a building: an engineer or an agent.
    fn capturers(&self, ctx: &Ctx<'_>) -> Vec<vera20k_core::sim::control::observation::OwnEntity> {
        let mut capturers = Vec::new();
        for unit in ctx.obs.own_entities() {
            let Some(profile) = ctx
                .obs
                .object_type(ctx.rules, unit.type_id)
                .and_then(|ty| ctx.profiles.get(&ty.id))
            else {
                continue;
            };
            if !(profile.roles.contains(Roles::ENGINEER) || profile.roles.contains(Roles::AGENT)) {
                continue;
            }
            if !unit.orderable || ctx.obs.inside_transport(unit.entity_id).is_some() {
                continue;
            }
            capturers.push(unit);
        }
        capturers.sort_by_key(|unit| unit.entity_id);
        capturers
    }

    /// The buildings worth taking: the enemy's and the map's neutrals.
    ///
    /// Both come from the projection rather than from the rules, because a policy
    /// that aimed at a building the player could not see would be using
    /// information outside L0/L2, which spec §2.5 forbids. `neutral_structures`
    /// is L0 by definition — the wish list says every neutral building is visible
    /// from the start — and `visible_enemy_entities` is L2.
    fn capture_targets(
        &self,
        ctx: &Ctx<'_>,
    ) -> Vec<vera20k_core::sim::control::observation::EnemyEntity> {
        let mut targets: Vec<_> = ctx
            .obs
            .visible_enemy_entities()
            .chain(ctx.obs.neutral_structures())
            .collect();
        targets.sort_by_key(|target| target.entity_id);
        targets.dedup_by_key(|target| target.entity_id);
        targets
    }

    /// The roles a type carries, from the policy's own profile table.
    fn roles_of(&self, ctx: &Ctx<'_>, type_id: vera20k_core::sim::intern::InternedId) -> Roles {
        ctx.obs
            .object_type(ctx.rules, type_id)
            .and_then(|ty| ctx.profiles.get(&ty.id))
            .map_or(Roles::default(), |profile| profile.roles)
    }
}

/// Chebyshev distance in cells: how far a walk is on this grid.
fn cell_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    i32::from(a.0.abs_diff(b.0)).max(i32::from(a.1.abs_diff(b.1)))
}
