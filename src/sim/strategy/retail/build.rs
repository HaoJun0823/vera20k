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
use crate::rules::unit_role::{self, ROLES, Role, RoleTable, RoleWeights};
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

/// How many refineries the base is built around. Past this the ore within
/// reach of the base is the limit, not the unloading bays. A refinery also
/// brings its own harvester, so each one is income twice over.
const REFINERY_TARGET: usize = 3;

/// The wallet the house wants in hand before it buys troops. Below it every
/// credit buys another harvester or another bay, which is what makes the
/// army affordable later; an army bought out of a thin economy is an army
/// that stops the income it was bought with.
const COMBAT_MIN_CREDITS: i32 = 2_000;

/// The wallet the infantry line needs: a soldier costs a tenth of a tank, and
/// at the vehicle's floor the barracks never fires at all — the wallet is
/// always somewhere between one tank and the next.
const INFANTRY_MIN_CREDITS: i32 = 500;

/// The price cuts between the bands a unit can fall in: a soldier around 200,
/// a line vehicle around 900, the heavy and long-range types above that.
const CHEAP_UNIT_CREDITS: i32 = 500;
const MID_UNIT_CREDITS: i32 = 1_000;

/// The wallet that decides how dear a band the house reaches for: under the
/// first it buys cheap, past the second it buys the heavy end.
const POOR_CREDITS: i32 = 1_200;
const RICH_CREDITS: i32 = 4_000;

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

/// The wallet a building on the yard maker's `Prerequisite=` list has to
/// leave behind: it buys the ability to rebuild a lost yard, which only pays
/// off once the base can afford to lose one.
const SUPPORT_MIN_CREDITS: i32 = 4_000;

/// How many fighting units the house fields before it buys a radar.
const ARMY_FOR_TECH: usize = 8;

/// How many war factories the house runs. One factory replaces losses; two
/// grow an army.
const WAR_FACTORY_TARGET: usize = 2;

/// The wallet a house has to be holding before it treats a factory as the
/// bottleneck: past this it cannot spend what it earns, so the answer is
/// another factory rather than another harvester.
const SURPLUS_CREDITS: i32 = 12_000;

/// How many war factories and barracks a house with a surplus runs.
const WAR_FACTORY_SURPLUS: usize = 4;
const BARRACKS_SURPLUS: usize = 2;

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
    let roles = brain.roles(rules);
    make_units(rules, obs, roles, out);
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
            log::debug!(
                "auto play: no cell for {} (attempt {attempt})",
                obs.object_type(rules, ready.type_id).map_or("?", |ty| ty.id.as_str())
            );
            continue;
        };
        log::debug!(
            "auto play: place {} at {cell:?} (attempt {attempt})",
            obs.object_type(rules, ready.type_id).map_or("?", |ty| ty.id.as_str())
        );
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
    // A defense is made by the Defense factory, not the Buildings one, so the
    // guard above does not see it: a tower already on the line is queued again
    // on every frame of its build. Removing the repeats changes the match
    // (measured 2026-10-24: timeout at 36000 instead of a win), because the
    // repeated queue is also what keeps the tower count up, so the two have to
    // be re-balanced together — `DEFENSE_TARGET` at 4 did not make up for it.
    if obs.in_production(rules, type_id) {
        log::debug!("queued already holds {}", ty.id);
    }
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
    log::debug!(
        "auto play: build {} credits={} power={} fighters={}",
        ty.id,
        obs.credits(),
        obs.power_balance(),
        fighter_count(rules, obs)
    );
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
    // Money piling up is the house telling us the bottleneck is not income
    // but throughput: a wallet past this cannot be spent by two factories,
    // and a house that banks it has an army it could not build. Every extra
    // factory shortens a build by `[General] MultipleFactory=` (the rules'
    // own factor per extra factory, `Time_To_Build`'s `factory_count`).
    if obs.credits() >= SURPLUS_CREDITS {
        if count_in(obs, rules, &rules.build_weapons_types) < WAR_FACTORY_SURPLUS {
            if let Some(found) = first_buildable(rules, obs, &rules.build_weapons_types) {
                return Some(found);
            }
        }
        if count_in(obs, rules, &rules.build_barracks_types) < BARRACKS_SURPLUS {
            if let Some(found) = first_buildable(rules, obs, &rules.build_barracks_types) {
                return Some(found);
            }
        }
    }
    // Then something to hold the base with, before anything that only makes
    // it grow: a radar is worth less than a base that survives the first
    // raid.
    if can_replace_units(rules, obs)
        && count_in(obs, rules, base_defenses(rules, obs)) < DEFENSE_TARGET
    {
        if let Some(found) = first_buildable(rules, obs, base_defenses(rules, obs)) {
            return Some(found);
        }
    }
    // The way back from a lost yard, bought while the yard still stands: the
    // yard maker's own `Prerequisite=` list is the rules' answer to what has
    // to be there to rebuild it, and nothing else in this order asks for the
    // service depot retail's MCV wants. It waits for a wallet the army has
    // already been paid out of — it is insurance, and insurance that starves
    // the army is the raid it was bought against.
    if obs.credits() >= SUPPORT_MIN_CREDITS {
        if let Some(found) = super::expand::yard_maker_support(rules, obs) {
            return Some(found);
        }
    }
    // Radar and the tech centre unlock better types, not more of them, and
    // they draw power the factories need. They wait for an army to unlock them
    // for: a house that buys a radar before it can field eight fighting units
    // has bought a map it has nothing to put on.
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

