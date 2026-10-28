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
use crate::rules::superweapon_type::SuperWeaponKind;
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::control::placement::{PLACEMENT_ATTEMPTS, Placement};
use crate::sim::intern::InternedId;
use crate::sim::production::{CanBuild, ProductionCategory};

use super::RetailBrain;
use super::census::Census;
use super::plan::{self, Budget, Item};

/// How many harvesters one refinery keeps busy. Two, not the rules'
/// `HarvestersPerRefinery=`: the trip out and back is most of a harvester's
/// life, and a third one mostly queues at the bay.
pub(super) const HARVESTERS_PER_REFINERY: usize = 2;

/// How many refineries the base is built around, and when the second one is
/// worth it: entries of [`super::plan::STEPS`], because the shape of the base
/// is one list rather than a set of thresholds. Two, not three: an unloading
/// bay is the dearest structure in the opening and the second one only pays
/// once the harvesters to fill it exist.

/// The free wallet the tech stage needs before it answers: the price of the
/// dearest thing it buys.
///
/// `[GATECH] Cost=2000`. A house does not start unlocking a tree it cannot
/// finish, and every cheaper step of the stage (`[AMRADR] 1000`, the radar)
/// costs less than this, so one number covers all three.
pub(super) const TECH_MIN_CREDITS: i32 = 2_000;

/// The free wallet a building on the yard maker's `Prerequisite=` list has to
/// hold: it buys the ability to rebuild a lost yard, which only pays off once
/// the base can afford to lose one.
pub(super) const SUPPORT_MIN_CREDITS: i32 = 4_000;

/// How many war factories the house runs before it is rich, and how many a
/// rich house runs: the list in [`super::plan`] holds the numbers now, because
/// they are entries of one order rather than thresholds of their own.

/// The free wallet a house has to be holding before it treats a factory as the
/// bottleneck: past this it cannot spend what it earns, so the answer is
/// another factory rather than another harvester.
pub(super) const SURPLUS_CREDITS: i32 = 12_000;

/// The income one production line can spend, in credits a minute, and so the
/// income at which a second line is worth buying.
///
/// A line turns out a unit in its `Cost_Of` worth of frames: a 900-credit tank
/// takes about forty-five seconds to build, which is 1 200 credits a minute.
/// Two lines spend about 2 400. Past that the house is earning faster than its
/// factories can turn it into army, and the wallet it cannot spend is the
/// signal — not a round number the wallet happens to reach once.
pub(super) const SURPLUS_INCOME_PER_MINUTE: i32 = 2_400;

/// How long the build list's answer stands before it may answer a different
/// entry, in frames: forty seconds (600 frames at the 15 a second the
/// simulation steps).
///
/// Long enough to save for a structure the wallet cannot cover the frame it is
/// chosen — a gate opens on a wallet that is then spent elsewhere — and short
/// enough that a base which really has changed its mind (the defense it wanted
/// now stands) is not held to a plan for a minute.
pub(super) const BUILD_HOLD_FRAMES: u64 = 600;

/// How many war factories and barracks a house with a surplus runs: an entry
/// of [`super::plan::STEPS`], as the target counts above are.

/// One frame's base spending: take what is finished out of the yard and start
/// the next structure. The production lines are
/// [`super::composition::fill_lines`]' job.
pub(super) fn spend(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    budget: &Budget,
    out: &mut Outbox,
) {
    place_finished(brain, rules, obs, out);
    build_next(brain, rules, obs, budget, out);
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
    if census.harvesters_served() {
        return 0;
    }
    harvester(rules, obs).map_or(0, |(ty, _)| obs.cost_of(rules, ty))
}

/// Whether the type earns its keep: power, income, the factory the harvesters
/// come out of, or the tech the rest of the tree hangs off.
///
/// The tech entries are here for the same reason the refinery is: the oil
/// derrick — the one income a captured building pays — has
/// `[GAOREP] Prerequisite=GATECH,...`, so the lab is income one step removed.
/// Without them the harvester reserve blocked the whole tree for as long as
/// the house was short a harvester, which is how a house reached the end of a
/// match with no lab and no aircraft it could build.
fn earns_its_keep(rules: &RuleSet, ty: &ObjectType) -> bool {
    [
        &rules.build_power_types,
        &rules.build_refinery_types,
        &rules.build_weapons_types,
        &rules.build_radar_types,
        &rules.build_tech_types,
    ]
    .iter()
    .any(|names| names.iter().any(|name| name == &ty.id))
}

