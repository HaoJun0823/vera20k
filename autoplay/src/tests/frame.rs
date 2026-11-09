//! The per-frame decision: one order at most, and none when there is nothing to do.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::control::ControlState;
use crate::frame::plan_frame;
use crate::idle::TaskBoard;
use crate::world::Snapshot;

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0xF1A5);
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
    let americans = sim.interner().get("Americans")?;
    Some((sim, rules, americans, ours))
}

/// A unit with no task is the one that gets an order.
#[test]
fn a_unit_with_no_task_gets_one_order() {
    let Some((sim, rules, americans, ours)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let mut tasks = TaskBoard::new();

    let plan = plan_frame(&sim, &snapshot, &control, &mut tasks).expect("an order is planned");
    assert_eq!(plan.validated.subject, ours);
    assert!(plan.endings.is_empty());
}

/// A unit already holding a task must not get a second order on the same frame.
#[test]
fn a_unit_that_already_has_a_task_gets_no_second_order() {
    let Some((sim, rules, americans, ours)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let mut tasks = TaskBoard::new();
    tasks.assign(ours);

    assert!(plan_frame(&sim, &snapshot, &control, &mut tasks).is_none());
}

/// Switched off: no order, no task endings, no state changed.
#[test]
fn with_autoplay_off_no_order_is_planned() {
    let Some((sim, rules, americans, _)) = scene() else {
        return;
    };
    let control = ControlState::disabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let mut tasks = TaskBoard::new();

    assert!(plan_frame(&sim, &snapshot, &control, &mut tasks).is_none());
    assert!(tasks.endings().is_empty());
}
