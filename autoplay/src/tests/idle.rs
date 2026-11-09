//! The task board: which units AutoPlay has not given work to.
//!
//! The engine gives every fresh unit a default mission, so "idle" cannot be read from the
//! engine's mission state. The authority is AutoPlay's own assignment record, and these
//! cases pin the four things that record has to get right: a fresh unit needs work, only
//! this house's units are ours to give work to, switched off means no claims, and a
//! released unit must be reported again.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::control::ControlState;
use crate::idle::TaskBoard;
use crate::world::Snapshot;

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x1D1E);
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
    let ours = sim.spawn_object("MTNK", "Americans", 8, 8, 0, &rules)?;
    let theirs = sim.spawn_object("MTNK", "Russians", 20, 20, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    Some((sim, rules, americans, ours, theirs))
}

/// A unit nobody has given a task is the gap Phase 2 has to close.
#[test]
fn a_fresh_unit_nobody_ordered_is_reported_unassigned() {
    let Some((sim, rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    assert_eq!(TaskBoard::new().unassigned(&snapshot, &control), vec![ours]);
}

/// Only this house's units are AutoPlay's to give work to.
#[test]
fn another_houses_unit_is_never_reported() {
    let Some((sim, rules, americans, _, theirs)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    assert!(
        !TaskBoard::new()
            .unassigned(&snapshot, &control)
            .contains(&theirs)
    );
}

/// Switched off means no work is claimed, so nothing is reported.
#[test]
fn with_autoplay_off_nothing_is_reported() {
    let Some((sim, rules, americans, _, _)) = scene() else {
        return;
    };
    let control = ControlState::disabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    assert!(TaskBoard::new().unassigned(&snapshot, &control).is_empty());
}

/// Assigning stops the report, and releasing brings it back - so a unit that loses its
/// task, changes hands or is destroyed is not left silently "busy" forever.
#[test]
fn an_assigned_unit_is_not_reported_and_a_released_one_is_again() {
    let Some((sim, rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let mut board = TaskBoard::new();

    board.assign(ours);
    assert!(board.has(ours));
    assert!(!board.unassigned(&snapshot, &control).contains(&ours));

    board.release(ours);
    assert!(!board.has(ours));
    assert_eq!(board.unassigned(&snapshot, &control), vec![ours]);
}
