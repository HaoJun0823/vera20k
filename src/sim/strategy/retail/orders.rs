//! The army's direct orders: put an idle combat unit on area guard, and every
//! unit that takes a building on foot — an engineer capturing, a spy
//! infiltrating — on the nearest one it may.
//!
//! Native teams carry the attack. An `AITrigger` creates a team, its script
//! assigns missions, and `team_script_vm` writes them onto the members. Those
//! missions have no player equivalent, and a team's script already runs for a
//! brain-driven house, so this module only covers **the units no team has
//! taken**: an idle one goes on area guard where it stands, and the sim's own
//! Guard mission decides what it engages and where it returns to. That is the
//! native posture for a unit no team has taken — it sits, it does not charge,
//! and it is the sim, not this module, that picks its fight.
//!
//! Sending every idle unit at the nearest hostile instead is what a play test
//! loses with: the opening squad walks into the enemy's own pre-placed force
//! and dies, the plan loses the units it was going to recruit, and the base
//! keeps spending on replacements it cannot afford.
//!
//! That leaves the offensive half to the teams, which is where the retail AI
//! puts it: a hostile the enemy owns is what an `AITrigger` needs, and the
//! enemy search that sets it runs in the house's own Strategy tick
//! (`sim::house_strategy`), which the brain asks for.
//!
//! The brain remembers one order per unit and refreshes it after
//! [`ORDER_REFRESH_FRAMES`] or when the order would change. Re-issuing every
//! frame would spend the whole per-frame budget on one army, and re-issuing a
//! unit that the gate refused would hide the refusal.

use std::collections::BTreeSet;

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::control::Goal;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::{EnemyEntity, Observation};
use crate::sim::mission::{MissionId, MissionType};
use crate::sim::strategy::retail::{ORDER_REFRESH_FRAMES, RetailBrain};

/// How many new unit orders one frame may carry. The board drops everything
/// past its own per-frame budget, so asking for more would only spend the frame
/// on orders that never leave; this keeps the surviving orders to the newest
/// units first, in storage order.
const FORCE_ORDERS_PER_FRAME: usize = 8;

/// One order the brain has issued, and when it looks at that unit again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SentOrder {
    /// The hostile the unit was sent at, or `None` for the hold order.
    pub(super) target: Option<u64>,
    pub(super) refresh: u64,
}

/// Every unit that walks into an enemy building takes the nearest one it may:
/// the Capture click, which an engineer's capture and a spy's infiltration both
/// are.
///
/// Natively the capture mission starts from the same order path the player's
/// click uses; a brain-driven house never receives that click, so the brain
/// issues the order. The two units differ only in what the building's own
/// per-cell handler does when they arrive — `InfantryClass::PerCellProcess`
/// captures it for an `Engineer=` and infiltrates it for an `Infiltrate=` — so
/// one order serves both, and neither unit would otherwise do anything: a spy
/// carries no weapon worth ranking and an engineer carries none at all.
pub(super) fn captures(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    let own: Vec<_> = obs.own_entities().collect();
    if !own.iter().any(|entity| {
        obs.object_type(rules, entity.type_id)
            .is_some_and(walks_into_buildings)
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
        if !walks_into_buildings(ty) {
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
        out.order(
            entity.entity_id,
            Goal::Capture {
                target: target.entity_id,
            },
        );
        issued += 1;
    }
}

/// Put every idle combat unit no team has taken on area guard where it stands.
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
    if !own.iter().any(|entity| {
        obs.object_type(rules, entity.type_id)
            .is_some_and(|ty| is_combat_unit(rules, ty))
    }) {
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
        // A unit that is already holding or fighting is doing exactly this, and
        // its mission owns it: a second Guard would cancel the fight.
        if holds_or_fights(entity.mission) {
            continue;
        }
        // The advance step owns a unit it is pushing: a guard here would pull
        // it back out of the push.
        if brain
            .orders
            .get(&entity.entity_id)
            .is_some_and(|sent| sent.target.is_some() && tick < sent.refresh)
        {
            continue;
        }
        let current = brain
            .orders
            .get(&entity.entity_id)
            .is_some_and(|sent| sent.target.is_none() && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            entity.entity_id,
            SentOrder {
                target: None,
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(entity.entity_id, Goal::Guard);
        issued += 1;
    }
}

/// Whether the object's mission already holds the ground it stands on with the
/// wider reach this step asks for, or has it fighting.
///
/// `Move` is here for the same reason: a unit the house is still walking
/// somewhere is not idle, and replacing that move with a guard would stop it
/// half-way. `Guard` is *not*: that mission holds the cell the object stands on
/// and returns fire, while `AreaGuard` also steps out to engage a hostile that
/// comes near — the posture a unit no team has taken holds, and the one
/// `[IQ] GuardArea` gates for a computer house.
fn holds_or_fights(mission: MissionId) -> bool {
    [
        MissionType::Attack,
        MissionType::Move,
        MissionType::AreaGuard,
    ]
    .into_iter()
    .any(|kind| mission == MissionId::from_known(kind))
}

/// Whether a unit's click on an enemy building is a Capture click: the two
/// flags the per-cell handler branches on, an engineer's by `Engineer=` and a
/// spy's by `Infiltrate=` (which `Engineer=` also implies, per
/// `InfantryTypeClass::ReadINI` `0x0052466E..0x0052469A`).
pub(super) fn walks_into_buildings(ty: &ObjectType) -> bool {
    ty.engineer || ty.infiltrate
}

/// Whether an object is one these orders step around: an infantry, vehicle or
/// aircraft that is neither a gatherer nor a Construction Yard maker.
/// Buildings and terrain are not.
pub(super) fn is_combat_unit(rules: &RuleSet, ty: &ObjectType) -> bool {
    matches!(
        ty.category,
        ObjectCategory::Infantry | ObjectCategory::Vehicle | ObjectCategory::Aircraft
    ) && ty.primary().is_some()
        && !ty.engineer
        && !ty.harvester
        && !ty.resource_gatherer
        && !ty
            .deploys_into
            .as_deref()
            .and_then(|name| rules.object(name))
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::world::Simulation;

    /// The retail rival arena, needed to field real object types. `None`
    /// without `RA2_DIR`, as the rest of the suite does.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
        let retail = crate::rules::retail_ini_fixture::retail_battle_rules()?;
        let rules = retail.rules;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    /// A spy's click on a building is the same Capture click an engineer's is:
    /// one order, and the per-cell handler decides which of the two it is. A
    /// spy has no combat value to rank, so without this order it stands where
    /// it was built.
    #[test]
    fn a_spy_walks_into_a_building_the_way_an_engineer_does() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let spy = sim
            .spawn_object("SPY", "Americans", 8, 8, 64, &rules)
            .expect("a spy");
        sim.spawn_object("CAOILD", "Russians", 20, 20, 64, &rules)
            .expect("a capturable derrick");
        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        let mut out = Outbox::default();
        captures(&mut brain, &rules, &obs, &mut out);

        assert!(
            out.orders
                .iter()
                .any(|order| order.actor == spy && matches!(order.goal, Goal::Capture { .. })),
            "a spy is sent into the building, not left standing: {:?}",
            out.orders
        );
        assert!(
            walks_into_buildings(rules.object("SPY").expect("the spy")),
            "Infiltrate= is a Capture click"
        );
        assert!(
            walks_into_buildings(rules.object("ENGINEER").expect("the engineer")),
            "and so is Engineer="
        );
        assert!(
            !walks_into_buildings(rules.object("E1").expect("a GI")),
            "a rifleman is not"
        );
    }
}
