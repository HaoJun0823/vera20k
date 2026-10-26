//! What the production lines make: the house's target shape, and the type each
//! idle factory starts next.
//!
//! The old rule here ranked types by *summed* role weight, so one IFV's
//! anti-air, anti-infantry and anti-armour weights together answered a tank and
//! a conscript at once, `shortfall` came back empty, and the fallback — the
//! dearest type in the thinnest price band — bought the same IFV forever. That
//! is the "one type deep" army a play test reports. This module counts instead:
//! how many of the house's units can actually do each job
//! ([`Census::able`]) against how many of the enemy's units ask for it
//! ([`Census::threat`]), and it holds the infantry share to a floor, so the
//! barracks is a line that fires rather than one that stands idle.
//!
//! Every idle line is filled, including the aircraft and the naval ones: a
//! house that owns a factory of a category and never uses it has paid for
//! nothing.
//!
//! Determinism: integer comparisons over the rules' arrays in their own order,
//! with the role arithmetic (`f64`) used as a comparison only — the same
//! contract [`crate::rules::unit_role`] states for its weights.

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::rules::unit_role::ROLES;
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;
use crate::sim::production::ProductionCategory;

use super::RetailBrain;
use super::build::{self, HARVESTERS_PER_REFINERY};
use super::census::{self, CHEAP_UNIT_CREDITS, Census, Slot, price_tier};
use super::plan::Budget;

/// The wallet the house wants in hand before it buys troops. Below it every
/// credit buys another harvester or another bay, which is what makes the army
/// affordable later.
const COMBAT_MIN_CREDITS: i32 = 2_000;

/// The wallet the infantry line needs: a soldier costs a tenth of a tank, and
/// at the vehicle's floor the barracks never fires at all.
const INFANTRY_MIN_CREDITS: i32 = 500;

/// The wallet that decides how dear a band the house reaches for: under the
/// first it buys cheap, past the second it buys the heavy end.
const POOR_CREDITS: i32 = 1_200;
const RICH_CREDITS: i32 = 4_000;

/// The share of the fighting army that has to be infantry, in percent.
///
/// Infantry is the cheapest way to hold ground and the only answer to another
/// house's infantry that does not cost a factory's worth of credits per unit.
/// A brain that never buys it fights every engagement with vehicles it cannot
/// replace, which is what "still the IFV thing" describes from the outside.
const INFANTRY_SHARE_PERCENT: usize = 40;

/// The share of the fighting army infantry may *not* pass, in percent.
///
/// The floor above is what makes the barracks fire; this is what makes it
/// stop. A soldier is a tenth of a tank, so a barracks that answers every
/// frame buys infantry with every credit the house earns and the wallet never
/// reaches the price of a vehicle — the army a play test reports as "only ever
/// infantry". Past this the barracks waits for the vehicles to be paid for.
const INFANTRY_CEILING_PERCENT: usize = 55;

/// How much of one job's gap a single unit's weight can claim, in units. One
/// bottomless gap — a wall of conscripts — must not decide the whole army's
/// composition by itself.
const GAP_CAP: i32 = 3;

/// Fill every idle production line with what the house needs next.
pub(super) fn fill_lines(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    budget: &Budget,
    out: &mut Outbox,
) {
    // The infantry floor decides who gets the wallet first when it is thin: a
    // house below its share of infantry spends on the barracks before it
    // spends on the war factory.
    let infantry_short = infantry_below_floor(&brain.census);
    // And the ceiling decides when it stops. A soldier costs a tenth of a tank,
    // so a barracks left to itself spends every credit the house earns on
    // infantry and the wallet never reaches the price of one — which is the
    // "only ever infantry" army. Past the ceiling the barracks waits, unless
    // the house is rich enough that the vehicles are not being starved.
    let infantry_full = infantry_above_ceiling(&brain.census) && budget.free < RICH_CREDITS;
    let order = if infantry_short {
        [
            ProductionCategory::Infantry,
            ProductionCategory::Vehicle,
            ProductionCategory::Aircraft,
            ProductionCategory::Ship,
        ]
    } else {
        [
            ProductionCategory::Vehicle,
            ProductionCategory::Infantry,
            ProductionCategory::Aircraft,
            ProductionCategory::Ship,
        ]
    };
    for category in order {
        // An army that already holds its share of infantry stops buying it, so
        // the wallet can reach a vehicle.
        if infantry_full && category == ProductionCategory::Infantry {
            continue;
        }
        // A line holding a product, a queue or a finished object is working and
        // is not given another request; a line with nothing in it — including
        // one the house has not used yet — can be started. Whether the house
        // can make any type of this category at all is `CanBuild`'s answer, and
        // the candidate list below is already filtered by it.
        if brain.census.line(category).busy() {
            continue;
        }
        let wanted = product_for(brain, rules, obs, budget, category, infantry_short);
        queue_unit(rules, obs, budget, out, wanted);
    }
}