/// Keep each free factory on one product: a yard maker when the house has no
/// yard, harvesters while the ore is waiting, then the best fighting type the
/// house can pay for.
///
/// The yard maker comes first because everything else the base builds is
/// gated on it: a house with no yard can still make units, and can never make
/// another structure until an MCV arrives to deploy.
fn make_units(rules: &RuleSet, obs: &Observation<'_>, roles: &RoleTable, out: &mut Outbox) {
    if obs.factory_product(ProductionCategory::Vehicle).is_none() {
        let wanted = vehicle_product(rules, obs, roles);
        queue_unit(rules, obs, out, wanted);
    }
    if obs.factory_product(ProductionCategory::Infantry).is_none()
        && obs.credits() >= INFANTRY_MIN_CREDITS
    {
        let wanted = chosen_product(rules, obs, ObjectCategory::Infantry)
            .or_else(|| next_fighter(rules, obs, roles, ObjectCategory::Infantry));
        queue_unit(rules, obs, out, wanted);
    }
}

/// What the war factory starts next.
///
/// A yard maker comes first while the house has neither a yard nor one on the
/// field — but only when the house may build one: `CanBuild` answers for the
/// maker's own `Prerequisite=` list, and retail's MCV needs a war factory the
/// side still holds. When it cannot, the factory keeps making what it can
/// rather than standing idle, because a house with no yard still fights with
/// the units it produces.
fn vehicle_product<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    roles: &RoleTable,
) -> Option<(&'r ObjectType, InternedId)> {
    if !super::expand::yard_stands(rules, obs)
        && !super::expand::owns_yard_maker(rules, obs)
        && let Some(maker) = super::expand::yard_maker(rules, obs)
    {
        return Some(maker);
    }
    if gatherers(obs, rules) < HARVESTERS_PER_REFINERY * refineries(obs, rules).max(1) {
        return harvester(rules, obs);
    }
    if obs.credits() >= COMBAT_MIN_CREDITS {
        return chosen_product(rules, obs, ObjectCategory::Vehicle)
            .or_else(|| next_fighter(rules, obs, roles, ObjectCategory::Vehicle));
    }
    None
}

