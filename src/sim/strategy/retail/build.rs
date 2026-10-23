//! The brain's own economy: what the base builds, what the factories make,
//! and in what order.
//!
//! This is **not** the retail AI's economy. `AI_EconomyStateMachine` and
//! `AI_Manage_Build_Queue` (`sim::ai_base_building`) run their credit cutoff
//! and their team accounting on `HouseClass` fields a human house never
//! fills, and a computer house is allowed money it has not earned. A house
//! this brain drives is a player's house: it earns what its harvesters
//! deliver and it pays what production charges. So the economy here is a
//! budget over the same two answers a player reads — the price
//! ([`Observation::cost_of`]) and whether the house may build it at all
//! ([`Observation::can_build`]) — and nothing is queued the wallet cannot
//! cover.
//!
//! The order is a player's, not a plan's: keep the lights on, then the
//! income, then the factories, then the army. Building types come from the
//! `[AI]` lists in the rules (`BuildPower=`, `BuildRefinery=`,
//! `BuildBarracks=`, `BuildWeapons=`, `BuildRadar=`, `BuildTech=`), and the
//! fighting types are the house's own buildable ones, dearest first, so a
//! mod's rules change what this builds without touching VERA.
//!
//! Determinism: every count and every choice walks the rules' arrays and the
//! entity store in their own order, and every test is an integer comparison.

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::control::placement::{PLACEMENT_ATTEMPTS, Placement};
use crate::sim::intern::InternedId;
use crate::sim::production::{CanBuild, ProductionCategory};

use super::RetailBrain;

/// How many harvesters one refinery keeps busy. Three, not the rules'
/// `HarvestersPerRefinery=`: a player runs a queue at the refinery, because
/// the trip out and back is most of a harvester's life and two of them idle
/// the bay between deliveries.
const HARVESTERS_PER_REFINERY: usize = 2;

/// How many refineries the economy grows to. Past this the ore within reach
/// of the base is the limit, not the unloading bays. A refinery also brings
/// its own harvester, so each one is income twice over.
const REFINERY_TARGET: usize = 3;

/// The wallet the house wants in hand before it buys troops. Below it every
/// credit buys another harvester or another bay, which is what makes the
/// army affordable later; an army bought out of a thin economy is an army
/// that stops the income it was bought with.
const COMBAT_MIN_CREDITS: i32 = 2_000;

/// How many base defenses the house keeps standing.
///
/// The opponent pushes with numbers a player's opening army cannot match, and
/// a base with no tower is a base whose production one raid stops for good:
/// the yard goes dark, the factories follow, and the match is lost by
/// logistics rather than by the fight.
const DEFENSE_TARGET: usize = 2;

/// The wallet a radar or a tech centre has to leave behind: both unlock
/// better types rather than more of them, so they come after the army.
const TECH_MIN_CREDITS: i32 = 4_000;

/// How many fighting units the house fields before it buys a radar.
const ARMY_FOR_TECH: usize = 8;

/// How many war factories the house runs. One factory replaces losses; two
/// grow an army.
const WAR_FACTORY_TARGET: usize = 2;

/// One frame's spending: take what is finished out of the yard, start the
/// next structure, and keep the factories busy.
pub(super) fn spend(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    place_finished(brain, rules, obs, out);
    build_next(rules, obs, out);
    make_units(rules, obs, out);
}

/// Place the structures the yard has finished: the click a player makes on
/// the build area after the cameo turns done.
///
/// A finished building waits for a cell, and until it gets one it holds its
/// category — the yard starts nothing else. The attempt counter is what moves
/// the sweep past a cell the gate admits but the receiver refuses.
fn place_finished(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let mut waiting: Vec<InternedId> = Vec::new();
    for ready in obs.ready_buildings() {
        waiting.push(ready.type_id);
        if !out.intents.is_empty() {
            continue;
        }
        let attempt = brain.site_attempts.get(&ready.type_id).copied().unwrap_or(0);
        let Some(cell) = Placement::new(obs, rules).cell_for(ready.type_id, attempt) else {
            continue;
        };
        out.intent(Intent::PlaceBuilding {
            type_id: ready.type_id,
            cell,
        });
        let next = (attempt + 1) % PLACEMENT_ATTEMPTS;
        brain.site_attempts.insert(ready.type_id, next);
    }
    brain.site_attempts.retain(|id, _| waiting.contains(id));
}