/// Whether the army is short of its infantry share.
///
/// A house with no army at all is short of it: `fighters.max(1)` makes the
/// first soldier the floor's, rather than leaving an empty army's share at
/// zero and letting the war factory decide what the opening is made of.
fn infantry_below_floor(census: &Census) -> bool {
    let infantry: usize = census.tiers[Slot::Infantry.index()].iter().sum();
    infantry * 100 < census.fighters.max(1) * INFANTRY_SHARE_PERCENT
}

/// Whether the army has all the infantry it needs, so the wallet can go to the
/// vehicles instead ([`INFANTRY_CEILING_PERCENT`]).
///
/// The same reading the floor makes, of the same fighters: a house with no
/// army is below the floor and below the ceiling, and the barracks fires.
fn infantry_above_ceiling(census: &Census) -> bool {
    let infantry: usize = census.tiers[Slot::Infantry.index()].iter().sum();
    infantry * 100 > census.fighters.max(1) * INFANTRY_CEILING_PERCENT
}

/// How many engineers the house keeps, and how many spies.
///
/// Two and one. An engineer is spent the moment it walks into a building, so
/// the house wants another behind it — a captured refinery is worth a dozen
/// soldiers, and the second engineer is the one that takes it after the first
/// is gone. A spy is a raid that walks for minutes and is worth exactly one at
/// a time: it does its work in a single building and does not come out.
const ENGINEER_WANT: usize = 2;
const SPY_WANT: usize = 1;

/// The tool this line should make, when the house has fewer than it wants and
/// can pay for one.
///
/// Neither an engineer nor a spy is army, and neither was being made: the
/// house's own choice (`chosen_product`) reads the AI's unit choices, which
/// the simulation never fills for a house a human controls, and the army's own
/// ranking refuses both. So the capture and infiltration steps have been
/// running against an empty army — the engineer that was never built.
fn tool<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    credits: i32,
) -> Option<(&'r ObjectType, InternedId)> {
    let engineers = census
        .units
        .iter()
        .filter(|unit| unit.traits.engineer)
        .count();
    let spies = census
        .units
        .iter()
        .filter(|unit| unit.traits.infiltrator)
        .count();
    if engineers >= ENGINEER_WANT && spies >= SPY_WANT {
        return None;
    }
    build::class_types(rules, obs, ObjectCategory::Infantry)
        .filter(|(ty, _)| {
            (ty.engineer && engineers < ENGINEER_WANT) || (ty.agent && spies < SPY_WANT)
        })
        .filter(|(ty, _)| obs.cost_of(rules, ty) <= credits)
        // The engineer first: a spy's work is a raid, an engineer's is the
        // base's own growth. Then the cheaper, then the earlier rule.
        .min_by_key(|(ty, id)| (u8::from(!ty.engineer), obs.cost_of(rules, ty), *id))
}