/// The type the house's own choice has this factory make, when the house may
/// still build it and pay for it: `Suggest_New_Object 0x004FBD80`.
///
/// This is where the retail team data reaches production. `HouseClass::Update`
/// fills the house's choice from what its forming teams still need
/// (`TeamClass::Get_Needed_Types`, over `AIMD.INI`'s TaskForces), so a team
/// that asks for an engineer, a spy or a transport is answered here — a
/// ranking over combat weights cannot ask for those at all.
///
/// Two guards, both the caller's, because the native reader is unchecked: the
/// type must still be buildable (a choice can outlive its factory or its
/// prerequisite) and affordable. A harvester is skipped too — the harvester
/// count above is the economy's own owner, and the slot can still name one
/// after the count is met.
fn chosen_product<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    category: ObjectCategory,
) -> Option<(&'r ObjectType, InternedId)> {
    let (ty, type_id) = obs.chosen_product(rules, category)?;
    if ty.resource_gatherer || obs.can_build(rules, ty, true) == CanBuild::No {
        return None;
    }
    Some((ty, type_id))
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
    log::debug!(
        "auto play: unit {} at {} (credits {})",
        ty.id,
        obs.cost_of(rules, ty),
        obs.credits()
    );
    out.intent(Intent::QueueProduction { type_id });
}

/// The house's own harvester: the first `ResourceGatherer=` Vehicle it may
/// build. Ore delivered is the whole income, so this one is not a choice
/// among equals, it is what pays for the rest.
fn harvester<'r>(rules: &'r RuleSet, obs: &Observation<'_>) -> Option<(&'r ObjectType, InternedId)> {
    class_types(rules, obs, ObjectCategory::Vehicle).find(|(ty, _)| ty.resource_gatherer)
}

/// The fighting type of `category` the house should make next: first the one
/// that answers what the army is short of, and only when it is short of
/// nothing, the price band the wallet can afford.
///
/// "Always the dearest one it can build" is what a house with a full wallet
/// looks like from the outside, and in practice it means one type forever —
/// prism tanks until the credits run out, and an infantry line that never
/// fires, because the dearest thing is never a soldier. What decides instead
/// is the gap between what the other side fields and what this house can
/// already answer ([`shortfall`]): a wall of conscripts is answered with the
/// types that bite infantry, however little they cost, and a base that is
/// already covered buys by price.
///
/// Price is still the gate on how dear a type may be, and still decides when
/// the army wants for nothing: a house under [`POOR_CREDITS`] buys cheap and
/// buys it now, a house past [`RICH_CREDITS`] buys the heavy end, and the same
/// base therefore buys different things depending on whether the ore is coming
/// in.
fn next_fighter<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    roles: &RoleTable,
    category: ObjectCategory,
) -> Option<(&'r ObjectType, InternedId)> {
    let fare: Vec<_> = class_types(rules, obs, category)
        .filter(|(ty, _)| ty.weapon_count > 0 && !ty.resource_gatherer)
        .collect();
    let ceiling = unit_ceiling(obs.credits());
    let affordable: Vec<_> = fare
        .iter()
        .copied()
        .filter(|(ty, _)| obs.cost_of(rules, ty) <= ceiling)
        .collect();
    if affordable.is_empty() {
        return None;
    }
    if let Some(need) = shortfall(rules, obs, roles) {
        let answer = |ty: &ObjectType| {
            (
                roles.weights(&ty.id).answer(need),
                f64::from(obs.cost_of(rules, ty)),
            )
        };
        let mut best: Option<(&ObjectType, InternedId)> = None;
        for candidate in affordable.iter().copied() {
            // The dearest only breaks a tie on the answer — within the band a
            // type is what it brings, not what it costs.
            if best.map_or(true, |held| answer(candidate.0) > answer(held.0)) {
                best = Some(candidate);
            }
        }
        return best;
    }
    let present = tier_present(rules, obs, category);
    let thinnest = affordable
        .iter()
        .map(|(ty, _)| price_tier(obs.cost_of(rules, ty)))
        .min_by_key(|tier| (present[*tier], *tier))?;
    // Within the band, the dearest: a band is a price, not a preference, and
    // two types at the same price are not the same unit.
    affordable
        .iter()
        .filter(|(ty, _)| price_tier(obs.cost_of(rules, ty)) == thinnest)
        .max_by_key(|(ty, _)| obs.cost_of(rules, ty))
        .copied()
}

