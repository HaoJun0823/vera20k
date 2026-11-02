//! Military: what to build, and where the army goes.
//!
//! Two decisions, both from the profile: queue the cheapest armed unit the
//! house can build (infantry from a barracks, vehicles from a war factory), and
//! once enough of them stand, send them at the nearest enemy object. Below that
//! size they hold the base. A harvester is armed and is not a soldier, so it is
//! never given either job.

use vera20k_core::rules::object_type::ObjectCategory;
use vera20k_core::sim::control::intent::Intent;
use vera20k_core::sim::control::observation::EnemyEntity;
use vera20k_core::sim::production::CanBuild;

use crate::job::{Proposal, Target};
use crate::manager::{Ctx, Demands, Manager};
use crate::profile::{Roles, UnitProfile};
use crate::registry::JobKind;

/// How many armed units the house wants before it attacks.
const ARMY_TO_ATTACK: usize = 30;

/// Whether the army attacks at all.
///
/// Off, measured three times now. On the un-fixed economy the army sat at
/// two-to-one and attacking lost three matches of six where holding lost one.
/// On the fixed build, with the army at parity and the order a cell attack-move,
/// attacking was worse in all six cells with the stuck count at 13-33: units
/// walked to one stale destination and stood on it. With the order now an attack
/// on the object a player would click, and the target read off what the house
/// actually sees, attacking no longer costs objects — own falls in no cell and
/// gains in one — but five units of one match stand for 1400-1875 frames on an
/// assault they cannot finish, so the stuck clause fails. What is missing is the
/// ladder below the order: a job that gives up on a target it cannot reach, and
/// an order that is re-sent when the target it names changes. Both are the next
/// slice; see docs/status.md.
const ATTACK_ENABLED: bool = false;
/// Below this the house keeps its credits for buildings.
const UNIT_SPEND_FLOOR: i32 = 900;

#[derive(Debug, Default)]
pub struct Military;

impl Manager for Military {
    fn propose(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        self.produce(ctx, out);
        self.commit(ctx, out);
    }
}

impl Military {
    /// One unit at a time, cheapest first, while the wallet allows it.
    fn produce(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        // The army does not spend the money the economy is saving: while the
        // house still wants a refinery, its cost stays in the wallet, and only
        // what is left over buys units. Without this the two managers bid
        // against each other every frame and the refinery never lands.
        let refineries = ctx
            .obs
            .own_entities()
            .filter(|unit| {
                ctx.obs
                    .object_type(ctx.rules, unit.type_id)
                    .and_then(|ty| ctx.profiles.get(&ty.id))
                    .is_some_and(|profile| profile.roles.contains(Roles::REFINERY))
            })
            .count();
        let reserve = if refineries < 2 { 1_500 } else { 0 };
        if ctx.obs.credits() < UNIT_SPEND_FLOOR + reserve {
            return;
        }
        let mut infantry: Option<&UnitProfile> = None;
        let mut vehicle: Option<&UnitProfile> = None;
        for ty in ctx.rules.all_objects() {
            if ty.category == ObjectCategory::Building {
                continue;
            }
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if !profile.roles.contains(Roles::ARMED) || profile.roles.contains(Roles::HARVESTER) {
                continue;
            }
            if ctx.obs.can_build(ctx.rules, ty, false) != CanBuild::Yes {
                continue;
            }
            match ty.category {
                ObjectCategory::Infantry => infantry = Self::cheaper(infantry, profile),
                ObjectCategory::Vehicle => vehicle = Self::cheaper(vehicle, profile),
                _ => {}
            }
        }
        for want in [vehicle, infantry].into_iter().flatten() {
            let Some(type_id) = ctx.obs.type_id(&want.id) else {
                continue;
            };
            if ctx.obs.in_production(ctx.rules, type_id) {
                continue;
            }
            out.intents.push(Intent::QueueProduction { type_id });
            return;
        }
    }

    /// The army attacks once it is big enough, and holds the base before that.
    fn commit(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        let mut soldiers = Vec::new();
        let mut home: Option<(u16, u16)> = None;
        for unit in ctx.obs.own_entities() {
            let Some(ty) = ctx.obs.object_type(ctx.rules, unit.type_id) else {
                continue;
            };
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if profile.roles.contains(Roles::STRUCTURE) {
                home = home.or(Some(unit.cell));
                continue;
            }
            if !profile.roles.contains(Roles::ARMED)
                || profile.roles.contains(Roles::HARVESTER)
                || !unit.orderable
            {
                continue;
            }
            soldiers.push(unit);
        }
        // What the house can see, not what the world holds. The projection's own
        // enemy list is omniscient (the native computer AI is, and a policy that
        // reused it would inherit that); the spec forbids aiming at anything the
        // player could not have clicked, so a target is something in line of
        // sight right now. A house that sees nothing holds its post.
        let seen: Vec<EnemyEntity> = ctx.obs.visible_enemy_entities().collect();
        let target = seen
            .iter()
            .min_by_key(|enemy| {
                soldiers
                    .iter()
                    .map(|unit| cell_distance(unit.cell, enemy.cell))
                    .min()
                    .unwrap_or(i32::MAX)
            })
            .map(|enemy| (enemy.entity_id, enemy.cell));
        // A house with no structure yet holds the ground its own units stand
        // on: the starting force is the whole army until the yard is up, and a
        // unit with no post is a unit nobody owns.
        let home = home.or_else(|| soldiers.first().map(|unit| unit.cell));
        let Some(home) = home else {
            return;
        };
        // Enough to go is not enough to win: the group commits only when it
        // outnumbers what stands within reach of the target, at three to two.
        // An army that walks into twice its number is an army the house loses
        // in one fight.
        let near_target = target.map_or(0, |(_, cell)| {
            seen.iter()
                .filter(|enemy| cell_distance(enemy.cell, cell) <= 15)
                .count()
        });
        // Two to one, not three to two: an even fight is a lost army, and the
        // army is the only thing keeping the base.
        let army =
            ATTACK_ENABLED && soldiers.len() >= ARMY_TO_ATTACK && soldiers.len() >= near_target * 2;
        for unit in soldiers {
            match (army, target) {
                (true, Some((entity_id, _))) => out.jobs.push(Proposal {
                    kind: JobKind::Assault,
                    // An object, not a cell: an attack-move to a cell is a
                    // destination the unit reaches and then stands on, which is
                    // the stale-target failure status.md records.
                    target: Target::Object(entity_id),
                    priority: 60,
                    actor: Some(unit.entity_id),
                }),
                _ => out.jobs.push(Proposal {
                    kind: JobKind::Defend,
                    target: Target::Cell(home),
                    priority: 20,
                    actor: Some(unit.entity_id),
                }),
            }
        }
    }

    fn cheaper<'a>(
        current: Option<&'a UnitProfile>,
        candidate: &'a UnitProfile,
    ) -> Option<&'a UnitProfile> {
        match current {
            Some(best) if (best.cost, &best.id) <= (candidate.cost, &candidate.id) => Some(best),
            _ => Some(candidate),
        }
    }
}

/// Chebyshev distance in cells: how far a walk is on this grid.
fn cell_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    i32::from(a.0.abs_diff(b.0)).max(i32::from(a.1.abs_diff(b.1)))
}