/// What one idle line starts next.
fn product_for<'r>(
    brain: &mut RetailBrain,
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    budget: &Budget,
    category: ProductionCategory,
    infantry_short: bool,
) -> Option<(&'r ObjectType, InternedId)> {
    // The house's own choice first: `HouseClass::Suggest_New_Object
    // 0x004FBD80` is where the native team data reaches production, and a team
    // that asks for an engineer or a spy is answered here — no ranking over
    // combat weights can ask for those at all. It is unchecked (§ the reader),
    // so the wallet is this caller's guard.
    if let Some(found) = chosen_product(rules, obs, category, budget.free) {
        return Some(found);
    }

    if category == ProductionCategory::Vehicle {
        // A yard maker comes first while the house has neither a yard nor one
        // on the field — everything else the base builds is gated on it.
        let needs_maker = !super::expand::yard_stands(&brain.census)
            && !super::expand::owns_yard_maker(&brain.census);
        if needs_maker
            && let Some(maker) = super::expand::yard_maker(rules, obs)
        {
            return Some(maker);
        }
        // A harvester first while the bays are short of them, but only one the
        // house can actually buy: a harvester that needs a bay the house has
        // not built (retail's `[CMIN] Prerequisite=GAWEAP,PROC`) must not hold
        // the line idle while the army waits for a tank.
        if brain.census.harvesters < HARVESTERS_PER_REFINERY * brain.census.refineries.max(1)
            && let Some(found) = build::harvester(rules, obs)
        {
            return Some(found);
        }
        if budget.free < COMBAT_MIN_CREDITS {
            return None;
        }
        // A naval type is a `Vehicle` in the rules, so the war factory has to
        // ask for the land ones: a destroyer queued at the war factory is a
        // request the simulation refuses every frame.
        return fighter(
            brain,
            rules,
            obs,
            budget,
            ObjectCategory::Vehicle,
            Some(false),
            infantry_short,
        );
    }

    // A tool before an army: the buildings are taken with an engineer and a
    // spy, and neither is made by the army's own line — `fighter` ranks by
    // combat weight and asks `is_combat_unit`, which refuses an engineer
    // outright and refuses a spy for carrying no weapon. Correctly: neither is
    // what the fist is made of. So the want is stated here, in numbers.
    if category == ProductionCategory::Infantry
        && let Some(found) = tool(rules, obs, &brain.census, budget.free)
    {
        return Some(found);
    }

    if budget.free < INFANTRY_MIN_CREDITS {
        return None;
    }
    match category {
        ProductionCategory::Infantry => fighter(
            brain,
            rules,
            obs,
            budget,
            ObjectCategory::Infantry,
            None,
            infantry_short,
        ),
        ProductionCategory::Aircraft => {
            // An aircraft line is worth running at the same floor as the
            // vehicles: it is the dearest unit the house can buy.
            if budget.free < COMBAT_MIN_CREDITS {
                return None;
            }
            fighter(
                brain,
                rules,
                obs,
                budget,
                ObjectCategory::Aircraft,
                None,
                infantry_short,
            )
        }
        ProductionCategory::Ship => {
            if budget.free < COMBAT_MIN_CREDITS {
                return None;
            }
            // Naval units are `Vehicle` types carrying `Naval=`: the rules'
            // own category, and the one `class_types` indexes.
            fighter(
                brain,
                rules,
                obs,
                budget,
                ObjectCategory::Vehicle,
                Some(true),
                infantry_short,
            )
        }
        // The vehicle line answered above, and the two structure lines are
        // `build`'s.
        ProductionCategory::Vehicle
        | ProductionCategory::Building
        | ProductionCategory::Defense => None,
    }
}

/// The type the house's own choice has this factory make, when the house may
/// still build it and pay for it: `Suggest_New_Object 0x004FBD80`.
///
/// Two guards, both the caller's, because the native reader is unchecked: the
/// type must still be buildable (a choice can outlive its factory or its
/// prerequisite) and affordable. A harvester is skipped too — the harvester
/// count above is the economy's own owner, and the slot can still name one
/// after the count is met.
fn chosen_product<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    category: ProductionCategory,
    credits: i32,
) -> Option<(&'r ObjectType, InternedId)> {
    let object_category = match category {
        ProductionCategory::Infantry => ObjectCategory::Infantry,
        ProductionCategory::Vehicle | ProductionCategory::Ship => ObjectCategory::Vehicle,
        ProductionCategory::Aircraft => ObjectCategory::Aircraft,
        ProductionCategory::Building | ProductionCategory::Defense => return None,
    };
    let (ty, type_id) = obs.chosen_product(rules, object_category)?;
    if ty.resource_gatherer
        || obs.can_build(rules, ty, true) == crate::sim::production::CanBuild::No
        || credits < obs.cost_of(rules, ty)
    {
        return None;
    }
    Some((ty, type_id))
}

