//! The brain's own economy: what the base builds, and in what order.
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
//! The order is a player's, and it is written as one list rather than as
//! nested conditions, because it is the one thing about this brain a play test
//! can measure: power when the base is short of it, income, the barracks, one
//! war factory, the base's own defense, and only then throughput and tech.
//! Building types come from the `[AI]` lists in the rules (`BuildPower=`,
//! `BuildRefinery=`, `BuildBarracks=`, `BuildWeapons=`, `BuildRadar=`,
//! `BuildTech=`), so a mod's rules change what this builds without touching
//! VERA.
//!
//! The counts behind every question here come from [`super::census`], which
//! walks the house's objects once a frame; this module asks it rather than
//! walking the entity store a second time.
//!
//! Determinism: every count and every choice walks the rules' arrays and the
//! entity store in their own order, and every test is an integer comparison.

use crate::rules::object_type::{FactoryType, ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::control::placement::{PLACEMENT_ATTEMPTS, Placement};
use crate::sim::intern::InternedId;
use crate::sim::production::{CanBuild, ProductionCategory};

use super::RetailBrain;
use super::census::Census;

/// How many harvesters one refinery keeps busy. Two, not the rules'
/// `HarvestersPerRefinery=`: the trip out and back is most of a harvester's
/// life, and a third one mostly queues at the bay.
pub(super) const HARVESTERS_PER_REFINERY: usize = 2;

/// How many refineries the base is built around.
///
/// Two, not three: an unloading bay is the dearest structure in the opening
/// and the second one only pays once the harvesters to fill it exist. Past
/// this the ore within reach of the base is the limit, not the bays, and a
/// house that keeps buying them is a house whose army never gets bought.
const REFINERY_TARGET: usize = 2;

/// How many credit the wallet has to leave behind before a structure that only
/// unlocks better types is worth starting.
const TECH_MIN_CREDITS: i32 = 4_000;

/// The wallet a building on the yard maker's `Prerequisite=` list has to
/// leave behind: it buys the ability to rebuild a lost yard, which only pays
/// off once the base can afford to lose one.
const SUPPORT_MIN_CREDITS: i32 = 4_000;

/// How many fighting units the house fields before it buys a radar.
const ARMY_FOR_TECH: usize = 8;

/// How many war factories the house runs before it is rich.
///
/// One. A single factory makes the harvesters, the yard maker and the whole
/// army; a second one doubles throughput the house cannot pay for yet, at the
/// price of the army it could have bought instead. The second line is a
/// surplus decision ([`SURPLUS_CREDITS`]), not an opening one.
const WAR_FACTORY_TARGET: usize = 1;

/// The wallet a house has to be holding before it treats a factory as the
/// bottleneck: past this it cannot spend what it earns, so the answer is
/// another factory rather than another harvester.
const SURPLUS_CREDITS: i32 = 12_000;

/// How many war factories and barracks a house with a surplus runs.
const WAR_FACTORY_SURPLUS: usize = 3;
const BARRACKS_SURPLUS: usize = 2;

/// How many base defenses the house keeps standing.
///
/// The opponent pushes with numbers a player's opening army cannot match, and
/// a base with no tower is a base whose production one raid stops for good:
/// the yard goes dark, the factories follow, and the match is lost by
/// logistics rather than by the fight.
const DEFENSE_TARGET: usize = 2;

/// One frame's base spending: take what is finished out of the yard and start
/// the next structure. The production lines are
/// [`super::composition::fill_lines`]' job.
pub(super) fn spend(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    place_finished(brain, rules, obs, out);
    build_next(rules, obs, &brain.census, out);
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
    for type_id in brain.census.ready.clone() {
        waiting.push(type_id);
        if !out.intents.is_empty() {
            continue;
        }
        let attempt = brain.site_attempts.get(&type_id).copied().unwrap_or(0);
        let Some(cell) = Placement::new(obs, rules).cell_for(type_id, attempt) else {
            log::debug!(
                "auto play: no cell for {} (attempt {attempt})",
                obs.object_type(rules, type_id).map_or("?", |ty| ty.id.as_str())
            );
            continue;
        };
        log::debug!(
            "auto play: place {} at {cell:?} (attempt {attempt})",
            obs.object_type(rules, type_id).map_or("?", |ty| ty.id.as_str())
        );
        out.intent(Intent::PlaceBuilding { type_id, cell });
        let next = (attempt + 1) % PLACEMENT_ATTEMPTS;
        brain.site_attempts.insert(type_id, next);
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
fn harvester_reserve(rules: &RuleSet, obs: &Observation<'_>, census: &Census) -> i32 {
    if census.harvesters >= HARVESTERS_PER_REFINERY * census.refineries.max(1) {
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
fn build_next(rules: &RuleSet, obs: &Observation<'_>, census: &Census, out: &mut Outbox) {
    if census.line(ProductionCategory::Building).product.is_some() {
        return;
    }
    let Some((ty, type_id)) = next_building(rules, obs, census) else {
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
    if !earns_its_keep(rules, ty) && harvester_reserve(rules, obs, census) > 0 {
        return;
    }
    log::debug!(
        "auto play: build {} credits={} power={} fighters={}",
        ty.id,
        obs.credits(),
        obs.power_balance(),
        census.fighters
    );
    out.intent(Intent::QueueProduction { type_id });
}

/// The structure to start now.
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
    census: &Census,
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
    let (ty, type_id) = match candidate(rules, obs, census) {
        Some(found) => found,
        // Nothing the base wants is buildable yet, and the house has no plant:
        // `Power=` plants are the prerequisite most structures hang off in the
        // retail rules, so a yard that can only make one is a yard that needs
        // a plant. With a plant already standing there is nothing to add — the
        // house waits for whatever it is missing instead of building blindly.
        None if count_in(obs, rules, census, &rules.build_power_types) == 0 => {
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

/// The structure the base is missing most, in the order a player builds one.
fn candidate<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    census: &Census,
) -> Option<(&'r ObjectType, InternedId)> {
    // 1. Income. The first bay is what an opening is made of; the second is
    // worth it once the harvesters to fill it exist, which the count says.
    if census.refineries < REFINERY_TARGET
        && (census.refineries == 0 || census.harvesters >= census.refineries * 2)
        && let Some(found) = first_buildable(rules, obs, &rules.build_refinery_types)
    {
        return Some(found);
    }
    // 2. The barracks, before the war factory: it is the cheaper structure and
    // the infantry line is the cheapest army a house can field. A base that
    // builds the factory first spends its opening wallet on one tank and has
    // no soldiers to hold anything with.
    if let Some(found) = wanted_once(rules, obs, census, &rules.build_barracks_types) {
        return Some(found);
    }
    // 3. One war factory: the harvesters, the yard maker and the army all come
    // out of it, and one is enough until the wallet cannot be spent.
    if count_in(obs, rules, census, &rules.build_weapons_types) < WAR_FACTORY_TARGET {
        if let Some(found) = first_buildable(rules, obs, &rules.build_weapons_types) {
            return Some(found);
        }
    }
    // 4. Something to hold the base with, before anything that only makes it
    // grow: a radar is worth less than a base that survives the first raid.
    if can_replace_units(rules, obs, census)
        && count_in(obs, rules, census, base_defenses(rules, obs)) < DEFENSE_TARGET
    {
        if let Some(found) = first_buildable(rules, obs, base_defenses(rules, obs)) {
            return Some(found);
        }
    }
    // 5. Throughput, once the wallet says the factories cannot spend it: a
    // wallet past this cannot be spent by one line, and a house that banks it
    // has an army it could not build. Every extra factory shortens a build by
    // `[General] MultipleFactory=` (the rules' own factor per extra factory,
    // `Time_To_Build`'s `factory_count`).
    if obs.credits() >= SURPLUS_CREDITS {
        if count_in(obs, rules, census, &rules.build_weapons_types) < WAR_FACTORY_SURPLUS {
            if let Some(found) = first_buildable(rules, obs, &rules.build_weapons_types) {
                return Some(found);
            }
        }
        if count_in(obs, rules, census, &rules.build_barracks_types) < BARRACKS_SURPLUS {
            if let Some(found) = first_buildable(rules, obs, &rules.build_barracks_types) {
                return Some(found);
            }
        }
    }
    // 6. The way back from a lost yard, bought while the yard still stands: the
    // yard maker's own `Prerequisite=` list is the rules' answer to what has
    // to be there to rebuild it, and nothing else in this order asks for the
    // service depot retail's MCV wants. It waits for a wallet the army has
    // already been paid out of — it is insurance, and insurance that starves
    // the army is the raid it was bought against.
    if obs.credits() >= SUPPORT_MIN_CREDITS {
        if let Some(found) = super::expand::yard_maker_support(rules, obs, census) {
            return Some(found);
        }
    }
    // 7. The building a production line needs, taken from the rules rather
    // than from a name list: `[GAAIRC]`/`[AMRADR]` carry `Factory=AircraftType`,
    // and which of them this house may build is `CanBuild`'s answer — the
    // Americans are refused `GAAIRC` by `ForbiddenHouses=` and allowed `AMRADR`
    // by `RequiredHouses=`. `[AI] BuildRadar=` happens to name both, but it is
    // a radar list: a house whose side has no aircraft factory (the Soviets in
    // the retail rules have none at all — `NARADR` is a radar tower, not a
    // plant) simply has no aircraft to build, and that is the honest answer.
    if obs.credits() >= TECH_MIN_CREDITS && census.fighters >= ARMY_FOR_TECH {
        if let Some(found) = line_factory(rules, obs, census, ProductionCategory::Aircraft) {
            return Some(found);
        }
        if let Some(found) = wanted_once(rules, obs, census, &rules.build_radar_types) {
            return Some(found);
        }
        return wanted_once(rules, obs, census, &rules.build_tech_types);
    }
    None
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

/// The type from `names` the house does not own yet: a barracks, a radar or a
/// war factory is worth building once, and a second one is worth building only
/// after the army is fed.
fn wanted_once<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    names: &[String],
) -> Option<(&'r ObjectType, InternedId)> {
    if count_in(obs, rules, census, names) > 0 {
        return None;
    }
    first_buildable(rules, obs, names)
}

/// The house's own harvester: the first `ResourceGatherer=` Vehicle it may
/// build. Ore delivered is the whole income, so this one is not a choice
/// among equals, it is what pays for the rest.
pub(super) fn harvester<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
) -> Option<(&'r ObjectType, InternedId)> {
    class_types(rules, obs, crate::rules::object_type::ObjectCategory::Vehicle)
        .find(|(ty, _)| ty.resource_gatherer)
}

/// Every type of `category` the rules' array holds, in array order, with its
/// id: the house's side or its prerequisites aside, so a caller can ask what a
/// type it may not build yet would need.
pub(super) fn all_class_types<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    category: crate::rules::object_type::ObjectCategory,
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
    category: crate::rules::object_type::ObjectCategory,
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
pub(super) fn base_defended(rules: &RuleSet, obs: &Observation<'_>, census: &Census) -> bool {
    count_in(obs, rules, census, base_defenses(rules, obs)) > 0
}

/// Whether the base can replace what it loses: a war factory for the
/// vehicles, a barracks for the infantry.
///
/// A push spends the army it was given, and a house with no factory to
/// replace it has traded its whole force for whatever it killed. That is why
/// this is the gate on attacking rather than a bigger army alone.
pub(super) fn can_replace_units(rules: &RuleSet, obs: &Observation<'_>, census: &Census) -> bool {
    count_in(obs, rules, census, &rules.build_weapons_types) > 0
        && count_in(obs, rules, census, &rules.build_barracks_types) > 0
}

/// The building a production line needs, when the house has not got one: the
/// rules' own answer, read off `Factory=` rather than off a name list.
///
/// A line can only run where a factory of its category stands — `Aircraft` is
/// `Factory=AircraftType`, and the retail rules hold exactly two such
/// buildings, `GAAIRC` and `AMRADR`, both Allied: `[GAAIRC]
/// ForbiddenHouses=Americans` and `[AMRADR] RequiredHouses=Americans` split them
/// between the allied countries, and the Soviets have none at all. That is a
/// fact about the rules, not a gap in this brain, so the answer here is the
/// same `CanBuild` the sidebar asks.
///
/// `None` when the house already owns a factory of the category — the line
/// runs and there is nothing to build — or when nothing of the category is
/// buildable for it.
fn line_factory<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    category: ProductionCategory,
) -> Option<(&'r ObjectType, InternedId)> {
    let factory = match category {
        ProductionCategory::Aircraft => FactoryType::AircraftType,
        ProductionCategory::Building | ProductionCategory::Defense => {
            return None;
        }
        // The infantry and vehicle plants are named by `[AI] Build*=` and are
        // already steps of this order; a naval yard needs water this brain does
        // not look for, so it is not asked for.
        ProductionCategory::Infantry | ProductionCategory::Vehicle | ProductionCategory::Ship => {
            return None;
        }
    };
    let candidates: Vec<(&ObjectType, InternedId)> = class_types(rules, obs, ObjectCategory::Building)
        .filter(|(ty, _)| ty.factory_type() == Some(factory))
        .collect();
    if candidates
        .iter()
        .any(|(_, type_id)| census.type_count(*type_id) > 0)
    {
        return None;
    }
    candidates.into_iter().next()
}

/// How many of the house's objects are one of `names`.
pub(super) fn count_in(
    obs: &Observation<'_>,
    _rules: &RuleSet,
    census: &Census,
    names: &[String],
) -> usize {
    census.count_named(obs, names)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::world::Simulation;

    /// The retail rival arena, needed to field real object types.
    ///
    /// The rules come from the repository's own `ini/` snapshot rather than a
    /// retail install, so this order — the thing a play test measures — is
    /// regression tested wherever the sources are checked out.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    /// The ledger the order is decided from, built the way `think` builds it.
    fn ledger(rules: &RuleSet, sim: &Simulation, owner: InternedId) -> Census {
        let mut census = Census::default();
        census.refresh(
            &crate::rules::unit_role::RoleTable::build(rules),
            rules,
            &Observation::new(sim, owner),
        );
        census
    }

    /// The opening is income, then the barracks, then one war factory. A house
    /// that buys the factory first spends its opening wallet on one tank, and
    /// the infantry line that would have held the base never opens.
    #[test]
    fn the_opening_is_a_refinery_then_a_barracks_then_one_war_factory() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("an Allied Construction Yard");
        // `GAREFN` and `GAPILE` both want `Prerequisite=POWER,GACNST`
        // (ini/rulesmd.ini:11731, :11692), so the plant is part of the opening.
        sim.spawn_object("GAPOWR", "Americans", 20, 16, 255, &rules)
            .expect("a power plant");
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(5_000);

        let census = ledger(&rules, &sim, owner);
        let (ty, _) = candidate(&rules, &Observation::new(&sim, owner), &census)
            .expect("a base with nothing but a yard wants something");
        assert!(
            rules.build_refinery_types.iter().any(|name| name == &ty.id),
            "the first structure is the income: {}",
            ty.id
        );

        // A refinery on the map: the next structure is the barracks, not the
        // war factory.
        sim.spawn_object("GAREFN", "Americans", 20, 20, 255, &rules)
            .expect("a refinery");
        let census = ledger(&rules, &sim, owner);
        let (ty, _) = candidate(&rules, &Observation::new(&sim, owner), &census)
            .expect("a base with income wants the next thing");
        assert!(
            rules.build_barracks_types.iter().any(|name| name == &ty.id),
            "the barracks comes before the war factory: {}",
            ty.id
        );
    }

    /// The aircraft plant comes from the rules, not from a name list: the only
    /// two buildings with `Factory=AircraftType` in the retail rules are
    /// `GAAIRC` and `AMRADR`, and which one a house may build is `CanBuild`'s
    /// answer. A side with neither — the Soviets, whose "air headquarters" is
    /// the `NARADR` radar tower with no production — gets no aircraft, because
    /// the rules give it none.
    #[test]
    fn the_aircraft_plant_is_the_rules_aircraft_factory() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [
            ("GACNST", (8u16, 8u16)),
            ("GAPOWR", (12, 8)),
            // `[AMRADR] Prerequisite=GAREFN,GACNST`.
            ("GAREFN", (16, 8)),
        ] {
            sim.spawn_object(name, "Americans", cell.0, cell.1, 255, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        let census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let (plant, _) = line_factory(&rules, &obs, &census, ProductionCategory::Aircraft)
            .expect("the Americans have an aircraft plant");
        assert_eq!(plant.id, "AMRADR");
        assert_eq!(plant.factory_type(), Some(FactoryType::AircraftType));

        // Once it stands there is nothing more to build for that line.
        sim.spawn_object("AMRADR", "Americans", (16u16, 16u16).0, 16, 255, &rules)
            .expect("the plant");
        let census = ledger(&rules, &sim, owner);
        assert!(
            line_factory(&rules, &Observation::new(&sim, owner), &census, ProductionCategory::Aircraft)
                .is_none(),
            "the line's plant already stands"
        );
    }

    /// Two refineries are the cap. A third bay is the dearest structure in the
    /// game and pays only once the harvesters to fill it exist, so a house that
    /// keeps buying them never gets an army.
    #[test]
    fn the_third_refinery_is_off_the_table() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("a yard");
        sim.spawn_object("GAPOWR", "Americans", 12, 16, 255, &rules)
            .expect("a power plant");
        for cell in [(20u16, 20u16), (24, 24)] {
            sim.spawn_object("GAREFN", "Americans", cell.0, cell.1, 255, &rules)
                .expect("a refinery");
        }
        let census = ledger(&rules, &sim, owner);
        assert_eq!(census.refineries, REFINERY_TARGET, "two bays stand");
        assert!(
            candidate(&rules, &Observation::new(&sim, owner), &census)
                .is_none_or(|(ty, _)| !rules.build_refinery_types.iter().any(|name| name == &ty.id)),
            "a third refinery is not what the base builds next"
        );
    }

    /// One war factory is the target; the second is a surplus decision. A base
    /// that buys two lines before it has an army has bought throughput nothing
    /// is spending.
    #[test]
    fn the_second_war_factory_waits_for_a_surplus() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("a yard");
        sim.spawn_object("GAPOWR", "Americans", 12, 16, 255, &rules)
            .expect("a power plant");
        sim.spawn_object("GAREFN", "Americans", 20, 20, 255, &rules)
            .expect("a refinery");
        sim.spawn_object("GAPILE", "Americans", 24, 24, 255, &rules)
            .expect("a barracks");
        sim.spawn_object("GAWEAP", "Americans", 28, 28, 255, &rules)
            .expect("a war factory");
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(2_000);

        let census = ledger(&rules, &sim, owner);
        assert!(
            candidate(&rules, &Observation::new(&sim, owner), &census)
                .is_none_or(|(ty, _)| !rules.build_weapons_types.iter().any(|name| name == &ty.id)),
            "a poor house with one war factory does not start another"
        );
    }
}
