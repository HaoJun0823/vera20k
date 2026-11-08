//! Duplicate suppression, and the retry it must not block.
//!
//! The interesting half of this module is not the suppression - dropping an identical order
//! is trivial - but the three ways it must *not* fire. A planner that replans every frame is
//! correct behaviour being throttled; a planner that replans after a failure is the plan's
//! own B3 requirement; and a unit that has finished its order and is asked again is receiving
//! a new instruction, not a duplicate. Get any of those wrong and a stuck unit stays stuck.

use vera20k_core::rules::mission_data::MissionType;
use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::mission::state::MissionId;
use vera20k_core::sim::runtime::{SimResources, SimRuntime};
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter};
use crate::control::{ControlState, Subsystem};
use crate::dispatch::{Dispatcher, Suppression};
use crate::executor::{Execution, Executor};

fn scene() -> Option<(SimRuntime, RuleSet, InternedId, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0xD15B);
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
    let ours = sim.spawn_object("MTNK", "Americans", 40, 40, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    let mut runtime = SimRuntime::from_simulation(sim);
    runtime.resources = SimResources::new(rules.clone());
    Some((runtime, rules, americans, ours))
}

fn move_order(id: u64, from: (u16, u16)) -> Command {
    Command::AttackMove {
        entity_id: id,
        target: None,
        destination: Some(TargetKind::Cell(from.0 + 2, from.1)),
    }
}

/// The ordinary case: the same order on a later frame, for a unit still doing it.
#[test]
fn an_order_the_unit_is_still_running_is_suppressed() {
    let Some((mut runtime, _rules, americans, ours)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let from = crate::observe::cell_of(&runtime.simulation, ours).expect("has a cell");
    let payload = move_order(ours, from);
    let move_mission = MissionId::from_known(MissionType::Move);

    let Admission::Allowed(validated) = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        payload.clone(),
    ) else {
        panic!("admitted");
    };
    assert_eq!(
        Executor::submit(&mut runtime, &validated),
        Execution::Committed
    );
    // Let the mission host promote it, so the unit is genuinely running the order.
    let _ = Executor::submit_idle(&mut runtime);

    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(ours, payload.clone(), 0, move_mission);
    // Only suppress if it really is still running it; a scene that did not promote would
    // make this assertion meaningless, so state the precondition.
    let still = crate::handover::order_still_stands(&runtime.simulation, ours, move_mission);
    assert!(still, "precondition: the unit is running the move");

    match dispatcher.repeat_of(&runtime.simulation, ours, &payload, 5) {
        Some(Suppression::StillRunning) => {}
        other => panic!("expected StillRunning, got {other:?}"),
    }
    dispatcher.note_suppressed(Suppression::StillRunning);
    assert_eq!(dispatcher.suppressed_still_running(), 1);
    assert_eq!(dispatcher.suppressed_same_frame(), 0);
}

/// A caller looping within one frame is the worst case: it can fill the queue before any
/// world change is possible.
#[test]
fn an_order_repeated_on_the_same_frame_is_suppressed() {
    let Some((runtime, _rules, americans, ours)) = scene() else {
        return;
    };
    let _ = americans;
    let from = crate::observe::cell_of(&runtime.simulation, ours).expect("has a cell");
    let payload = move_order(ours, from);
    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(ours, payload.clone(), 7, MissionId::NONE);

    assert_eq!(
        dispatcher.repeat_of(&runtime.simulation, ours, &payload, 7),
        Some(Suppression::SameFrame),
        "the same frame cannot have observed the effect of the first order"
    );
    // On the next frame the frame check no longer applies.
    assert_ne!(
        dispatcher.repeat_of(&runtime.simulation, ours, &payload, 8),
        Some(Suppression::SameFrame)
    );
}