/// Start the one structure the base needs most, while the yard is free.
fn build_next(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    budget: &Budget,
    out: &mut Outbox,
) {
    // Nothing is being saved for until this frame says what it is: a reserve
    // left over from a decision that has since been paid for would starve the
    // factories of money nobody is saving.
    brain.yard_reserve = 0;
    // A structure the yard has finished and not yet put down is *owned*: it is
    // waiting for a cell, not for a buyer. Starting another because the count
    // on the map has not caught up is how an opening ended with two power
    // plants — the second was queued in the frames the first spent waiting for
    // its cell, and then the yard built both.
    if brain.census.line(ProductionCategory::Building).busy()
        || brain.census.line(ProductionCategory::Defense).ready.is_some()
    {
        return;
    }
    let tick = obs.tick();
    let held = brain.plan_hold.filter(|(_, until)| tick < *until);
    let Some(chosen) =
        next_building(rules, obs, &brain.census, budget, held.map(|(step, _)| step))
    else {
        return;
    };
    // Power is spliced in ahead of the plan and held by nobody, so the entry
    // the house was saving for still stands when the plant is paid for.
    if let Some(step) = chosen.step {
        // The hold is taken when the decision is made, not renewed while it
        // stands: a hold that refreshed every frame would never expire, and
        // the list would answer the same entry for the rest of the match.
        brain.plan_hold = Some((
            step,
            match held {
                Some((held_step, until)) if held_step == step => until,
                _ => tick.saturating_add(BUILD_HOLD_FRAMES),
            },
        ));
    }
    let ty = chosen.ty;
    let type_id = chosen.type_id;
    // A defense is made by the Defense factory, not the Buildings one, so the
    // guard above does not see it: a tower already on the line is queued again
    // on every frame of its build. Removing the repeats changes the match
    // (measured 2026-10-24: timeout at 36000 instead of a win), because the
    // repeated queue is also what keeps the tower count up, so the two have to
    // be re-balanced together — `DEFENSE_TARGET` at 4 did not make up for it.
    // A structure the yard is already making is not made twice.
    //
    // The busy guard above asks the Building line, and it is not enough: the
    // frame a plant is queued on, the line still reads idle, so the next frame
    // buys the same plant again. That is the opening with two power plants —
    // measured in a real match, both orders logged with `output=0 drain=0
    // power_short=0`, so it was never the power arithmetic: it was the same
    // entry chosen twice. `CanBuild`'s own view of what is already queued is
    // the answer to that, and it does not need the count on the map to catch
    // up.
    //
    // The Defense line is deliberately left out: its repeat is what keeps the
    // tower count up (see below), and that is a separate balance.
    if ty.category == ObjectCategory::Building && obs.in_production(rules, type_id) {
        return;
    }
    let price = obs.cost_of(rules, ty);
    // What the yard has settled on is not the lines' money — but only the tree
    // is worth starving them for. A structure the list has decided to build is
    // paid for as the wallet fills, so the part the house does not have yet is
    // held back from the factories; without it the barracks spent the lab's
    // money on one more soldier every frame and the tech stage never came.
    //
    // Held back for everything, though, it is the barracks that stops: a
    // thousand credits of reserve is thirty seconds of no infantry at all, and
    // a house with no infantry loses the army while it is saving. Only the
    // entries whose whole point is what they unlock are worth that price.
    brain.yard_reserve = match chosen.item {
        Item::Aircraft | Item::Radar | Item::Tech => price.saturating_sub(budget.free).max(0),
        _ => 0,
    };
    // The budget, not the wallet: what a line has been promised is spent
    // whether or not it has left the wallet yet, and a queue costs the wallet
    // nothing until the build steps.
    if budget.free < price {
        return;
    }
    brain.yard_reserve = 0;
    // While the harvesters are still short, only what earns or powers waits
    // for nothing else: a radar bought before the second harvester is the
    // money the second harvester needed.
    if !earns_its_keep(rules, ty) && harvester_reserve(rules, obs, &brain.census) > 0 {
        return;
    }
    // Info, not debug: one line per structure is a handful a match, and it is
    // the only record of *why* the yard started what it did — the opening's
    // two-plant mystery was invisible until these numbers were read.
    log::info!(
        "auto play: build {} at tick {} credits={} power={} fighters={} power_short={} output={} drain={} planned={}",
        ty.id,
        obs.tick(),
        obs.credits(),
        obs.power_balance(),
        brain.census.fighters,
        plan::power_deficit(rules, obs, &brain.census, 0),
        obs.power_output(),
        obs.power_drain(),
        plan::planned_output(rules, obs, &brain.census)
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
    budget: &Budget,
    held: Option<usize>,
) -> Option<plan::Chosen<'r>> {
    // A brownout is the one state that stops everything else: production
    // runs at a crawl and the defenses go offline. Whatever else the base
    // wants, a plant comes first, and it comes first even when nothing else
    // is left to build.
    if plan::power_short(rules, obs, census) {
        if let Some((ty, type_id)) = first_buildable(rules, obs, &rules.build_power_types) {
            return Some(plan::Chosen {
                step: None,
                item: Item::Power,
                ty,
                type_id,
            });
        }
    }
    let chosen = match plan::answer(rules, obs, census, budget, held) {
        Some(found) => found,
        // Nothing the base wants is buildable yet, and the house has no plant:
        // `Power=` plants are the prerequisite most structures hang off in the
        // retail rules, so a yard that can only make one is a yard that needs
        // a plant. With a plant already standing there is nothing to add — the
        // house waits for whatever it is missing instead of building blindly.
        None if count_in(obs, rules, census, &rules.build_power_types) == 0 => {
            let (ty, type_id) = first_buildable(rules, obs, &rules.build_power_types)?;
            plan::Chosen {
                step: None,
                item: Item::Power,
                ty,
                type_id,
            }
        }
        None => return None,
    };
    // `Power=` is the type's net contribution: negative is what it draws, and
    // the plant that would feed it is counted at the output it will have. The
    // lab's own `-100` is a plant the base needs before it starts the lab.
    if plan::power_deficit(rules, obs, census, chosen.ty.power.min(0)) > 0 {
        if let Some((ty, type_id)) = first_buildable(rules, obs, &rules.build_power_types) {
            return Some(plan::Chosen {
                step: None,
                item: Item::Power,
                ty,
                type_id,
            });
        }
    }
    Some(chosen)
}


