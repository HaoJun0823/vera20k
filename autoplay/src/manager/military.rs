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
const ATTACK_ENABLED: bool = true;
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
        // Where the house's base is. Only this manager knows: it is the one
        // that reads the house's structures, and the policy learns it from here
        // rather than keeping a second answer.
        out.home = Some(home);
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
        // Why the army is or is not going is a reading, not something to infer
        // from a count that did not move: three separate conditions gate it, and
        // "the matrix showed no change" cannot tell which one said no. It settled
        // one question already — the army reaches its size and goes, and `seen=0`
        // means there is nothing in sight to go at.
        // With nothing in sight, a big-enough column marches at the edge of known
        // ground instead of guarding the base. Without it the army never leaves:
        // the bases are 116 cells apart, nothing walks between them, so `seen` is
        // zero on every reading and the assault gate never opens. That is the
        // deadlock status.md records.
        let advance = if army && target.is_none() {
            march_cell(ctx, home, soldiers.first().map_or(home, |unit| unit.cell))
        } else {
            None
        };
        if ctx.tick % 1800 == 0 {
            log::info!(
                "military: tick={} enabled={} soldiers={} want={} seen={} near={} going={} advance={:?}",
                ctx.tick,
                ATTACK_ENABLED,
                soldiers.len(),
                ARMY_TO_ATTACK,
                seen.len(),
                near_target,
                army,
                advance,
            );
        }
        for unit in soldiers {
            match (army, target, advance) {
                (true, Some((entity_id, _)), _) => out.jobs.push(Proposal {
                    kind: JobKind::Assault,
                    // An object, not a cell: an attack-move to a cell is a
                    // destination the unit reaches and then stands on, which is
                    // the stale-target failure status.md records.
                    target: Target::Object(entity_id),
                    priority: 60,
                    actor: Some(unit.entity_id),
                }),
                (true, None, Some(cell)) => out.jobs.push(Proposal {
                    // A march ordered as an assault: an attack-move fights
                    // whatever it meets on the way instead of walking past it.
                    kind: JobKind::Assault,
                    target: Target::Cell(cell),
                    priority: 55,
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

/// Where a column with no target should march: an unrevealed cell **on the edge of
/// ground the house already knows**, inside the playfield, within one march.
///
/// Edge of known ground, not "the farthest unrevealed cell", and that is the fix
/// for the case that killed the previous version. Marching deep into unexplored
/// terrain picks cells behind walls and water: the last attempt ordered a column
/// to `Cell((132, 204))`, the column stopped at `(153, 181)` — 23 cells away, a
/// cell it could not stand on — and stood there for the rest of the match, reading
/// `stuck=40`. A cell that touches known ground is one a unit standing on the
/// known ground could have walked to, so the frontier is the only place a blind
/// march can safely aim.
///
/// Ties break by distance from home, preferring the further cell, so the column
/// keeps opening new ground instead of clearing its own neighbourhood. That was
/// the previous version's other bug: nearest-from-home marched east while the
/// enemy held the west.
fn march_cell(ctx: &Ctx<'_>, home: (u16, u16), from: (u16, u16)) -> Option<(u16, u16)> {
    let (width, height) = ctx.obs.map_extent();
    if width == 0 || height == 0 {
        return None;
    }
    let mut best: Option<((u16, u16), i32, i32)> = None;
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            if !ctx.obs.is_cell_in_playfield(x, y)
                || ctx.obs.is_cell_revealed(x, y)
                || cell_distance(from, (x, y)) > i32::from(MARCH_REACH_CELLS)
            {
                x = x.saturating_add(MARCH_SCAN_STEP);
                continue;
            }
            if let Some(edge) = frontier_distance(ctx, x, y) {
                // A cell right on the known edge is a cell a unit could have
                // walked to. Rank by how close it is to that edge, then by how far
                // it takes the column from home.
                let reach = cell_distance(home, (x, y));
                let better = match best {
                    None => true,
                    Some((_, best_edge, best_reach)) => {
                        edge < best_edge || (edge == best_edge && reach > best_reach)
                    }
                };
                if better {
                    best = Some(((x, y), edge, reach));
                }
            }
            x = x.saturating_add(MARCH_SCAN_STEP);
        }
        y = y.saturating_add(MARCH_SCAN_STEP);
    }
    best.map(|(cell, _, _)| cell)
}

/// How many steps an unrevealed cell is from the nearest revealed one, up to
/// [`FRONTIER_SEARCH`]; `None` when no known ground is that close.
fn frontier_distance(ctx: &Ctx<'_>, x: u16, y: u16) -> Option<i32> {
    let mut edge = 1;
    while edge <= FRONTIER_SEARCH {
        let steps = edge as u16;
        for dy in 0..=steps {
            for dx in 0..=steps {
                if dx.max(dy) != steps {
                    continue;
                }
                for (sx, sy) in [
                    (i32::from(x) + i32::from(dx), i32::from(y) + i32::from(dy)),
                    (i32::from(x) + i32::from(dx), i32::from(y) - i32::from(dy)),
                    (i32::from(x) - i32::from(dx), i32::from(y) + i32::from(dy)),
                    (i32::from(x) - i32::from(dx), i32::from(y) - i32::from(dy)),
                ] {
                    if sx < 0 || sy < 0 {
                        continue;
                    }
                    let (sx, sy) = (sx as u16, sy as u16);
                    if ctx.obs.is_cell_revealed(sx, sy) {
                        return Some(edge);
                    }
                }
            }
        }
        edge += 1;
    }
    None
}

/// How far the frontier search looks for known ground, in cells.
const FRONTIER_SEARCH: i32 = 4;

/// How far one march goes, in cells: inside what a march is measured to cover.
const MARCH_REACH_CELLS: u16 = 18;

/// How coarsely the march scan samples the map, in cells.
const MARCH_SCAN_STEP: u16 = 4;

/// Chebyshev distance in cells: how far a walk is on this grid.
fn cell_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    i32::from(a.0.abs_diff(b.0)).max(i32::from(a.1.abs_diff(b.1)))
}