/// The case this module exists to protect: a failed order must be re-sendable.
#[test]
fn a_failed_order_may_be_sent_again() {
    let Some((mut runtime, _rules, americans, ours)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let from = crate::observe::cell_of(&runtime.simulation, ours).expect("has a cell");
    let payload = move_order(ours, from);
    let move_mission = MissionId::from_known(MissionType::Move);

    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(ours, payload.clone(), 0, move_mission);
    // Before any failure the order is in force and the unit is running it.
    assert_eq!(
        dispatcher.repeat_of(&runtime.simulation, ours, &payload, 5),
        Some(Suppression::StillRunning)
    );

    // The world says it did not take. Now the same order is replanning, not duplication.
    dispatcher.note_failed(ours);
    assert_eq!(
        dispatcher.repeat_of(&runtime.simulation, ours, &payload, 5),
        None,
        "a failed order may be retried with the same payload"
    );

    // And the retry actually goes out.
    let Admission::Allowed(validated) = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        payload.clone(),
    ) else {
        panic!("still admitted: suppression is a dispatch concern, not an authority one");
    };
    assert_eq!(
        Executor::submit(&mut runtime, &validated),
        Execution::Committed
    );
}

/// A different payload is never a duplicate, however similar.
#[test]
fn a_different_destination_is_a_new_order() {
    let Some((runtime, _rules, _americans, ours)) = scene() else {
        return;
    };
    let from = crate::observe::cell_of(&runtime.simulation, ours).expect("has a cell");
    let first = move_order(ours, from);
    let other = Command::AttackMove {
        entity_id: ours,
        target: None,
        destination: Some(TargetKind::Cell(from.0, from.1 + 2)),
    };
    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(ours, first.clone(), 3, MissionId::NONE);

    assert_eq!(
        dispatcher.repeat_of(&runtime.simulation, ours, &other, 3),
        None,
        "a different destination is a different order even on the same frame"
    );
    assert_eq!(
        dispatcher.repeat_of(&runtime.simulation, ours, &first, 3),
        Some(Suppression::SameFrame),
        "and the identical one still is a duplicate"
    );
}

/// Asking a settled unit to do the same thing again is a new instruction.
#[test]
fn an_order_for_a_settled_unit_is_not_a_duplicate() {
    let Some((mut runtime, _rules, americans, ours)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let from = crate::observe::cell_of(&runtime.simulation, ours).expect("has a cell");
    let payload = move_order(ours, from);
    let move_mission = MissionId::from_known(MissionType::Move);

    let Admission::Allowed(validated) = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        payload.clone(),
    ) else {
        panic!("admitted");
    };
    assert_eq!(
        Executor::submit(&mut runtime, &validated),
        Execution::Committed
    );
    let _ = Executor::submit_idle(&mut runtime);

    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(ours, payload.clone(), 0, move_mission);
    // Walk it to its destination, so the mission ends and the unit is idle.
    for _ in 0..80 {
        let _ = Executor::submit_idle(&mut runtime);
        if !crate::handover::order_still_stands(&runtime.simulation, ours, move_mission) {
            break;
        }
    }

    assert_eq!(
        dispatcher.repeat_of(&runtime.simulation, ours, &payload, 90),
        None,
        "a finished order does not block the same order being issued again"
    );
}

/// A dispatcher belongs to a match; nothing carries into the next one.
#[test]
fn a_new_dispatcher_remembers_nothing() {
    let Some((runtime, _rules, _americans, ours)) = scene() else {
        return;
    };
    let from = crate::observe::cell_of(&runtime.simulation, ours).expect("has a cell");
    let payload = move_order(ours, from);
    let mut first = Dispatcher::new();
    first.note_issued(ours, payload.clone(), 0, MissionId::NONE);
    assert!(first.knows(ours));

    let second = Dispatcher::new();
    assert!(!second.knows(ours), "a new match starts with no history");
    assert_eq!(second.suppressed_same_frame(), 0);
    assert_eq!(second.suppressed_still_running(), 0);
    // A destructed or captured unit is forgotten, so a recycled id cannot inherit its orders.
    first.forget(ours);
    assert!(!first.knows(ours));
}
