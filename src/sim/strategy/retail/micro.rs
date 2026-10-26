//! What the army does *this second*: the layer between a team's target and a
//! unit's mission.
//!
//! The teams above decide where the army goes. What they do not decide, and
//! what a player decides continuously, is what each unit shoots on the way:
//! an army told to walk at a base and nothing else walks past the tanks
//! sitting between it and that base, because the fight was never its order —
//! the walk was. That is the whole of "the push is dense and blind", and it is
//! not fixed by choosing a better target: it is fixed by asking, every second
//! and per unit, what is in reach *now*.
//!
//! So the cycle here is short ([`super::tracked::MICRO_FRAMES`], a second) and
//! its answers override a march rather than being overridden by one: a unit
//! whose reach already holds a hostile is told to shoot it, and the march is
//! re-decided when the shooting is over. The intent it records is
//! [`TrackedKind::Engage`], whose term is one cycle — long enough that the
//! order is not re-sent every frame, short enough that a unit which has
//! finished fighting is the march's again inside a second.
//!
//! The rest of the layer is what each role does when nothing is in reach: a
//! defender walks at the raid, a scout walks where it can see what the enemy is
//! doing. The fist is left alone — the advance step owns it.
//!
//! Determinism: the ledger's own order, squared integer distances, id
//! tie-breaks.

use crate::rules::ruleset::RuleSet;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::Observation;
use crate::sim::mission::{MissionId, MissionType};

use super::RetailBrain;
use super::squads::Role;
use super::tracked::TrackedKind;
use crate::sim::control::Goal;

/// How many engagements one frame may start. The board drops what is past its
/// own budget, and a frame that spends everything on the first squad it looked
/// at has none left for the squad under attack.
const MICRO_ORDERS_PER_FRAME: usize = 8;

/// How far past its own reach a unit will still take a fight, in cells. One:
/// the cell it is about to step into, so a closing hostile is met rather than
/// walked into.
const ENGAGE_MARGIN_CELLS: i32 = 1;

/// One unit's share of this cycle.
struct Work {
    entity_id: u64,
    role: Role,
    cell: (u16, u16),
    /// What its weapon reaches, in cells; zero means it cannot shoot at all.
    reach: i32,
    /// What its team is aimed at, when the team is aimed at something.
    squad_target: Option<(u64, (u16, u16))>,
}

pub(super) fn run(brain: &mut RetailBrain, rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
    let tick = obs.tick();
    // Snapshotted: the ledger and the teams are borrowed while they are read,
    // and the intent table is written below.
    let work = collect(brain, rules, obs, tick);
    let mut issued = 0;
    for item in work {
        if issued >= MICRO_ORDERS_PER_FRAME {
            break;
        }
        // What is in reach now outranks where the unit was going: this is the
        // whole reason the layer exists.
        if let Some(enemy) = in_reach(&brain.census.enemies, item.cell, item.reach) {
            if !brain.tracked.admits(
                tick,
                item.entity_id,
                TrackedKind::Engage,
                Some(enemy.0),
                Some(enemy.1),
            ) {
                continue;
            }
            brain.tracked.record(
                tick,
                item.entity_id,
                TrackedKind::Engage,
                Some(enemy.0),
                Some(enemy.1),
            );
            out.order(item.entity_id, Goal::Attack { target: enemy.0 });
            issued += 1;
            continue;
        }
        // Nothing in reach: the role says where to be.
        match item.role {
            // The fist is the advance step's; this layer does not walk it.
            Role::Attack => continue,
            Role::Defend => {
                let Some((target, cell)) = item.squad_target else {
                    continue;
                };
                if !brain.tracked.admits(
                    tick,
                    item.entity_id,
                    TrackedKind::Attack,
                    Some(target),
                    Some(cell),
                ) {
                    continue;
                }
                brain.tracked.record(
                    tick,
                    item.entity_id,
                    TrackedKind::Attack,
                    Some(target),
                    Some(cell),
                );
                out.order(item.entity_id, Goal::Attack { target });
                issued += 1;
            }
            // A scout is sent to look, not to fight: a walk, so it does not
            // stop to bite every tank it passes on the way to the map.
            Role::Scout => {
                let Some((_, cell)) = nearest(&brain.census.enemies, item.cell) else {
                    continue;
                };
                if !brain
                    .tracked
                    .admits(tick, item.entity_id, TrackedKind::Move, None, Some(cell))
                {
                    continue;
                }
                brain
                    .tracked
                    .record(tick, item.entity_id, TrackedKind::Move, None, Some(cell));
                out.order(item.entity_id, Goal::Move { cell });
                issued += 1;
            }
        }
    }
}