/// The fighting type of `category` the line should start next.
///
/// The gap closes first: a type is what it brings to a job the army cannot
/// already answer. Only when nothing is short does the price band decide, and
/// then the type the house holds fewest of — so a mixed army forms instead of
/// one type forever.
fn fighter<'r>(
    brain: &mut RetailBrain,
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    budget: &Budget,
    category: ObjectCategory,
    naval: Option<bool>,
    infantry_short: bool,
) -> Option<(&'r ObjectType, InternedId)> {
    let ceiling = unit_ceiling(budget.free);
    let affordable: Vec<(&ObjectType, InternedId, i32)> = build::class_types(rules, obs, category)
        .filter(|(ty, _)| naval.is_none_or(|wanted| ty.naval == wanted))
        .filter(|(ty, _)| super::orders::is_combat_unit(rules, ty))
        .filter_map(|(ty, type_id)| {
            let cost = obs.cost_of(rules, ty);
            (cost <= ceiling).then_some((ty, type_id, cost))
        })
        .collect();
    if affordable.is_empty() {
        return None;
    }

    // The infantry floor outranks the ranking: below the share, the barracks
    // buys the cheapest soldier it can pay for and the line keeps firing.
    if category == ObjectCategory::Infantry && infantry_short {
        return affordable
            .iter()
            .copied()
            .min_by_key(|(_, _, cost)| *cost)
            .map(|(ty, type_id, _)| (ty, type_id));
    }

    let need = brain.census.shortfall();
    let tiers = brain.census.tiers;
    if let Some(need) = need {
        // The table is borrowed for the scoring pass only: the fallback below
        // reads the ledger, and one `&mut brain` cannot be both.
        let mut best: Option<(f64, i32, &ObjectType, InternedId)> = None;
        {
            let roles = brain.roles(rules);
            for (ty, type_id, cost) in affordable.iter().copied() {
                let weights = roles.weights(&ty.id);
                // **The biggest gap it closes**, not the sum of all of them: a
                // type that answers four jobs a little is not four units, and
                // summing let one multi-role vehicle outrank every specialist
                // — which is what a play test sees as "still the IFV thing".
                let score = ROLES
                    .iter()
                    .map(|role| weights.get(*role) * f64::from(need[role.index()].min(GAP_CAP)))
                    .fold(0.0f64, f64::max);
                // Within the same answer, the cheaper type: a gap is closed by
                // one unit whether it costs 200 or 900.
                if best.is_none_or(|(held_score, held_cost, _, _)| {
                    (score, -cost) > (held_score, held_cost)
                }) {
                    best = Some((score, -cost, ty, type_id));
                }
            }
        }
        if let Some((score, _, ty, type_id)) = best
            && score > 0.0
        {
            return Some((ty, type_id));
        }
    }

    // Nothing is short: the thinnest price band that has anything the house can
    // pay for, and inside it the type the house holds fewest of. The dearest
    // breaks ties, because a band is a price and not a preference.
    //
    // The bands are tried in order rather than picking one and hoping: the
    // cheapest band often holds no fighting type at all (a band under 500 is
    // soldiers and the odd light vehicle), and returning "nothing" there is
    // what kept the war factory idle while the house was rich.
    let present = tiers[slot_category(category).index()];
    let mut bands: Vec<usize> = (0..3).collect();
    bands.sort_by_key(|tier| (present[*tier], *tier));
    for band in bands {
        let pick = affordable
            .iter()
            .copied()
            .filter(|(_, _, cost)| price_tier(*cost) == band)
            .min_by_key(|(_, type_id, cost)| (brain.census.type_count(*type_id), -cost));
        if let Some((ty, type_id, _)) = pick {
            return Some((ty, type_id));
        }
    }
    None
}

