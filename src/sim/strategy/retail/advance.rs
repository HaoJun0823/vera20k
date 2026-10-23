//! Sending the army out.
//!
//! Native carries the attack in teams: an `AITrigger` creates one and its
//! script assigns the missions. A brain-driven house never gets an `AITrigger`
//! — `HouseClass::Update`'s team block is gated on "no human controls it" —
//! so without this module its army sits on area guard for the whole match and
//! the house can only lose by attrition.
//!
//! This is the player's substitute, and deliberately one rule: when the army
//! is large enough to be worth spending, every idle fighting unit is sent at
//! the enemy structure nearest it. What it shoots on the way, and whether it
//! holds what it took, stay the sim's own `AttackMove` and `Guard`
//! behaviours: this module decides *when* to push and *where*, not how to
//! fight.
//!
//! While it is pushing it owns the whole army, so the area-guard step stands
//! down for the frame; a house that has not got an army yet keeps holding its
//! base, which is what a player does with five tanks and no plan.

use crate::rules::object_type::ObjectCategory;
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
const ARMY_TO_ATTACK: usize = 8;

/// How many units one frame may re-task. The board's per-frame budget drops
/// everything past its own limit, so a larger number here would only spend
/// the frame on orders that never leave.
const ADVANCE_ORDERS_PER_FRAME: usize = 8;

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
    // walking out of the gate.
    if fighters.len() < ARMY_TO_ATTACK || !super::build::can_replace_units(rules, obs) {
        return false;
    }
    // Structures, not units: a base is what a house loses with, and a
    // defending unit walks into the push on its own.
    let targets: Vec<EnemyEntity> = obs
        .enemy_entities()
        .filter(|enemy| {
            obs.object_type(rules, enemy.type_id)
                .is_some_and(|ty| ty.category == ObjectCategory::Building)
        })
        .collect();
    if targets.is_empty() {
        return false;
    }

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
        let Some(target) = nearest_target(&targets, entity.cell) else {
            continue;
        };
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