/// What the army is short of: the jobs the other side's field asks for, less
/// what this house's own army already answers, floored at zero. `None` when it
/// is short of nothing, and then the wallet decides instead of the threat.
///
/// The enemy list is `Observation::enemy_entities`' own: the whole map, fog
/// aside, which is the same view the native computer picks its targets from and
/// the same open question the module header records.
///
/// The arithmetic is a planner's, not the simulation's: it ranks types to buy
/// and writes nothing back, so its floats are a comparison, not state.
fn shortfall(rules: &RuleSet, obs: &Observation<'_>, roles: &RoleTable) -> Option<RoleWeights> {
    let mut threat = [0.0f64; ROLES.len()];
    for enemy in obs.enemy_entities() {
        let Some(ty) = obs.object_type(rules, enemy.type_id) else {
            continue;
        };
        // A building is ground the army stands on, not a threat it has to be
        // composed against.
        if ty.category == ObjectCategory::Building {
            continue;
        }
        let Some(role) = counter(ty) else {
            continue;
        };
        threat[role.index()] += 1.0;
    }

    let mut covered = [0.0f64; ROLES.len()];
    for entity in obs.own_entities() {
        let Some(ty) = obs.object_type(rules, entity.type_id) else {
            continue;
        };
        if ty.category == ObjectCategory::Building || ty.weapon_count == 0 || ty.resource_gatherer {
            continue;
        }
        let weights = roles.weights(&ty.id);
        for role in ROLES {
            covered[role.index()] += weights.get(role);
        }
    }

    let mut need = RoleWeights::default();
    let mut short = false;
    for role in ROLES {
        let gap = threat[role.index()] - covered[role.index()];
        if gap > 0.0 {
            need.0[role.index()] = gap;
            short = true;
        }
    }
    short.then_some(need)
}

/// The job that answers `ty`: shooting at an aircraft is its own job, and for
/// everything else it is the armour the type wears.
fn counter(ty: &ObjectType) -> Option<Role> {
    if ty.category == ObjectCategory::Aircraft {
        return Some(Role::AntiAir);
    }
    unit_role::role_against_armour(ty.armor_id())
}

/// How dear a unit the wallet may reach for: cheap while the base is short,
/// the middle while it is solvent, anything once it is rich — and never more
/// than it holds.
fn unit_ceiling(credits: i32) -> i32 {
    let band = if credits < POOR_CREDITS {
        CHEAP_UNIT_CREDITS
    } else if credits < RICH_CREDITS {
        MID_UNIT_CREDITS
    } else {
        i32::MAX
    };
    band.min(credits)
}

/// The price band a unit falls in: cheap, the line unit, or the heavy end. The
/// cuts are retail's own steps — a soldier around 200, a line vehicle around
/// 900, the heavy and long-range types above that — read as the type's own
/// `Cost=` through the house's cost factors.
fn price_tier(cost: i32) -> usize {
    if cost <= CHEAP_UNIT_CREDITS {
        0
    } else if cost <= MID_UNIT_CREDITS {
        1
    } else {
        2
    }
}

/// How many fighting units of `category` the house already holds, per band.
fn tier_present(rules: &RuleSet, obs: &Observation<'_>, category: ObjectCategory) -> [usize; 3] {
    let mut present = [0usize; 3];
    for entity in obs.own_entities() {
        let Some(ty) = obs.object_type(rules, entity.type_id) else {
            continue;
        };
        if ty.category != category || ty.weapon_count == 0 || ty.resource_gatherer {
            continue;
        }
        present[price_tier(obs.cost_of(rules, ty))] += 1;
    }
    present
}

/// Every type of `category` the rules' array holds, in array order, with its
/// id: the house's side or its prerequisites aside, so a caller can ask what a
/// type it may not build yet would need.
pub(super) fn all_class_types<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    category: ObjectCategory,
) -> impl Iterator<Item = (&'r ObjectType, InternedId)> {
    let count = rules.type_array_ids(category).len();
    (0..count).filter_map(move |index| {
        let ty = rules.type_array_at(category, i32::try_from(index).ok()?)?;
        let type_id = obs.type_id(&ty.id)?;
        Some((ty, type_id))
    })
}

