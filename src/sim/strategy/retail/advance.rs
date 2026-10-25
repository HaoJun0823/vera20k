//! Sending the army out.
//!
//! Native carries the attack in teams: an `AITrigger` creates one and its
//! script assigns the missions. A brain-driven house never gets an `AITrigger`
//! — `HouseClass::Update`'s team block is gated on "no human controls it" —
//! so without this module its army sits on area guard for the whole match and
//! the house can only lose by attrition.
//!
//! This is the player's substitute, and deliberately two rules. The first is
//! *when*: an army large enough to be worth spending, a base that can replace
//! it, and nothing already at the yard. The second is *where*: one structure
//! for the whole army — the enemy economy structure nearest the first fighter
//! in entity-store order — because a fist takes a base, and units that walk
//! at their own nearest building arrive one at a time and die one at a time.
//!
//! One goal for the army is a measurement, not a preference. Three variants
//! were run end to end against the pinned `Death.mmx` match and all three
//! lost the push the current rule wins: aiming each unit at the structure its
//! own weapons answer best, aiming the fist from the army's centre instead of
//! that first fighter, and holding one target until it is destroyed instead
//! of re-reading the nearest one each frame (`super::RESIDUALS` has the
//! ticks). What it shoots on the way, and whether it holds what it took, stay
//! the sim's own `AttackMove` and `Guard` behaviours: this module decides
//! *when* to push and *where*, not how to fight.
//!
//! While it is pushing it owns the whole army, so the area-guard step stands
//! down for the frame; a house that has not got an army yet keeps holding its
//! base, which is what a player does with five tanks and no plan.

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::control::Goal;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::{EnemyEntity, Observation};
use crate::sim::mission::{MissionId, MissionType};

use super::orders::SentOrder;
use super::{ORDER_REFRESH_FRAMES, RetailBrain};

/// How large the army has to be before it is worth pushing with.
///
/// Below this the opening force is the whole army, and an opening force that
/// walks into the enemy's pre-placed garrison is a house that has traded its
/// base for nothing.
const ARMY_TO_ATTACK: usize = 10;

/// How many units one frame may re-task. The board's per-frame budget drops
/// everything past its own limit, so a larger number here would only spend
/// the frame on orders that never leave.
const ADVANCE_ORDERS_PER_FRAME: usize = 8;

/// How close an enemy has to come to the yard before the army stays home,
/// in cells. A raid inside this ring is what the army was bought for.
const THREAT_RADIUS: i32 = 18;

/// Send the army at the enemy's base when it is large enough. `false` when
/// the house is not pushing this frame, so the caller keeps its own posture.
pub(super) fn advance(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) -> bool {
    let fighters: Vec<_> = obs
        .own_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| super::orders::is_combat_unit(rules, ty))
        })
        .filter(|entity| !obs.team_has_member(entity.entity_id))
        .collect();
    // An army the base cannot replace is not an army, it is the whole house
    // walking out of the gate; and a base with nothing to hold it is a base
    // that falls while the army is away. The third gate is the one that
    // decides matches: an enemy already at the yard is what the army is for,
    // and marching out with it in the base loses the production the army
    // came from.
    if fighters.len() < ARMY_TO_ATTACK
        || !super::build::can_replace_units(rules, obs)
        || !super::build::base_defended(rules, obs)
        || base_under_threat(rules, obs)
    {
        return false;
    }
    // Structures, not units: a base is what a house loses with, and a
    // defending unit walks into the push on its own. Refineries first — a
    // house that loses its unloading bays loses the war it is paying for,
    // and the opponent's wallet is the one number that decides the late game.
    let structures: Vec<(&ObjectType, EnemyEntity)> = obs
        .enemy_entities()
        .filter_map(|enemy| {
            obs.object_type(rules, enemy.type_id)
                .filter(|ty| ty.category == ObjectCategory::Building)
                .map(|ty| (ty, enemy))
        })
        .collect();
    let economy: Vec<EnemyEntity> = structures
        .iter()
        .filter(|(ty, _)| ty.resource_destination)
        .map(|(_, enemy)| *enemy)
        .collect();
    let targets: Vec<EnemyEntity> = if economy.is_empty() {
        structures.iter().map(|(_, enemy)| *enemy).collect()
    } else {
        economy
    };
    if targets.is_empty() {
        return false;
    }

    // One target for the whole army, not the nearest for each unit: units
    // pointed at their own nearest structure arrive one at a time and are
    // killed one at a time. A fist is what takes a base.
    let anchor = fighters.first().map_or((0, 0), |entity| entity.cell);
    let Some(target) = nearest_target(&targets, anchor) else {
        return false;
    };

    let tick = obs.tick();
    let mut issued = 0;
    for entity in &fighters {
        if issued >= ADVANCE_ORDERS_PER_FRAME {
            break;
        }
        // A unit already fighting or walking is doing what this would ask.
        if matches!(
            entity.mission,
            mission if mission == MissionId::from_known(MissionType::Attack)
                || mission == MissionId::from_known(MissionType::Move)
        ) {
            continue;
        }
        // The step that owns a specialist order already spent this unit's frame:
        // a spy walking into a building and a rifleman boarding a carrier both
        // outrank the push.
        if out.orders.iter().any(|order| order.actor == entity.entity_id) {
            continue;
        }
        let current = brain
            .orders
            .get(&entity.entity_id)
            .is_some_and(|sent| sent.target == Some(target.entity_id) && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            entity.entity_id,
            SentOrder {
                target: Some(target.entity_id),
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(entity.entity_id, Goal::AttackMove { cell: target.cell });
        issued += 1;
    }
    true
}

/// Whether an enemy is close enough to the yard that the army belongs at
/// home. `true` when the house has no yard to measure from: a house with
/// nothing to defend has nothing to gain by walking away either.
fn base_under_threat(rules: &RuleSet, obs: &Observation<'_>) -> bool {
    let Some(base) = obs.own_entities().find(|entity| {
        obs.object_type(rules, entity.type_id)
            .is_some_and(|ty| ty.build_const_eligible)
    }) else {
        return true;
    };
    obs.enemy_entities().any(|enemy| {
        let dx = i32::from(enemy.cell.0) - i32::from(base.cell.0);
        let dy = i32::from(enemy.cell.1) - i32::from(base.cell.1);
        dx * dx + dy * dy <= THREAT_RADIUS * THREAT_RADIUS
    })
}

/// The enemy structure nearest `from`, ties going to the earlier one in
/// storage order. Integer cells, so the choice is deterministic.
fn nearest_target(targets: &[EnemyEntity], from: (u16, u16)) -> Option<EnemyEntity> {
    let mut best: Option<(i32, EnemyEntity)> = None;
    for target in targets {
        let dx = i32::from(target.cell.0) - i32::from(from.0);
        let dy = i32::from(target.cell.1) - i32::from(from.1);
        let distance = dx * dx + dy * dy;
        if best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, *target));
        }
    }
    best.map(|(_, target)| target)
}
