//! The army's direct orders, for the units no team has taken: an idle combat
//! unit goes on area guard, an engineer walks into the bridge hut whose span is
//! down or into a building it may capture, a spy walks into the building worth
//! most, and an `Occupier=` rifleman garrisons a building near the base.
//!
//! Each is a click the player already has, issued as the same command: the
//! Capture click (engineers, spies, bridge huts) and the Enter click (garrisons).
//! Nothing here decides what a click *does* — `InfantryClass::PerCellProcess`,
//! `BuildingClass::CanBeOccupiedBy` and the entry queries own that.
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

/// How near one of the house's own buildings a bridge or a garrison has to be
/// for the policy to spend a unit on it, in cells. A garrison is only worth
/// holding where the base is, and a bridge nobody crosses is an engineer spent
/// for nothing.
const HOME_REACH_CELLS: i32 = 12;

// Why there is no boarding step here. Wiring one — an idle rifleman climbing
// into his own empty carrier, a correct Enter click (`Goal::Enter` and
// `can_enter_transport` both answered as expected) — cost the reference match
// nine thousand ticks: `won at 25636` became `won at 34897`, and with the army's
// strength count corrected to skip units the gate would refuse, the match
// stopped winning inside its 36000-tick cap. Whether the carrier loses value
// once loaded or the boarded unit is simply not where the push needs it is not
// established, so the step stays out and recorded rather than guessed at.