/// Every type of `category` the house may build now, in the rules' array
/// order, with its id.
pub(super) fn class_types<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    category: ObjectCategory,
) -> impl Iterator<Item = (&'r ObjectType, InternedId)> {
    all_class_types(rules, obs, category)
        .filter(|(ty, _)| obs.can_build(rules, ty, true) != CanBuild::No)
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

    /// What the army is short of is read off the jobs the enemy's own types
    /// ask for: a tank by something that bites armour, a conscript by
    /// something that bites soldiers, and neither by reach. An aircraft asks
    /// for the one thing that can reach the sky, which is the same mapping
    /// read straight off the type.
    #[test]
    fn the_jobs_the_army_is_short_of_are_the_enemys_own() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [("HTNK", (6u16, 6u16)), ("E2", (8, 8))] {
            sim.spawn_object(name, "Russians", cell.0, cell.1, 64, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        let obs = Observation::new(&sim, owner);
        let roles = RoleTable::build(&rules);
        let need = shortfall(&rules, &obs, &roles).expect("an army of nothing is short of it all");

        assert!(need.get(Role::AntiVehicle) > 0.0, "a Rhino tank: {need:?}");
        assert!(need.get(Role::AntiInfantry) > 0.0, "a conscript: {need:?}");
        assert_eq!(need.get(Role::AntiAir), 0.0, "nothing in the air: {need:?}");
        assert_eq!(need.get(Role::AntiStructure), 0.0, "no buildings: {need:?}");
        assert_eq!(
            need.get(Role::Artillery),
            0.0,
            "nothing asks for reach: {need:?}"
        );
        assert_eq!(
            counter(rules.object("APACHE").expect("an Apache")),
            Some(Role::AntiAir),
            "what answers an aircraft is what can reach it"
        );
    }

    /// The house's own team needs come before any ranking, and they include
    /// needs a combat weight cannot state: a team that asks for an engineer is
    /// answered with an engineer, which no warhead ranking would ever pick.
    #[test]
    fn a_factory_makes_what_the_houses_teams_asked_for() {
        use crate::sim::ai_unit_choice::UnitChoiceKind;

        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GAPILE", "Americans", 12, 12, 64, &rules)
            .expect("a barracks");
        let engineer = rules
            .type_array_index(ObjectCategory::Infantry, "ENGINEER")
            .expect("the retail engineer");
        let house = sim.houses.get_mut(&owner).expect("the house");
        house
            .ai_unit_choices_mut()
            .set(UnitChoiceKind::Infantry, engineer);
        house.economy_mut().set_credits_for_test(5_000);

        let obs = Observation::new(&sim, owner);
        let roles = RoleTable::build(&rules);
        let mut out = Outbox::default();
        make_units(&rules, &obs, &roles, &mut out);

        let queued: Vec<&str> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::QueueProduction { type_id } => Some(sim.interner().resolve(*type_id)),
                _ => None,
            })
            .collect();
        assert_eq!(
            queued,
            ["ENGINEER"],
            "the house's own choice is what its barracks makes"
        );
    }

    /// Coverage is the other half: an army that can already answer everything
    /// it faces is short of nothing, and then the wallet decides instead.
    #[test]
    fn an_army_that_answers_everything_it_faces_is_short_of_nothing() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [("HTNK", (6u16, 6u16)), ("E2", (8, 8))] {
            sim.spawn_object(name, "Russians", cell.0, cell.1, 64, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        // The IFV answers both, and the arena's rules arm it: four of them
        // cover what a tank and a conscript ask for.
        for cell in [(16u16, 16u16), (18, 16), (20, 16), (16, 18)] {
            sim.spawn_object("FV", "Americans", cell.0, cell.1, 64, &rules)
                .expect("an IFV");
        }
        let obs = Observation::new(&sim, owner);
        let roles = RoleTable::build(&rules);
        assert!(
            shortfall(&rules, &obs, &roles).is_none(),
            "four IFVs answer a tank and a conscript"
        );
    }
}
