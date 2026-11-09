//! The five ledgers, and the two double-counting traps the plan names.
//!
//! Flow versus stock is the structural test: "produced 30" and "has 30" are different numbers,
//! and a ledger that stores one cannot answer the other. The two named traps are behavioural:
//! an engineer's capture is not production, and a mind-controlled unit is a loss recorded apart
//! from a control change.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::ledger::{Holding, Ledger};
use crate::world::Snapshot;

fn scene() -> Option<(Simulation, RuleSet, InternedId, InternedId)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x1ED6);
    vera20k_core::sim::arena_fixture::flat_ground(&mut sim, &rules);
    sim.intern_rule_type_ids(&rules);
    sim.resolve_type_handles(&rules);
    for (name, human) in [("Americans", true), ("Russians", false)] {
        let owner = sim.intern(name);
        let side = rules.country_side_index(name)?.0;
        let mut house = HouseState::new(owner, side, Some(owner), human, 0, 10);
        house.project_country_mults(&rules, sim.interner());
        sim.houses_mut_for_test().register(house);
    }
    let americans = sim.interner().get("Americans")?;
    let russians = sim.interner().get("Russians")?;
    Some((sim, rules, americans, russians))
}

/// The first observation is stock, not production.
///
/// A match does not begin by building its starting units. Counting them would make the first
/// report wildly wrong, and it is the simplest form of the flow/stock mistake.
#[test]
fn the_first_observation_is_stock_and_produces_nothing() {
    let Some((mut sim, rules, americans, _russians)) = scene() else {
        return;
    };
    sim.spawn_object("MTNK", "Americans", 8, 8, 0, &rules);
    let snapshot = Snapshot::take(&sim, &rules, americans);

    let mut ledger = Ledger::new();
    let movement = ledger.observe(&snapshot);

    // The first observation establishes the baseline and reports no movement at all: there is
    // nothing to compare against, and inventing "gained everything" would count a house's
    // starting army as production.
    assert!(
        movement.is_empty(),
        "the first observation measures nothing"
    );
    assert!(
        ledger.produced().is_empty(),
        "nothing was produced: the tank was already there"
    );
    // The stock is nevertheless recorded, because the next comparison needs it.
    let stock = ledger.holding(americans).expect("the baseline is kept");
    assert_eq!(stock.count_of("MTNK"), 1, "the stock is recorded");
}

/// A tank built between observations is production; one merely captured is not.
#[test]
fn a_built_tank_is_production_and_a_captured_one_is_a_transfer() {
    let Some((mut sim, rules, americans, russians)) = scene() else {
        return;
    };
    sim.spawn_object("MTNK", "Americans", 8, 8, 0, &rules);
    let first = Snapshot::take(&sim, &rules, americans);
    let mut ledger = Ledger::new();
    ledger.observe(&first);

    // Build one: new to the world and to this house.
    sim.spawn_object("MTNK", "Americans", 10, 8, 0, &rules);
    let second = Snapshot::take(&sim, &rules, americans);
    let movement = ledger.observe(&second);
    let ours = movement.get(&americans).expect("reported");
    assert_eq!(
        ours.produced(),
        vec![("MTNK".to_string(), 1)],
        "a newly built tank is production"
    );
    assert_eq!(ledger.produced().get("MTNK"), Some(&1));

    // Now capture an enemy tank: it becomes ours without being built.
    let enemy = sim
        .spawn_object("MTNK", "Russians", 20, 20, 0, &rules)
        .expect("spawns");
    let third = Snapshot::take(&sim, &rules, americans);
    ledger.observe(&third);
    let before_capture = ledger.produced().get("MTNK").copied().unwrap_or(0);

    sim.entities_mut_for_test().change_owner(enemy, americans);
    let fourth = Snapshot::take(&sim, &rules, americans);
    let movement = ledger.observe(&fourth);
    let ours = movement.get(&americans).expect("reported");

    // It appears as gained, and as transferred, but `produced` subtracts the transfer.
    assert_eq!(
        ours.produced(),
        Vec::<(String, usize)>::new(),
        "a captured tank must never be counted as production"
    );
    assert_eq!(
        ours.transferred_in.len(),
        1,
        "it is recorded as an ownership change instead"
    );
    assert_eq!(ours.transferred_in[0].id, enemy);
    assert_eq!(
        ledger.produced().get("MTNK").copied().unwrap_or(0),
        before_capture,
        "and the match total is unchanged by the capture"
    );
    assert_eq!(ledger.transferred().get("MTNK"), Some(&1));
    let _ = russians;
}

/// A destroyed unit is a loss; a captured one is a transfer. The plan keeps them apart.
#[test]
fn destruction_and_capture_are_recorded_differently() {
    let Some((mut sim, rules, americans, _russians)) = scene() else {
        return;
    };
    let doomed = sim
        .spawn_object("MTNK", "Americans", 8, 8, 0, &rules)
        .expect("spawns");
    let kept = sim
        .spawn_object("MTNK", "Americans", 9, 8, 0, &rules)
        .expect("spawns");
    let first = Snapshot::take(&sim, &rules, americans);
    let mut ledger = Ledger::new();
    ledger.observe(&first);

    // One is destroyed: gone from the world entirely.
    sim.entities_mut_for_test().remove(doomed);
    // The other changes hands: still standing, under another house.
    let russians = sim.interner().get("Russians").expect("interned");
    sim.entities_mut_for_test().change_owner(kept, russians);

    let second = Snapshot::take(&sim, &rules, americans);
    let movement = ledger.observe(&second);
    let ours = movement.get(&americans).expect("reported");

    assert_eq!(ours.destroyed.len(), 1, "one left the world");
    assert_eq!(ours.destroyed[0].id, doomed);
    assert_eq!(
        ours.destroyed[0].type_name, "MTNK",
        "the type is named from the previous holding, not from a row that no longer exists"
    );
    assert!(ours.destroyed[0].was_destroyed());
    assert!(!ours.destroyed[0].was_transferred());

    assert_eq!(ours.transferred_out.len(), 1, "one changed hands");
    assert_eq!(ours.transferred_out[0].id, kept);
    assert_eq!(ours.transferred_out[0].now_owned_by, Some(russians));
    assert!(ours.transferred_out[0].was_transferred());
    assert!(!ours.transferred_out[0].was_destroyed());
    assert_eq!(ledger.destroyed().get("MTNK"), Some(&1));
}