/// Every unit that walks into an enemy building takes one it may: the Capture
/// click, which an engineer's capture and a spy's infiltration both are.
///
/// Natively the capture mission starts from the same order path the player's
/// click uses; a brain-driven house never receives that click, so the brain
/// issues the order. The two units differ only in what the building's own
/// per-cell handler does when they arrive — `InfantryClass::PerCellProcess`
/// captures it for an `Engineer=` and infiltrates it for an `Infiltrate=` — so
/// one order serves both, and neither unit would otherwise do anything: a spy
/// carries no weapon worth ranking and an engineer carries none at all.
///
/// What they walk at differs, because the click's own admission does: a spy
/// needs `Spyable=` (see [`Observation::spyable_buildings`]) while an engineer
/// needs `Capturable=`. And a spy's target is picked by what the building is
/// worth, not by what is nearest — walking the house's only spy into the
/// derrick next door is how the feature is wasted.
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
    let engineer_targets: Vec<EnemyEntity> = obs.capturable_buildings(rules).collect();
    let spy_targets: Vec<(EnemyEntity, u8)> = obs
        .spyable_buildings(rules)
        .filter_map(|target| {
            let ty = obs.object_type(rules, target.type_id)?;
            Some((target, infiltration_value(rules, ty)))
        })
        .collect();
    let mut issued = 0;
    for entity in &own {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        if !entity.orderable || ordered_this_frame(out, entity.entity_id) {
            continue;
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
        // A spy takes what is worth most; anyone else takes what is nearest.
        let chosen = if ty.agent {
            spy_targets
                .iter()
                .max_by_key(|(target, value)| {
                    (*value, -squared_distance(entity.cell, target.cell))
                })
                .map(|(target, _)| *target)
        } else {
            nearest_enemy(&engineer_targets, entity.cell)
        };
        let Some(target) = chosen else {
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

/// Send an engineer into a `BridgeRepairHut=` whose span is down.
///
/// Walking in is the only thing that rebuilds a bridge: the hut carries no
/// timer and no `Capturable=`, and `BridgeRepairHut` is retail's only consumer
/// ([`Observation::bridge_repair_huts`]). The engineer is consumed by it, which
/// is also retail's price, so this only pays for a bridge inside the house's
/// own reach.
pub(super) fn bridges(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    let own: Vec<_> = obs.own_entities().collect();
    if !own
        .iter()
        .any(|entity| obs.object_type(rules, entity.type_id).is_some_and(|ty| ty.engineer))
    {
        return;
    }
    let near: Vec<(u16, u16)> = home_buildings(rules, obs);
    let huts: Vec<(u64, (u16, u16))> = obs
        .bridge_repair_huts(rules)
        .filter(|site| within(near.iter().copied(), site.cell, HOME_REACH_CELLS))
        .map(|site| (site.building_id, site.cell))
        .collect();
    if huts.is_empty() {
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
        if !ty.engineer
            || !entity.orderable
            || obs.team_has_member(entity.entity_id)
            || ordered_this_frame(out, entity.entity_id)
            || holds_or_fights(entity.mission)
        {
            continue;
        }
        let Some(target) = nearest_site(&huts, entity.cell) else {
            continue;
        };
        let current = brain
            .orders
            .get(&entity.entity_id)
            .is_some_and(|sent| sent.target == Some(target) && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            entity.entity_id,
            SentOrder {
                target: Some(target),
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(entity.entity_id, Goal::Capture { target });
        issued += 1;
    }
}

/// Idle `Occupier=` infantry garrison a building near the house's own base.
///
/// `BuildingClass::CanBeOccupiedBy` decides whether this infantryman may walk
/// in, so a full building, a red one and a stranger's are all refused by the
/// owner rather than by a second rule here.
pub(super) fn garrison(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    let near: Vec<(u16, u16)> = home_buildings(rules, obs);
    let billets: Vec<_> = obs
        .occupiable_buildings(rules)
        .filter(|billet| within(near.iter().copied(), billet.cell, HOME_REACH_CELLS))
        .collect();
    if billets.is_empty() {
        return;
    }
    let mut issued = 0;
    for entity in obs.own_entities() {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        let Some(ty) = obs.object_type(rules, entity.type_id) else {
            continue;
        };
        if ty.category != ObjectCategory::Infantry
            || !ty.occupier
            || !entity.orderable
            || obs.team_has_member(entity.entity_id)
            || holds_or_fights(entity.mission)
            || ordered_this_frame(out, entity.entity_id)
        {
            continue;
        }
        let Some(billet) = billets
            .iter()
            .filter(|billet| squared_distance(entity.cell, billet.cell) <= HOME_REACH_CELLS.pow(2))
            .filter(|billet| obs.can_garrison(rules, entity.entity_id, billet.building_id))
            .min_by_key(|billet| squared_distance(entity.cell, billet.cell))
        else {
            continue;
        };
        let current = brain
            .orders
            .get(&entity.entity_id)
            .is_some_and(|sent| sent.target == Some(billet.building_id) && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            entity.entity_id,
            SentOrder {
                target: Some(billet.building_id),
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(
            entity.entity_id,
            Goal::Enter {
                target: billet.building_id,
            },
        );
        issued += 1;
    }
}

/// What a spy gets out of a building, in retail's own categories.
///
/// The order follows `AIMD.INI`'s own spy teams — `General Spy vs Soviet
/// Power` / `vs Allied Power` before `vs Allied War` / `vs Soviet War`, and
/// `vs Sov Tech` after them (`ini/aimd.ini:1058-1158`) — and the categories are
/// the rules' `BuildCat=` lists, so a mod that moves a building keeps its
/// weight. A power plant blacks the enemy out, a refinery hands over its ore
/// and a radar its map; a barracks or a war factory makes its army veteran; a
/// lab is worth the walk and little more.
fn infiltration_value(rules: &RuleSet, ty: &ObjectType) -> u8 {
    if rules.build_power_types.iter().any(|name| *name == ty.id) {
        4
    } else if ty.resource_destination || ty.radar {
        3
    } else if rules.build_weapons_types.iter().any(|name| *name == ty.id)
        || rules.build_barracks_types.iter().any(|name| *name == ty.id)
    {
        2
    } else {
        1
    }
}

/// The cells of the house's own buildings: the yard, the plants, the factories,
/// the defenses. A bridge or a garrison is only worth a unit near one of them.
fn home_buildings(rules: &RuleSet, obs: &Observation<'_>) -> Vec<(u16, u16)> {
    obs.own_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| ty.category == ObjectCategory::Building)
        })
        .map(|entity| entity.cell)
        .collect()
}

/// Whether any of `cells` is within `reach` cells (Chebyshev-independent, all
/// orders in this file use squared Euclidean distance) of `cell`.
fn within(cells: impl Iterator<Item = (u16, u16)>, cell: (u16, u16), reach: i32) -> bool {
    cells
        .into_iter()
        .any(|other| squared_distance(cell, other) <= reach * reach)
}

/// The nearest of `(id, cell)` sites, ties broken by id so the answer does not
/// depend on the order the entity store happens to yield.
fn nearest_site(sites: &[(u64, (u16, u16))], from: (u16, u16)) -> Option<u64> {
    sites
        .iter()
        .min_by_key(|(id, cell)| (squared_distance(from, *cell), *id))
        .map(|(id, _)| *id)
}

/// Squared cell distance: comparisons only, so the square root is not needed.
fn squared_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    let dx = i32::from(a.0) - i32::from(b.0);
    let dy = i32::from(a.1) - i32::from(b.1);
    dx * dx + dy * dy
}

/// Whether an earlier step already gave this unit an order this frame. The
/// steps run in priority order — a bridge before a capture, a capture before a
/// guard — and a unit is one unit: the first answer wins.
fn ordered_this_frame(out: &Outbox, actor: u64) -> bool {
    out.orders.iter().any(|order| order.actor == actor)
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
        sim.spawn_object("NAPOWR", "Russians", 20, 20, 64, &rules)
            .expect("a power plant a spy may enter");
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

    /// A spy takes what is worth the walk, and only what a spy may enter: the
    /// power plant over the refinery beside it, and neither over a derrick the
    /// click would have refused.
    #[test]
    fn a_spy_targets_the_building_worth_the_most() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let spy = sim
            .spawn_object("SPY", "Americans", 8, 8, 64, &rules)
            .expect("a spy");
        // Nearest of all, and `Capturable=`, but not `Spyable=`.
        let derrick = sim
            .spawn_object("CAOILD", "Russians", 10, 10, 64, &rules)
            .expect("a derrick");
        let refinery = sim
            .spawn_object("NAREFN", "Russians", 12, 12, 64, &rules)
            .expect("a refinery");
        let power = sim
            .spawn_object("NAPOWR", "Russians", 22, 22, 64, &rules)
            .expect("a power plant");

        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        let mut out = Outbox::default();
        captures(&mut brain, &rules, &obs, &mut out);

        let target = out
            .orders
            .iter()
            .find(|order| order.actor == spy)
            .and_then(|order| match order.goal {
                Goal::Capture { target } => Some(target),
                _ => None,
            })
            .expect("the spy is given a target");
        assert_eq!(
            target,
            power,
            "the power plant blacks the enemy out; the derrick and the refinery are nearer"
        );
        assert_ne!(target, derrick);
        assert_ne!(target, refinery);
    }

    /// `Occupier=` infantry garrison the house's own bunker, through the same
    /// Enter click.
    #[test]
    fn riflemen_garrison_a_bunker_of_their_own() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let gi = sim
            .spawn_object("E1", "Americans", 10, 12, 64, &rules)
            .expect("a GI");
        let bunker = sim
            .spawn_object("NABNKR", "Americans", 12, 12, 64, &rules)
            .expect("a battle bunker");

        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        let mut out = Outbox::default();
        garrison(&mut brain, &rules, &obs, &mut out);

        assert!(
            out.orders
                .iter()
                .any(|order| order.actor == gi
                    && matches!(order.goal, Goal::Enter { target } if target == bunker)),
            "the rifleman takes the bunker: {:?}",
            out.orders
        );
    }
}