/// The wallet the house keeps for the harvesters it has not built yet.
///
/// Ore delivered is the only income a player's house has, so spending the
/// last of the wallet on something that earns nothing is how a house
/// starves: it ends with a full base and no money to run it. Anything that
/// is not power, income or the factory that makes harvesters waits until the
/// harvesters are paid for.
fn harvester_reserve(rules: &RuleSet, obs: &Observation<'_>) -> i32 {
    if gatherers(obs, rules) >= HARVESTERS_PER_REFINERY * refineries(obs, rules).max(1) {
        return 0;
    }
    harvester(rules, obs).map_or(0, |(ty, _)| obs.cost_of(rules, ty))
}

/// Whether the type earns its keep: power, income, or the factory the
/// harvesters come out of.
fn earns_its_keep(rules: &RuleSet, ty: &ObjectType) -> bool {
    [
        &rules.build_power_types,
        &rules.build_refinery_types,
        &rules.build_weapons_types,
    ]
    .iter()
    .any(|names| names.iter().any(|name| name == &ty.id))
}

/// Start the one structure the base needs most, while the yard is free.
fn build_next(rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
    if obs.building_in_production().is_some() {
        return;
    }
    let Some((ty, type_id)) = next_building(rules, obs) else {
        return;
    };
    let price = obs.cost_of(rules, ty);
    if obs.credits() < price {
        return;
    }
    // While the harvesters are still short, only what earns or powers waits
    // for nothing else: a radar bought before the second harvester is the
    // money the second harvester needed.
    if !earns_its_keep(rules, ty) && harvester_reserve(rules, obs) > 0 {
        return;
    }
    out.intent(Intent::QueueProduction { type_id });
}

/// The structure to start now, dearest need first.
///
/// Power is spliced in ahead of the choice, the way a player does it: a
/// structure that would take the house past its output is not started until
/// a plant stands. Native does the same splice in `AI_Choose_Building`
/// (`0x004FE6F6..0x004FE943`); here it is the house's own balance and the
/// type's own `Power=`/`ExtraPower=` figure, both read from their owners.
///
/// Each entry asks the rules' own list, so a side that has no such type — or
/// a house that cannot build it yet — simply falls through.
fn next_building<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
) -> Option<(&'r ObjectType, InternedId)> {
    // A brownout is the one state that stops everything else: production
    // runs at a crawl and the defenses go offline. Whatever else the base
    // wants, a plant comes first, and it comes first even when nothing else
    // is left to build.
    if obs.power_balance() < 0 {
        if let Some(plant) = first_buildable(rules, obs, &rules.build_power_types) {
            return Some(plant);
        }
    }
    let (ty, type_id) = match candidate(rules, obs) {
        Some(found) => found,
        // Nothing the base wants is buildable yet, and the house has no plant:
        // `Power=` plants are the prerequisite most structures hang off in the
        // retail rules, so a yard that can only make one is a yard that needs
        // a plant. With a plant already standing there is nothing to add — the
        // house waits for whatever it is missing instead of building blindly.
        None if count_in(obs, rules, &rules.build_power_types) == 0 => {
            first_buildable(rules, obs, &rules.build_power_types)?
        }
        None => return None,
    };
    // `Power=` is the type's net contribution: negative is what it draws.
    if obs.power_balance() + ty.power.min(0) < 0 {
        if let Some(plant) = first_buildable(rules, obs, &rules.build_power_types) {
            return Some(plant);
        }
    }
    Some((ty, type_id))
}