/// Flow accumulates across observations; stock describes only the last one.
#[test]
fn flow_accumulates_and_stock_is_current() {
    let Some((mut sim, rules, americans, _russians)) = scene() else {
        return;
    };
    sim.spawn_object("MTNK", "Americans", 8, 8, 0, &rules);
    let mut ledger = Ledger::new();
    let first = Snapshot::take(&sim, &rules, americans);
    ledger.observe(&first);

    for step in 1..=3u16 {
        sim.spawn_object("MTNK", "Americans", 8 + step, 8, 0, &rules);
        let snapshot = Snapshot::take(&sim, &rules, americans);
        ledger.observe(&snapshot);
    }

    assert_eq!(
        ledger.produced().get("MTNK").copied().unwrap_or(0),
        3,
        "three were built across the interval"
    );
    let stock = ledger.holding(americans).expect("held");
    assert_eq!(
        stock.count_of("MTNK"),
        4,
        "the stock is what it has now, including the one it started with"
    );
    assert_eq!(ledger.observations(), 4);
}

/// A house that loses everything is still reported, or its loss vanishes when it matters most.
#[test]
fn a_house_that_loses_everything_is_still_reported() {
    let Some((mut sim, rules, americans, _russians)) = scene() else {
        return;
    };
    let only = sim
        .spawn_object("MTNK", "Americans", 8, 8, 0, &rules)
        .expect("spawns");
    let first = Snapshot::take(&sim, &rules, americans);
    let mut ledger = Ledger::new();
    ledger.observe(&first);

    sim.entities_mut_for_test().remove(only);
    let second = Snapshot::take(&sim, &rules, americans);
    let movement = ledger.observe(&second);
    let ours = movement
        .get(&americans)
        .expect("a house with nothing left still gets a row");
    assert_eq!(ours.destroyed.len(), 1);
    assert_eq!(ours.holding.total(), 0, "its stock is now empty");
}

/// A new match starts blank.
#[test]
fn a_new_ledger_remembers_nothing() {
    let ledger = Ledger::new();
    assert!(ledger.produced().is_empty());
    assert!(ledger.destroyed().is_empty());
    assert!(ledger.transferred().is_empty());
    assert_eq!(ledger.observations(), 0);
}

/// A holding read straight from a snapshot reflects that house only.
#[test]
fn a_holding_counts_only_one_house() {
    let Some((mut sim, rules, americans, russians)) = scene() else {
        return;
    };
    sim.spawn_object("MTNK", "Americans", 8, 8, 0, &rules);
    sim.spawn_object("MTNK", "Russians", 20, 20, 0, &rules);
    sim.spawn_object("MTNK", "Russians", 21, 20, 0, &rules);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let ours = Holding::of(&snapshot, americans);
    let theirs = Holding::of(&snapshot, russians);
    assert_eq!(ours.count_of("MTNK"), 1);
    assert_eq!(theirs.count_of("MTNK"), 2);
    assert_eq!(ours.total(), 1);
    assert_eq!(theirs.total(), 2);
}

/// A type replaced one-for-one still produced one: a flow is counted by identity, not by
/// comparing stock totals. "Built one, lost one" leaves the stock unchanged, and a ledger
/// that reads the stock would report no production at all.
#[test]
fn a_replacement_of_the_same_type_is_still_production() {
    let Some((mut sim, rules, americans, _russians)) = scene() else {
        return;
    };
    let old = sim
        .spawn_object("MTNK", "Americans", 8, 8, 0, &rules)
        .expect("spawns");
    let first = Snapshot::take(&sim, &rules, americans);
    let mut ledger = Ledger::new();
    ledger.observe(&first);

    // Built first, then the old one changes hands: no test-only removal, and the stock
    // count is still one at the end of the interval.
    sim.spawn_object("MTNK", "Americans", 9, 8, 0, &rules)
        .expect("spawns");
    let russians = sim.interner().get("Russians").expect("interned");
    sim.entities_mut_for_test().change_owner(old, russians);
    let second = Snapshot::take(&sim, &rules, americans);
    let movement = ledger.observe(&second);
    let ours = movement.get(&americans).expect("reported");

    assert_eq!(
        ours.produced(),
        vec![("MTNK".to_string(), 1)],
        "one was built, even though the stock count did not grow"
    );
    assert_eq!(
        ours.transferred_out.len(),
        1,
        "and the old one changed hands"
    );
    assert_eq!(ledger.produced().get("MTNK"), Some(&1));
    assert_eq!(
        ledger.holding(americans).expect("held").count_of("MTNK"),
        1,
        "the stock is still one"
    );
}