/// The type from `names` the house may build now, in list order.
///
/// `can_build` is the production path's own test, so a type of another side,
/// one whose prerequisite is missing or one beyond the house's tech level
/// answers No here exactly as it would in the sidebar.
pub(super) fn first_buildable<'r>(
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
pub(super) fn base_defenses<'r>(rules: &'r RuleSet, obs: &Observation<'_>) -> &'r [String] {
    obs.side_index()
        .map_or(&[], |side| rules.base_defense_types(side))
}

/// The lab, preferring the one that authorises a power the house can use.
///
/// A super weapon is not granted by the power existing: it is granted by the
/// building that carries it (`SuperWeapon=` / `SuperWeapon2=`,
/// `src/rules/object_type.rs:3610-3611`, read by
/// `grant_provided_supers` in `src/sim/superweapon/mod.rs:1905`), and
/// `[AI] BuildTech=` names more than one lab. Taking the first one is how a
/// house spends a whole match holding a paradrop it was never authorised to
/// fire — which is what "the support powers are never used" turns out to be.
pub(super) fn tech_building<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
) -> Option<(&'r ObjectType, InternedId)> {
    class_types(rules, obs, ObjectCategory::Building)
        .filter(|(ty, _)| rules.build_tech_types.iter().any(|name| *name == ty.id))
        .min_by_key(|(ty, id)| (u8::from(!grants_support(rules, ty)), *id))
}