/// The structure the base is missing most, ignoring power: income first,
/// then the two factories, then what opens the higher tech.
fn candidate<'r>(rules: &'r RuleSet, obs: &Observation<'_>) -> Option<(&'r ObjectType, InternedId)> {
    let refineries = count_in(obs, rules, &rules.build_refinery_types);
    if refineries < 1 || (refineries < REFINERY_TARGET && gatherers(obs, rules) >= 2 * refineries) {
        if let Some(found) = first_buildable(rules, obs, &rules.build_refinery_types) {
            return Some(found);
        }
    }
    // The war factory before the barracks: it is what makes harvesters, and
    // ore delivered is the only income a player's house has. A base that
    // builds its barracks first pays for soldiers out of its starting wallet
    // and then has nothing left to pay for anything.
    // Two war factories, not one: the second doubles what the house can put
    // on the field per minute, which is the whole difference between an army
    // that replaces itself and one that shrinks with every fight.
    if count_in(obs, rules, &rules.build_weapons_types) < WAR_FACTORY_TARGET {
        if let Some(found) = first_buildable(rules, obs, &rules.build_weapons_types) {
            return Some(found);
        }
    }
    if let Some(found) = wanted_once(rules, obs, &rules.build_barracks_types) {
        return Some(found);
    }
    // Then something to hold the base with, before anything that only makes
    // it grow: a radar is worth less than a base that survives the first
    // raid.
    if can_replace_units(rules, obs) && count_in(obs, rules, base_defenses(rules, obs)) < DEFENSE_TARGET
    {
        if let Some(found) = first_buildable(rules, obs, base_defenses(rules, obs)) {
            return Some(found);
        }
    }
    // Radar and the tech centre unlock better types, not more of them, and
    // they draw power the factories need. They wait for an army to unlock
    // them for: a house that buys a radar before it can field eight fighting
    // units has bought a map it has nothing to put on.
    if obs.credits() >= TECH_MIN_CREDITS && fighter_count(rules, obs) >= ARMY_FOR_TECH {
        if let Some(found) = wanted_once(rules, obs, &rules.build_radar_types) {
            return Some(found);
        }
        return wanted_once(rules, obs, &rules.build_tech_types);
    }
    None
}

/// The house's fighting units: what the army is made of, harvesters and
/// engineers aside.
fn fighter_count(rules: &RuleSet, obs: &Observation<'_>) -> usize {
    obs.own_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id).is_some_and(|ty| {
                ty.weapon_count > 0
                    && ty.category != ObjectCategory::Building
                    && !ty.resource_gatherer
                    && !ty.engineer
            })
        })
        .count()
}

/// The type from `names` the house may build now, in list order.
///
/// `can_build` is the production path's own test, so a type of another side,
/// one whose prerequisite is missing or one beyond the house's tech level
/// answers No here exactly as it would in the sidebar.
fn first_buildable<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    names: &[String],
) -> Option<(&'r ObjectType, InternedId)> {
    names.iter().find_map(|name| {
        let ty = rules.object(name)?;
        let type_id = obs.type_id(name)?;
        (obs.can_build(rules, ty, true) != CanBuild::No).then_some((ty, type_id))
    })
}

/// The type from `names` the house does not own yet: a barracks or a war
/// factory is worth building once, and a second one is worth building only
/// after the army is fed.
fn wanted_once<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    names: &[String],
) -> Option<(&'r ObjectType, InternedId)> {
    if count_in(obs, rules, names) > 0 {
        return None;
    }
    first_buildable(rules, obs, names)
}

/// Keep each free factory on one product: harvesters while the ore is
/// waiting, then the best fighting type the house can pay for.
fn make_units(rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
    if obs.factory_product(ProductionCategory::Vehicle).is_none() {
        let wanted = if gatherers(obs, rules)
            < HARVESTERS_PER_REFINERY * refineries(obs, rules).max(1)
        {
            harvester(rules, obs)
        } else if obs.credits() >= COMBAT_MIN_CREDITS {
            best_fighter(rules, obs, ObjectCategory::Vehicle)
        } else {
            None
        };
        queue_unit(rules, obs, out, wanted);
    }
    if obs.factory_product(ProductionCategory::Infantry).is_none() && obs.credits() >= COMBAT_MIN_CREDITS
    {
        let wanted = best_fighter(rules, obs, ObjectCategory::Infantry);
        queue_unit(rules, obs, out, wanted);
    }
}

