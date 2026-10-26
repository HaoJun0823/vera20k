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
use crate::rules::superweapon_type::SuperWeaponKind;
use crate::sim::control::{Goal, MicroKind, MicroParams};
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::command::Command;
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
    if !brain
        .census
        .units
        .iter()
        .any(|unit| walks_into_buildings_flags(&unit.traits))
    {
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
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        if !unit.orderable || unit.hand || ordered_this_frame(out, unit.entity_id) {
            continue;
        }
        let Some(ty) = obs.object_type(rules, unit.type_id) else {
            continue;
        };
        if !walks_into_buildings(ty) {
            continue;
        }
        // A spy takes what is worth most; anyone else takes what is nearest.
        let chosen = if ty.agent {
            spy_targets
                .iter()
                .max_by_key(|(target, value)| (*value, -squared_distance(unit.cell, target.cell)))
                .map(|(target, _)| *target)
        } else {
            nearest_enemy(&engineer_targets, unit.cell)
        };
        let Some(target) = chosen else {
            continue;
        };
        let current = brain
            .orders
            .get(&unit.entity_id)
            .is_some_and(|sent| sent.target == Some(target.entity_id) && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            unit.entity_id,
            SentOrder {
                target: Some(target.entity_id),
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(
            unit.entity_id,
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
    if !brain.census.units.iter().any(|unit| unit.traits.engineer) {
        return;
    }
    let huts: Vec<(u64, (u16, u16))> = obs
        .bridge_repair_huts(rules)
        .filter(|site| within(home_cells(brain), site.cell, HOME_REACH_CELLS))
        .map(|site| (site.building_id, site.cell))
        .collect();
    if huts.is_empty() {
        return;
    }
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        let Some(ty) = obs.object_type(rules, unit.type_id) else {
            continue;
        };
        if !ty.engineer
            || !unit.orderable
            || unit.hand
            || ordered_this_frame(out, unit.entity_id)
            || holds_or_fights(unit.mission)
        {
            continue;
        }
        let Some(target) = nearest_site(&huts, unit.cell) else {
            continue;
        };
        let current = brain
            .orders
            .get(&unit.entity_id)
            .is_some_and(|sent| sent.target == Some(target) && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            unit.entity_id,
            SentOrder {
                target: Some(target),
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(unit.entity_id, Goal::Capture { target });
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
    let billets: Vec<_> = obs
        .occupiable_buildings(rules)
        .filter(|billet| within(home_cells(brain), billet.cell, HOME_REACH_CELLS))
        .collect();
    if billets.is_empty() {
        return;
    }
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        if !unit.traits.occupier
            || !unit.orderable
            || unit.hand
            || holds_or_fights(unit.mission)
            || ordered_this_frame(out, unit.entity_id)
        {
            continue;
        }
        let Some(billet) = billets
            .iter()
            .filter(|billet| squared_distance(unit.cell, billet.cell) <= HOME_REACH_CELLS.pow(2))
            .filter(|billet| obs.can_garrison(rules, unit.entity_id, billet.building_id))
            .min_by_key(|billet| squared_distance(unit.cell, billet.cell))
        else {
            continue;
        };
        let current = brain
            .orders
            .get(&unit.entity_id)
            .is_some_and(|sent| sent.target == Some(billet.building_id) && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            unit.entity_id,
            SentOrder {
                target: Some(billet.building_id),
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(
            unit.entity_id,
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
fn home_cells(brain: &RetailBrain) -> impl Iterator<Item = (u16, u16)> + '_ {
    brain.census.home_cells.iter().copied()
}

/// Whether a unit's type is one the Capture click reaches: an engineer's by
/// `Engineer=` and a spy's by `Infiltrate=`.
fn walks_into_buildings_flags(traits: &super::census::Traits) -> bool {
    traits.engineer || traits.infiltrator
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
    _rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    // Forget the units the house no longer has, so the memory stays bounded by
    // the army and a recycled id cannot inherit an old order.
    let ids: BTreeSet<u64> = brain.census.units.iter().map(|unit| unit.entity_id).collect();
    brain.orders.retain(|id, _| ids.contains(id));
    if !brain.census.units.iter().any(|unit| unit.traits.combat) {
        return;
    }
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        if !unit.traits.combat {
            continue;
        }
        // A team's script or one of the player's control groups owns its
        // members; this step is for the units no hand has taken.
        if unit.hand {
            continue;
        }
        // A unit that is already holding or fighting is doing exactly this, and
        // its mission owns it: a second Guard would cancel the fight.
        if holds_or_fights(unit.mission) {
            continue;
        }
        // The advance step owns a unit it is pushing: a guard here would pull
        // it back out of the push.
        if brain
            .orders
            .get(&unit.entity_id)
            .is_some_and(|sent| sent.target.is_some() && tick < sent.refresh)
        {
            continue;
        }
        let current = brain
            .orders
            .get(&unit.entity_id)
            .is_some_and(|sent| sent.target.is_none() && tick < sent.refresh);
        if current {
            continue;
        }
        brain.orders.insert(
            unit.entity_id,
            SentOrder {
                target: None,
                refresh: tick.saturating_add(ORDER_REFRESH_FRAMES),
            },
        );
        out.order(unit.entity_id, Goal::Guard);
        issued += 1;
    }
}

/// How near a hostile has to be before a soldier takes its deployed position,
/// in cells. Deploying is a weapon change and a movement stop — `DeployFire=`
/// swaps the primary for the deployed one — so it is worth taking when the
/// fight has arrived, not when it is a rumour across the map.
const DEPLOY_RANGE_CELLS: i32 = 6;

/// How many toggles one frame carries. The D key is a click like any other,
/// and a frame that spends its budget on soldiers standing up has none left
/// for the orders that move them.
const DEPLOYS_PER_FRAME: usize = 4;

/// The D key on the soldiers that fight better deployed: `DeployFire=yes`.
///
/// `[E1]` and `[GGI]` carry it with `Deployer=yes`, and what it buys is the
/// deployed weapon — a GI's sandbag posture, a Guardian GI's missiles — at the
/// price of not moving. A player presses D when the enemy is in front of the
/// soldier, which is what the range above is.
///
/// The command toggles (`None → Deploying`, `Deployed → Undeploying`, and a
/// no-op mid-transition), so it is only issued on the edge: a soldier that is
/// already down stays down, and one that is standing stays up. Issuing it
/// every frame would walk them in and out of the pose, and a deployed soldier
/// that never stands up is a soldier that never takes ground.
pub(super) fn deploy(brain: &RetailBrain, obs: &Observation<'_>, out: &mut Outbox) {
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= DEPLOYS_PER_FRAME {
            break;
        }
        if !unit.traits.deployer || unit.hand || !unit.orderable {
            continue;
        }
        let engaged = brain.census.enemies.iter().any(|enemy| {
            let dx = i32::from(enemy.cell.0) - i32::from(unit.cell.0);
            let dy = i32::from(enemy.cell.1) - i32::from(unit.cell.1);
            dx * dx + dy * dy <= DEPLOY_RANGE_CELLS * DEPLOY_RANGE_CELLS
        });
        if engaged == obs.is_deployed(unit.entity_id) {
            continue;
        }
        out.command(Command::ToggleInfantryDeploy {
            entity_id: unit.entity_id,
        });
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

/// The health fraction at which a fighting unit belongs to the retreat routine
/// rather than to the brain's own orders.
///
/// A unit the sim's own mission owns fights to the death, which is the wrong
/// trade for a house that pays for every replacement: a third of its strength
/// left is the point where a player walks it home. The routine hands the unit
/// back as soon as it is healthy again, so the threshold is the whole rule.
const RETREAT_HEALTH_PERMILLE: u16 = 300;

/// Hand the house's hurt fighters to the L1 retreat routine.
///
/// This is the one place the brain uses a managed routine. It asks through
/// `Outbox::attach` because the table of attached objects is L2's, and from
/// then on the routine — not this brain — is that object's issuer. Asking
/// again for a unit the table already holds is free: `MicroTable::attach`
/// refuses a second attach, so no bookkeeping is kept here and a routine that
/// hands an object back is picked up again the next frame its condition holds.
///
/// The post is the base's own anchor, so a unit that breaks off walks home
/// rather than freezing where the fight left it.
pub(super) fn managed(brain: &mut RetailBrain, out: &mut Outbox) {
    let post = brain.census.base_cell;
    for unit in &brain.census.units {
        if !unit.traits.combat || unit.hand || !unit.orderable {
            continue;
        }
        if unit.health_permille > RETREAT_HEALTH_PERMILLE {
            continue;
        }
        out.attach(
            unit.entity_id,
            MicroKind::Retreat,
            MicroParams {
                health_floor: RETREAT_HEALTH_PERMILLE,
                standoff_cells: 4,
                radius_cells: 10,
                hold_frames: 15,
            },
            post,
        );
    }
}

/// How long a fired super waits before this brain tries it again, in frames.
///
/// The charge itself is the real gate — a launched super is not ready until it
/// recharges — so this only covers the refusal case, where the click did not
/// take (no warhead for the type, the house's wallet for a paid launch) and the
/// button stays lit. Ten seconds: long enough not to spend the frame budget on
/// a refused command, short enough to catch a refusal that clears.
const SUPER_RETRY_FRAMES: u64 = 600;

/// Fire the house's charged supers at the enemy, the way a player clicks the
/// button and then the target.
///
/// Native has this decision (`AI_TryFireSW @ 0x005098F0`) but gates it on the
/// house not being human (`is_controlled_by_human`), and a brain-driven house
/// is a human's: without this step it never fires one. The decision is the
/// native one's in shape — for each charged super, in the rules' order, find a
/// target — with the target read off the ledger instead of the native value
/// table: the enemy's Construction Yard when one stands, otherwise the first
/// enemy structure.
///
/// Only powers aimed at an enemy cell are fired. The Iron Curtain, the Force
/// Shield and the Chrono pair are spent on the house's *own* units and
/// buildings, and firing them at the enemy burns the charge for nothing; they
/// belong to a defensive decision this brain does not make yet.
pub(super) fn supers(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    if !obs.super_weapons() {
        return;
    }
    let ready = obs.ready_super_weapons(rules);
    if ready.is_empty() {
        return;
    }
    let Some(target) = enemy_structure_cell(brain, rules, obs) else {
        return;
    };
    let tick = obs.tick();
    for name in &rules.super_weapon_order {
        let Some(type_id) = obs.type_id(name) else {
            continue;
        };
        if !ready.contains(&type_id) {
            continue;
        }
        if brain
            .super_at
            .get(&type_id)
            .is_some_and(|at| tick < *at)
        {
            continue;
        }
        let Some(super_weapon) = rules.super_weapon(name) else {
            continue;
        };
        if !spent_on_the_enemy(super_weapon.kind) {
            continue;
        }
        log::info!("auto play: fire {name} at {target:?}");
        brain
            .super_at
            .insert(type_id, tick.saturating_add(SUPER_RETRY_FRAMES));
        out.intent(Intent::LaunchSuperWeapon {
            sw_type_id: type_id,
            cell: target,
        });
    }
}

/// Whether a power is aimed at an enemy cell rather than held for the house's
/// own force.
fn spent_on_the_enemy(kind: SuperWeaponKind) -> bool {
    match kind {
        SuperWeaponKind::MultiMissile
        | SuperWeaponKind::LightningStorm
        | SuperWeaponKind::ParaDrop
        | SuperWeaponKind::AmerParaDrop
        | SuperWeaponKind::SpyPlane
        | SuperWeaponKind::PsychicReveal
        | SuperWeaponKind::PsychicDominator
        | SuperWeaponKind::GeneticConverter
        | SuperWeaponKind::GenericWarhead
        | SuperWeaponKind::UnitDelivery => true,
        // Spent on the house's own units and buildings, or on nothing.
        SuperWeaponKind::IronCurtain
        | SuperWeaponKind::ChronoSphere
        | SuperWeaponKind::ChronoWarp
        | SuperWeaponKind::ForceShield
        | SuperWeaponKind::NoEffect => false,
    }
}

/// The cell a super is fired at: the enemy Construction Yard when one stands,
/// otherwise the first enemy structure in storage order.
///
/// The yard is native's own most-valued target (`AI_FindBestRallyTarget`'s
/// value table puts a Construction Yard at 100 against a power plant's 60 and a
/// tank's 2), and it is the cell a single missile decides a match at.
fn enemy_structure_cell(
    brain: &RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<(u16, u16)> {
    let mut first = None;
    for enemy in &brain.census.enemies {
        let Some(ty) = obs.object_type(rules, enemy.type_id) else {
            continue;
        };
        if ty.category != ObjectCategory::Building {
            continue;
        }
        if ty.build_const_eligible {
            return Some(enemy.cell);
        }
        first.get_or_insert(enemy.cell);
    }
    first
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

    /// A snapshot arena for the steps that do not read the scenario's own
    /// layering: the repository's own `ini/`, so this holds without `RA2_DIR`.
    fn snapshot(seed: u64) -> Option<(Simulation, RuleSet)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    /// A `DeployFire=` soldier takes its deployed position when a hostile
    /// reaches it. The D key is a toggle, so the step only answers on the edge:
    /// a soldier standing in an empty field is not sent down, and one that has
    /// taken the pose is not walked back out of it.
    #[test]
    fn a_soldier_deploys_when_the_fight_reaches_it() {
        let Some((mut sim, rules)) = snapshot(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("E1", "Americans", 20, 20, 255, &rules)
            .expect("a GI");
        // Interned before the projection borrows the world: the ledger's
        // enemies are named by id, and the interner is the world's.
        let hostile = sim.interner.intern("E2");
        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        brain.census.refresh(
            &crate::rules::unit_role::RoleTable::build(&rules),
            &rules,
            &obs,
        );
        let Some(gi) = brain.census.units.first().map(|unit| unit.entity_id) else {
            return;
        };

        // Nothing in front of it: an empty field is not a fight.
        let mut out = Outbox::default();
        deploy(&brain, &obs, &mut out);
        assert!(
            out.commands.is_empty(),
            "no hostile, no D key: {:?}",
            out.commands
        );

        // A hostile three cells away is.
        brain.census.enemies.push(EnemyEntity {
            entity_id: gi.wrapping_add(1),
            type_id: hostile,
            cell: (23, 20),
        });
        let mut out = Outbox::default();
        deploy(&brain, &obs, &mut out);
        assert!(
            out.commands.iter().any(|command| matches!(
                command,
                Command::ToggleInfantryDeploy { entity_id } if *entity_id == gi
            )),
            "the soldier takes the deployed position: {:?}",
            out.commands
        );
    }

    /// The retail rival arena, needed to field real object types. `None`
    /// without `RA2_DIR`, as the rest of the suite does: these orders read the
    /// scenario's own layering, which the unlayered snapshot does not carry.
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
        // Above `ConditionRed`: a bunker at red health admits nobody, the same
        // rule the sim's own garrison ask enforces.
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

    /// A charged power is fired at the enemy's Construction Yard. Native has
    /// this decision (`AI_TryFireSW`), but it is gated on the house not being
    /// human — and a brain-driven house is a human's, so without this step the
    /// button is never pressed.
    #[test]
    fn a_charged_super_is_fired_at_the_enemy_yard() {
        use crate::sim::superweapon::SuperWeaponInstance;

        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("NACNST", "Russians", 24, 24, 255, &rules)
            .expect("an enemy Construction Yard");
        let Some(name) = rules
            .super_weapon_order
            .iter()
            .find(|name| {
                rules
                    .super_weapon(name)
                    .is_some_and(|sw| spent_on_the_enemy(sw.kind))
            })
            .cloned()
        else {
            return;
        };
        let type_id = sim.interner.intern(&name);
        let mut instance = SuperWeaponInstance::new(type_id, owner, 0);
        instance.set_is_active_for_test(true);
        instance.set_is_ready_for_test(true);
        instance.set_charge_start_tick_for_test(0);
        instance.set_charge_duration_for_test(9_000);
        sim.super_weapons_mut_for_test()
            .entry(owner)
            .or_default()
            .insert(type_id, instance);

        let obs = Observation::new(&sim, owner);
        assert!(obs.super_weapons(), "the arena plays with powers on");
        assert_eq!(
            obs.ready_super_weapons(&rules),
            vec![type_id],
            "the charged power is the one live button"
        );

        let mut brain = RetailBrain::new(0xA17);
        brain.census.refresh(
            &crate::rules::unit_role::RoleTable::build(&rules),
            &rules,
            &obs,
        );
        let mut out = Outbox::default();
        supers(&mut brain, &rules, &obs, &mut out);

        assert_eq!(out.intents.len(), 1, "{:?}", out.intents);
        match out.intents[0] {
            Intent::LaunchSuperWeapon { sw_type_id, cell } => {
                assert_eq!(sw_type_id, type_id);
                assert_eq!(cell, (24, 24), "fired at the enemy yard");
            }
            ref other => panic!("a launch, not {other:?}"),
        }
        // The same frame again: the cooldown keeps a refused or recharging
        // button from being clicked every frame.
        let mut again = Outbox::default();
        supers(&mut brain, &rules, &obs, &mut again);
        assert!(again.intents.is_empty(), "{:?}", again.intents);
    }

    /// The Iron Curtain and the Force Shield are spent on the house's own
    /// force. Firing them at the enemy burns the charge for nothing.
    #[test]
    fn a_power_held_for_our_own_force_is_not_fired_at_the_enemy() {
        assert!(!spent_on_the_enemy(SuperWeaponKind::IronCurtain));
        assert!(!spent_on_the_enemy(SuperWeaponKind::ForceShield));
        assert!(!spent_on_the_enemy(SuperWeaponKind::ChronoSphere));
        assert!(spent_on_the_enemy(SuperWeaponKind::MultiMissile));
        assert!(spent_on_the_enemy(SuperWeaponKind::LightningStorm));
    }
}