/// The pool a rules category's fighting units are counted in. Naval units are
/// their own pool, which the rules express as `Naval=yes` on a `Vehicle`; a
/// naval build reaches this function through the `Ship` line instead.
fn slot_category(category: ObjectCategory) -> Slot {
    match category {
        ObjectCategory::Infantry => Slot::Infantry,
        ObjectCategory::Vehicle => Slot::Vehicle,
        ObjectCategory::Aircraft => Slot::Aircraft,
        ObjectCategory::Building => Slot::Building,
    }
}

/// How dear a unit the wallet may reach for: cheap while the base is short,
/// the middle while it is solvent, anything once it is rich — and never more
/// than it holds.
fn unit_ceiling(credits: i32) -> i32 {
    let band = if credits < POOR_CREDITS {
        CHEAP_UNIT_CREDITS
    } else if credits < RICH_CREDITS {
        census::MID_UNIT_CREDITS
    } else {
        i32::MAX
    };
    band.min(credits)
}

fn queue_unit(
    rules: &RuleSet,
    obs: &Observation<'_>,
    budget: &Budget,
    out: &mut Outbox,
    wanted: Option<(&ObjectType, InternedId)>,
) {
    let Some((ty, type_id)) = wanted else {
        return;
    };
    if budget.free < obs.cost_of(rules, ty) {
        return;
    }
    log::debug!(
        "auto play: unit {} at {} (credits {})",
        ty.id,
        obs.cost_of(rules, ty),
        budget.free
    );
    out.intent(Intent::QueueProduction { type_id });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::world::Simulation;

    /// The retail rival arena, needed to field real object types. The rules
    /// come from the repository's own `ini/` snapshot, so the lines' choices are
    /// regression tested without a retail install.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    fn ledger(rules: &RuleSet, sim: &Simulation, owner: InternedId) -> Census {
        let mut census = Census::default();
        census.refresh(
            &crate::rules::unit_role::RoleTable::build(rules),
            rules,
            &Observation::new(sim, owner),
        );
        census
    }

    /// The barracks is a line that fires. Before the floor, an army with no
    /// infantry counted as short of nothing — the vehicle ranking answered
    /// every job — and the barracks stood idle for the whole match.
    #[test]
    fn a_barracks_below_the_infantry_share_makes_soldiers() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("a yard");
        sim.spawn_object("GAPOWR", "Americans", 20, 16, 255, &rules)
            .expect("a power plant");
        sim.spawn_object("GAPILE", "Americans", 24, 16, 255, &rules)
            .expect("a barracks");
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(3_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let mut out = Outbox::default();
        let budget = Budget {
            committed: 0,
            free: obs.credits(),
            income: None,
        };
        fill_lines(&mut brain, &rules, &obs, &budget, &mut out);

        let queued: Vec<&str> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::QueueProduction { type_id } => Some(sim.interner().resolve(*type_id)),
                _ => None,
            })
            .collect();
        assert_eq!(queued.len(), 1, "the idle barracks starts one build");
        let ty = rules.object(queued[0]).expect("a rules type");
        assert_eq!(
            ty.category,
            ObjectCategory::Infantry,
            "the barracks makes a soldier, not nothing: {}",
            queued[0]
        );
    }

    /// A house with no factory of a category has nothing to fill: the type
    /// filter is `CanBuild`, and a type with no factory of its category in the
    /// house's hands answers No. Without that, every frame would queue a
    /// request the simulation refuses.
    #[test]
    fn a_house_with_no_factory_queues_nothing() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("a yard");
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(5_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let mut out = Outbox::default();
        let budget = Budget {
            committed: 0,
            free: obs.credits(),
            income: None,
        };
        fill_lines(&mut brain, &rules, &obs, &budget, &mut out);

        assert!(
            out.intents.is_empty(),
            "no barracks, no war factory, nothing to start: {:?}",
            out.intents
        );
    }

    /// A rich house with an idle war factory starts a vehicle. The cheapest
    /// price band holds no fighting vehicle at all, so picking the thinnest band
    /// and giving up when it is empty is how the factory went idle for a whole
    /// match while the wallet filled up.
    #[test]
    fn a_rich_house_with_an_idle_war_factory_starts_a_vehicle() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("a yard");
        sim.spawn_object("GAPOWR", "Americans", 20, 16, 255, &rules)
            .expect("a power plant");
        sim.spawn_object("GAWEAP", "Americans", 24, 16, 255, &rules)
            .expect("a war factory");
        // A bay and the two miners it keeps busy, so the harvester arm of the
        // vehicle line is satisfied and the fighting rank is what is asked.
        sim.spawn_object("GAREFN", "Americans", 16, 24, 255, &rules)
            .expect("a refinery");
        for cell in [(24u16, 24u16), (26, 24)] {
            sim.spawn_object("CMIN", "Americans", cell.0, cell.1, 255, &rules)
                .expect("a Chrono Miner");
        }
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(4_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let mut out = Outbox::default();
        let budget = Budget {
            committed: 0,
            free: obs.credits(),
            income: None,
        };
        fill_lines(&mut brain, &rules, &obs, &budget, &mut out);

        let queued: Vec<&str> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::QueueProduction { type_id } => Some(sim.interner().resolve(*type_id)),
                _ => None,
            })
            .collect();
        assert_eq!(queued.len(), 1, "one line, one build: {:?}", out.intents);
        let ty = rules.object(queued[0]).expect("a rules type");
        assert!(
            ty.category == ObjectCategory::Vehicle && !ty.naval,
            "the war factory makes a land vehicle: {}",
            queued[0]
        );
        assert!(
            !ty.resource_gatherer,
            "the miners are already at the bays: {}",
            queued[0]
        );
    }

    /// A house that already owns a few of the dearest cheap vehicle does not buy
    /// another: with nothing short, the pick is the type it holds fewest of. One
    /// type forever — the "still the IFV thing" in a play test — is what the old
    /// ranking did, because the dearest type inside the thinnest band always won
    /// the tie against itself.
    #[test]
    fn the_vehicle_line_does_not_buy_one_type_forever() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [
            ("GACNST", (8u16, 8u16)),
            ("GAPOWR", (12, 8)),
            ("GAREFN", (16, 8)),
            ("GAWEAP", (8, 16)),
        ] {
            sim.spawn_object(name, "Americans", cell.0, cell.1, 255, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        for cell in [(16u16, 16u16), (18, 16)] {
            sim.spawn_object("CMIN", "Americans", cell.0, cell.1, 255, &rules)
                .expect("a Chrono Miner");
        }
        for cell in [(20u16, 16u16), (22, 16), (24, 16)] {
            sim.spawn_object("MTNK", "Americans", cell.0, cell.1, 255, &rules)
                .expect("a Grizzly");
        }
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(6_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let mut out = Outbox::default();
        let budget = Budget {
            committed: 0,
            free: obs.credits(),
            income: None,
        };
        fill_lines(&mut brain, &rules, &obs, &budget, &mut out);

        let queued: Vec<&str> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::QueueProduction { type_id } => Some(sim.interner().resolve(*type_id)),
                _ => None,
            })
            .collect();
        assert_eq!(queued.len(), 1, "{:?}", out.intents);
        assert_ne!(
            queued[0], "MTNK",
            "three of them already stand, so the line buys something else"
        );
    }

    /// The aircraft line is a line like any other once its plant stands. The
    /// plant is the rules' own `Factory=AircraftType` building —
    /// `build::line_factory`, not a name list — and for the Americans that is
    /// `AMRADR` (`RequiredHouses=Americans`), while `GAAIRC` is refused by
    /// `ForbiddenHouses=Americans` (ini/rulesmd.ini:11822).
    #[test]
    fn an_aircraft_line_makes_an_aircraft_once_the_hangar_stands() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [
            ("GACNST", (8u16, 8u16)),
            ("GAPOWR", (12, 8)),
            ("GAREFN", (16, 8)),
            ("GAWEAP", (8, 16)),
            ("AMRADR", (14, 16)),
        ] {
            sim.spawn_object(name, "Americans", cell.0, cell.1, 255, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(6_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let mut out = Outbox::default();
        let budget = Budget {
            committed: 0,
            free: obs.credits(),
            income: None,
        };
        fill_lines(&mut brain, &rules, &obs, &budget, &mut out);

        let aircraft: Vec<&str> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::QueueProduction { type_id } => {
                    let ty = obs.object_type(&rules, *type_id)?;
                    (ty.category == ObjectCategory::Aircraft)
                        .then(|| sim.interner().resolve(*type_id))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            aircraft.len(),
            1,
            "an idle air force command HQ starts an aircraft: {:?}",
            out.intents
        );
    }

    /// An army that already holds its share of infantry stops buying it, so
    /// the wallet can reach the price of a tank. A soldier is a tenth of a
    /// tank, so a barracks left to itself spends every credit the house earns
    /// on infantry and the war factory never gets to buy anything — the "only
    /// ever infantry" army.
    #[test]
    fn an_army_past_its_infantry_share_buys_the_vehicle_instead() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [
            ("GACNST", (16u16, 16u16)),
            ("GAPOWR", (12, 16)),
            ("GAPILE", (20, 16)),
            ("GAWEAP", (24, 16)),
            // The bays have their harvesters, so the vehicle line is free to
            // spend on the army rather than on income.
            ("CMIN", (28, 16)),
            ("CMIN", (30, 16)),
        ] {
            sim.spawn_object(name, "Americans", cell.0, cell.1, 255, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        // Six soldiers and nothing else: past the ceiling, not past the floor.
        for index in 0..6u16 {
            sim.spawn_object("E1", "Americans", 20 + index, 24, 255, &rules)
                .expect("a soldier");
        }
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(3_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let mut out = Outbox::default();
        let budget = Budget {
            committed: 0,
            free: obs.credits(),
            income: None,
        };
        fill_lines(&mut brain, &rules, &obs, &budget, &mut out);

        let queued: Vec<&str> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::QueueProduction { type_id } => Some(sim.interner().resolve(*type_id)),
                _ => None,
            })
            .collect();
        for name in &queued {
            let ty = rules.object(name).expect("a rules type");
            assert_ne!(
                ty.category,
                ObjectCategory::Infantry,
                "the barracks stops past the ceiling: {name}"
            );
        }
        assert!(
            queued.iter().any(|name| rules
                .object(name)
                .expect("a rules type")
                .category
                == ObjectCategory::Vehicle),
            "the wallet reaches the war factory: {queued:?}"
        );
    }

    /// A house with no engineer makes one. The buildings are taken with them,
    /// and the army's own ranking refuses an engineer outright and refuses a
    /// spy for carrying no weapon — so the capture and infiltration steps have
    /// been running against an empty army.
    #[test]
    fn a_barracks_makes_the_engineer_the_army_refuses() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [
            ("GACNST", (16u16, 16u16)),
            ("GAPOWR", (12, 16)),
            ("GAPILE", (20, 16)),
        ] {
            sim.spawn_object(name, "Americans", cell.0, cell.1, 255, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(3_000);

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let budget = Budget {
            committed: 0,
            free: obs.credits(),
            income: None,
        };
        let mut out = Outbox::default();
        fill_lines(&mut brain, &rules, &obs, &budget, &mut out);

        let made: Vec<&str> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::QueueProduction { type_id } => Some(sim.interner().resolve(*type_id)),
                _ => None,
            })
            .collect();
        assert!(
            made.iter()
                .any(|name| rules.object(name).is_some_and(|ty| ty.engineer)),
            "the engineer the army refuses: {made:?}"
        );
    }
}