fn queue_unit(
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
    wanted: Option<(&ObjectType, InternedId)>,
) {
    let Some((ty, type_id)) = wanted else {
        return;
    };
    if obs.credits() < obs.cost_of(rules, ty) {
        return;
    }
    out.intent(Intent::QueueProduction { type_id });
}

/// The house's own harvester: the first `ResourceGatherer=` Vehicle it may
/// build. Ore delivered is the whole income, so this one is not a choice
/// among equals, it is what pays for the rest.
fn harvester<'r>(rules: &'r RuleSet, obs: &Observation<'_>) -> Option<(&'r ObjectType, InternedId)> {
    class_types(rules, obs, ObjectCategory::Vehicle).find(|(ty, _)| ty.resource_gatherer)
}

/// The dearest fighting type of the class the house may build: a type with a
/// weapon that is not a harvester. Price is the only ranking — what a house
/// buys with a full wallet is what it would rather have — and it is the
/// house's own price, so a country's cost multiplier cannot invert it.
fn best_fighter<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    category: ObjectCategory,
) -> Option<(&'r ObjectType, InternedId)> {
    class_types(rules, obs, category)
        .filter(|(ty, _)| ty.weapon_count > 0 && !ty.resource_gatherer)
        .max_by_key(|(ty, _)| obs.cost_of(rules, ty))
}

/// Every type of `category` the house may build now, in the rules' array
/// order, with its id.
fn class_types<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    category: ObjectCategory,
) -> impl Iterator<Item = (&'r ObjectType, InternedId)> {
    let count = rules.type_array_ids(category).len();
    (0..count).filter_map(move |index| {
        let ty = rules.type_array_at(category, i32::try_from(index).ok()?)?;
        let type_id = obs.type_id(&ty.id)?;
        (obs.can_build(rules, ty, true) != CanBuild::No).then_some((ty, type_id))
    })
}

/// The `[AI]` base-defense list of the house's own side: what the native
/// computer would answer a raid with, read for the same question here.
fn base_defenses<'r>(rules: &'r RuleSet, obs: &Observation<'_>) -> &'r [String] {
    obs.side_index()
        .map_or(&[], |side| rules.base_defense_types(side))
}

/// Whether the base has anything standing between a raid and its yard.
pub(super) fn base_defended(rules: &RuleSet, obs: &Observation<'_>) -> bool {
    count_in(obs, rules, base_defenses(rules, obs)) > 0
}

/// Whether the base can replace what it loses: a war factory for the
/// vehicles, a barracks for the infantry.
///
/// A push spends the army it was given, and a house with no factory to
/// replace it has traded its whole force for whatever it killed. That is why
/// this is the gate on attacking rather than a bigger army alone.
pub(super) fn can_replace_units(rules: &RuleSet, obs: &Observation<'_>) -> bool {
    count_in(obs, rules, &rules.build_weapons_types) > 0
        && count_in(obs, rules, &rules.build_barracks_types) > 0
}

/// How many of the house's objects are one of `names`.
fn count_in(obs: &Observation<'_>, rules: &RuleSet, names: &[String]) -> usize {
    obs.own_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| names.iter().any(|name| name == &ty.id))
        })
        .count()
}

/// The house's harvesters: the objects whose type is `ResourceGatherer=`.
fn gatherers(obs: &Observation<'_>, rules: &RuleSet) -> usize {
    obs.own_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| ty.resource_gatherer)
        })
        .count()
}

/// The house's refineries: the objects whose type is `ResourceDestination=`.
fn refineries(obs: &Observation<'_>, rules: &RuleSet) -> usize {
    obs.own_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| ty.resource_destination)
        })
        .count()
}