/// Every team member that may be told anything, with what the cycle needs to
/// know about it.
fn collect(brain: &RetailBrain, rules: &RuleSet, obs: &Observation<'_>, tick: u64) -> Vec<Work> {
    let mut work = Vec::new();
    for squad in brain.squads.all() {
        let squad_target = squad.target.and_then(|target| {
            brain
                .census
                .enemies
                .iter()
                .find(|enemy| enemy.entity_id == target)
                .map(|enemy| (target, enemy.cell))
        });
        for unit in &brain.census.units {
            if !squad.members.contains(&unit.entity_id) || !unit.orderable || unit.hand {
                continue;
            }
            // A unit already mid-fight is told nothing: its mission owns it,
            // and re-ordering it is what makes the fight stutter.
            if unit.mission == MissionId::from_known(MissionType::Attack) {
                continue;
            }
            // A unit a routine has taken is that routine's: the retreat owns
            // it while it walks the unit out.
            if brain.tracked.held(tick, unit.entity_id) {
                continue;
            }
            let reach = obs
                .object_type(rules, unit.type_id)
                .map_or(0, |ty| super::advance::weapon_range_cells(rules, ty));
            work.push(Work {
                entity_id: unit.entity_id,
                role: squad.role,
                cell: unit.cell,
                reach,
                squad_target,
            });
        }
    }
    work
}

/// The nearest hostile inside `reach`, as `(id, cell)`; ties by id.
fn in_reach(
    enemies: &[crate::sim::control::observation::EnemyEntity],
    from: (u16, u16),
    reach: i32,
) -> Option<(u64, (u16, u16))> {
    if reach <= 0 {
        return None;
    }
    let reach = (reach + ENGAGE_MARGIN_CELLS).max(1);
    enemies
        .iter()
        .filter(|enemy| squared_distance(enemy.cell, from) <= reach * reach)
        .min_by_key(|enemy| (squared_distance(enemy.cell, from), enemy.entity_id))
        .map(|enemy| (enemy.entity_id, enemy.cell))
}

/// The nearest hostile of any kind, as `(id, cell)`; ties by id.
fn nearest(
    enemies: &[crate::sim::control::observation::EnemyEntity],
    from: (u16, u16),
) -> Option<(u64, (u16, u16))> {
    enemies
        .iter()
        .min_by_key(|enemy| (squared_distance(enemy.cell, from), enemy.entity_id))
        .map(|enemy| (enemy.entity_id, enemy.cell))
}

fn squared_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    let dx = i32::from(a.0) - i32::from(b.0);
    let dy = i32::from(a.1) - i32::from(b.1);
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::control::observation::EnemyEntity;
    use crate::sim::intern::InternedId;

    /// What a unit is told to shoot is what its own weapon reaches, and the
    /// margin is the cell it is about to step into — not the whole map, and
    /// not nothing.
    #[test]
    fn what_is_in_reach_is_what_the_unit_can_shoot() {
        let enemies = vec![EnemyEntity {
            entity_id: 42,
            type_id: InternedId::default(),
            cell: (22, 20),
        }];
        assert_eq!(
            in_reach(&enemies, (20, 20), 5),
            Some((42, (22, 20))),
            "two cells away, and the weapon reaches five"
        );
        let far = vec![EnemyEntity {
            entity_id: 43,
            type_id: InternedId::default(),
            cell: (40, 20),
        }];
        assert_eq!(
            in_reach(&far, (20, 20), 1),
            None,
            "twenty cells away is out of a one-cell reach"
        );
        assert_eq!(
            in_reach(&enemies, (20, 20), 0),
            None,
            "a unit with no weapon engages nothing"
        );
    }

    /// The nearest hostile is the one a scout is sent to look at.
    #[test]
    fn a_scout_is_sent_to_the_nearest_thing_it_can_see() {
        let enemies = vec![
            EnemyEntity {
                entity_id: 7,
                type_id: InternedId::default(),
                cell: (40, 40),
            },
            EnemyEntity {
                entity_id: 9,
                type_id: InternedId::default(),
                cell: (21, 20),
            },
        ];
        assert_eq!(nearest(&enemies, (20, 20)), Some((9, (21, 20))));
    }
}
