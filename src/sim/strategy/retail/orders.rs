//! The army's direct orders: attack the nearest hostile, capture with
//! engineers.
//!
//! Native teams carry this. An `AITrigger` creates a team, its script assigns
//! missions, and `team_script_vm` writes them onto the members. Those missions
//! have no player equivalent, and a team's script already runs for a
//! brain-driven house, so this module only covers **the units no team has
//! taken**: each is sent at the nearest hostile object (each engineer at the
//! nearest capturable building), which is what a player's click on it means.
//!
//! That is a residual, not a port: native target selection reads team scripts,
//! threat and value, so a play test should expect an army that fights everything
//! hostile it can reach rather than one that fights what matters. Fog is
//! likewise not modelled — the computer AI is omniscient and this inherits that.
//!
//! The brain remembers one order per unit and refreshes it after
//! [`ORDER_REFRESH_FRAMES`] or when a nearer target appears. Re-issuing every
//! frame would spend the whole per-frame budget on one army, and re-issuing a
//! unit that the gate refused would hide the refusal.

use std::collections::BTreeSet;

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::control::Goal;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::{EnemyEntity, Observation};
use crate::sim::strategy::retail::{ORDER_REFRESH_FRAMES, RetailBrain};

/// How many new unit orders one frame may carry. The board drops everything
/// past its own per-frame budget, so asking for more would only spend the frame
/// on orders that never leave; this keeps the surviving orders to the newest
/// units first, in storage order.
const FORCE_ORDERS_PER_FRAME: usize = 8;

/// One order the brain has issued, and when it looks at that unit again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SentOrder {
    pub(super) target: u64,
    pub(super) refresh: u64,
}

/// Every engineer takes the nearest building it may capture, the way the
/// player's engineer click on it does.
///
/// Natively the capture mission starts from the same order path the player's
/// click uses; a brain-driven house never receives that click, so the brain
/// issues the order. An engineer has no weapon, so it is not a combat unit and
/// would otherwise stand idle.
pub(super) fn engineers(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    let own: Vec<_> = obs.own_entities().collect();
    if !own.iter().any(|entity| {
        obs.object_type(rules, entity.type_id)
            .is_some_and(|ty| ty.engineer)
    }) {
        return;
    }
    let targets: Vec<EnemyEntity> = obs.capturable_buildings(rules).collect();
    if targets.is_empty() {
        return;
    }
    let mut issued = 0;
    for entity in &own {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        let Some(ty) = obs.object_type(rules, entity.type_id) else {
            continue;
        };
        if !ty.engineer {
            continue;
        }
        if obs.team_has_member(entity.entity_id) {
            continue;
        }
        let Some(target) = nearest_enemy(&targets, entity.cell) else {
            continue;
        };
        let current = brain
            .orders
            .get(&entity.entity_id)
            .is_some_and(|sent| sent.target == target.entity_id && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            entity.entity_id,
            SentOrder {
                target: target.entity_id,
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(
            entity.entity_id,
            Goal::Capture {
                target: target.entity_id,
            },
        );
        issued += 1;
    }
}

/// Send the army at the enemy.
pub(super) fn forces(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    let own: Vec<_> = obs.own_entities().collect();
    // Forget the units the house no longer has, so the memory stays bounded by
    // the army and a recycled id cannot inherit an old order.
    let ids: BTreeSet<u64> = own.iter().map(|entity| entity.entity_id).collect();
    brain.orders.retain(|id, _| ids.contains(id));
    let enemies: Vec<EnemyEntity> = obs.enemy_entities().collect();
    if enemies.is_empty() {
        return;
    }
    let mut issued = 0;
    for entity in &own {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        let Some(ty) = obs.object_type(rules, entity.type_id) else {
            continue;
        };
        if !is_combat_unit(rules, ty) {
            continue;
        }
        // A team's script owns its members; this step is for the units no team
        // has taken.
        if obs.team_has_member(entity.entity_id) {
            continue;
        }
        let Some(target) = nearest_enemy(&enemies, entity.cell) else {
            continue;
        };
        let current = brain
            .orders
            .get(&entity.entity_id)
            .is_some_and(|sent| sent.target == target.entity_id && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            entity.entity_id,
            SentOrder {
                target: target.entity_id,
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(
            entity.entity_id,
            Goal::Attack {
                target: target.entity_id,
            },
        );
        issued += 1;
    }
}

/// Whether an object is one these orders step around: an infantry, vehicle or
/// aircraft that is neither a gatherer nor a Construction Yard maker.
/// Buildings and terrain are not.
fn is_combat_unit(rules: &RuleSet, ty: &ObjectType) -> bool {
    matches!(
        ty.category,
        ObjectCategory::Infantry | ObjectCategory::Vehicle | ObjectCategory::Aircraft
    ) && ty.primary().is_some()
        && !ty.engineer
        && !ty.harvester
        && !ty.resource_gatherer
        && !crate::sim::mcv_deploy::deploys_into_of(rules, ty)
            .is_some_and(|yard| yard.build_const_eligible)
}

/// The enemy object nearest `from`, ties going to the earlier one in storage
/// order. Integer cells, so the choice is deterministic.
fn nearest_enemy(enemies: &[EnemyEntity], from: (u16, u16)) -> Option<EnemyEntity> {
    let mut best: Option<(i32, EnemyEntity)> = None;
    for target in enemies {
        let dx = i32::from(target.cell.0) - i32::from(from.0);
        let dy = i32::from(target.cell.1) - i32::from(from.1);
        let distance = dx * dx + dy * dy;
        if best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, *target));
        }
    }
    best.map(|(_, target)| target)
}