/// Whether a building authorises one of the powers this brain aims at
/// something other than its own force: a paradrop, a spy plane, a reveal.
fn grants_support(rules: &RuleSet, ty: &ObjectType) -> bool {
    [ty.super_weapon.as_deref(), ty.super_weapon2.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|name| rules.super_weapon(name))
        .any(|power| {
            matches!(
                power.kind,
                SuperWeaponKind::ParaDrop
                    | SuperWeaponKind::AmerParaDrop
                    | SuperWeaponKind::SpyPlane
                    | SuperWeaponKind::PsychicReveal
                    | SuperWeaponKind::UnitDelivery
            )
        })
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
pub(super) fn line_factory<'r>(
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

    /// The opening is the plant, then the barracks, then the income, then the
    /// tech: power is what every other structure's `Prerequisite=` asks for, the
    /// barracks is the cheaper production building and the one the engineer and
    /// the dog come out of, and the tree is unlocked before a second line or a
    /// second tower is bought.
    #[test]
    fn the_opening_is_power_then_barracks_then_income_then_tech() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("an Allied Construction Yard");
        // `GAREFN` and `GAPILE` both want `Prerequisite=POWER,GACNST`
        // (ini/rulesmd.ini:11731, :11692), so the plant is the first thing the
        // list can answer at all.
        let ty = {
            let census = ledger(&rules, &sim, owner);
            let obs = Observation::new(&sim, owner);
            let budget = plan::budget(&rules, &obs, &census, None);
            plan::answer(&rules, &obs, &census, &budget, None)
                .expect("a base with nothing but a yard wants something")
                .ty
        };
        assert!(
            rules.build_power_types.iter().any(|name| name == &ty.id),
            "the first structure is the plant: {}",
            ty.id
        );

        sim.spawn_object("GAPOWR", "Americans", 20, 16, 255, &rules)
            .expect("a power plant");
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(5_000);

        let census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let budget = plan::budget(&rules, &obs, &census, None);
        let ty = plan::answer(&rules, &obs, &census, &budget, None)
            .expect("a base with power wants the next thing")
            .ty;
        assert!(
            rules.build_barracks_types.iter().any(|name| name == &ty.id),
            "the first production building is the barracks: {}",
            ty.id
        );

        // A refinery on the map: still the barracks, because the production
        // building comes before the second bay.
        sim.spawn_object("GAREFN", "Americans", 20, 20, 255, &rules)
            .expect("a refinery");
        let census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let budget = plan::budget(&rules, &obs, &census, None);
        let ty = plan::answer(&rules, &obs, &census, &budget, None)
            .expect("a base with income wants the next thing")
            .ty;
        assert!(
            rules.build_barracks_types.iter().any(|name| name == &ty.id),
            "the barracks comes before the war factory: {}",
            ty.id
        );

        // With the opening in place and money in the wallet, the list answers
        // the tech chain and not a tower: the war factory first, because the
        // lab's own `Prerequisite=GAWEAP,RADAR,GACNST`, and then the aircraft
        // plant that stands in for the radar.
        sim.spawn_object("GAPILE", "Americans", 24, 24, 255, &rules)
            .expect("a barracks");
        let census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let budget = plan::budget(&rules, &obs, &census, None);
        let chosen = plan::answer(&rules, &obs, &census, &budget, None)
            .expect("an open tree is what the house buys next");
        assert_eq!(
            chosen.item,
            plan::Item::Weapons,
            "the war factory is the lab's own prerequisite"
        );
        assert!(
            !base_defenses(&rules, &obs)
                .iter()
                .any(|name| name == &chosen.ty.id),
            "not a tower: {}",
            chosen.ty.id
        );

        sim.spawn_object("GAWEAP", "Americans", 28, 24, 255, &rules)
            .expect("a war factory");
        let census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let budget = plan::budget(&rules, &obs, &census, None);
        let chosen = plan::answer(&rules, &obs, &census, &budget, None)
            .expect("the aircraft plant is what the tree asks for next");
        assert_eq!(chosen.item, plan::Item::Aircraft, "the tech stage");
        assert_eq!(chosen.ty.id, "AMRADR", "the Americans' aircraft plant");
    }

    /// A structure the yard has finished and not yet put down is a structure the
    /// house owns. Counting the map was how an opening bought two power plants:
    /// the second was queued in the frames the first spent waiting for its cell.
    #[test]
    fn a_structure_waiting_for_a_cell_is_not_bought_again() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("an Allied Construction Yard");
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(5_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        // The plant is built and standing in the yard, waiting for its cell.
        let plant = sim.interner().get("GAPOWR").expect("the power plant type");
        brain.census.lines[crate::sim::strategy::retail::census::LINE_BUILDING].ready =
            Some(plant);
        let obs = Observation::new(&sim, owner);
        let budget = plan::budget(&rules, &obs, &brain.census, None);
        let mut out = Outbox::default();
        spend(&mut brain, &rules, &obs, &budget, &mut out);

        assert!(
            !out.intents
                .iter()
                .any(|intent| matches!(intent, Intent::QueueProduction { .. })),
            "the yard is holding a plant, not asking for a second: {:?}",
            out.intents
        );
    }

    /// The tech buildings are not what the harvester reserve is holding money
    /// back for.
    ///
    /// `earns_its_keep` knew only about power, income and the war factory, so a
    /// house short of a harvester refused to buy the lab — and a house that
    /// never buys the lab never gets the rest of the tree, which is how a match
    /// ended with no lab and no aircraft either. The lab is income one step
    /// removed: `[GAOREP] Prerequisite=GATECH,PROC,GACNST` is the oil derrick.
    #[test]
    fn the_tech_buildings_are_not_what_the_harvester_reserve_holds_money_for() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let obs = Observation::new(&sim, owner);
        for name in ["GATECH", "AMRADR"] {
            let ty = obs
                .type_id(name)
                .and_then(|type_id| obs.object_type(&rules, type_id))
                .unwrap_or_else(|| panic!("{name} is in the rules"));
            assert!(
                earns_its_keep(&rules, ty),
                "{name} is not paid for out of the harvester reserve"
            );
        }
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
        assert_eq!(census.refineries, 2, "two bays stand");
        let obs = Observation::new(&sim, owner);
        let budget = plan::budget(&rules, &obs, &census, None);
        assert!(
            plan::answer(&rules, &obs, &census, &budget, None).is_none_or(|chosen| !rules
                .build_refinery_types
                .iter()
                .any(|name| name == &chosen.ty.id)),
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
        let obs = Observation::new(&sim, owner);
        let budget = plan::budget(&rules, &obs, &census, None);
        assert!(
            plan::answer(&rules, &obs, &census, &budget, None).is_none_or(|chosen| !rules
                .build_weapons_types
                .iter()
                .any(|name| name == &chosen.ty.id)),
            "a poor house with one war factory does not start another"
        );
    }

    /// A plant that is still building supplies its `Power=` scaled by the
    /// health it has built, so the balance reads a brownout for as long as the
    /// plant is being built. The base used to answer that with a second plant:
    /// the opening became two plants and one refinery, which is a base that
    /// paid for power it did not need.
    #[test]
    fn a_plant_that_is_building_is_the_plant_the_base_was_short_of() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("a yard");
        sim.spawn_object("GAREFN", "Americans", 20, 20, 255, &rules)
            .expect("a refinery");

        let mut census = ledger(&rules, &sim, owner);
        let plant = sim.interner().get("GAPOWR").expect("the plant's id");
        census.lines[crate::sim::strategy::retail::census::LINE_BUILDING].product = Some(plant);
        let obs = Observation::new(&sim, owner);
        assert!(
            obs.power_balance() < 0,
            "the plant on the line supplies nothing yet: {}",
            obs.power_balance()
        );
        assert!(
            !plan::power_short(&rules, &obs, &census),
            "the plant it is building is the plant it was short of"
        );
    }
}
