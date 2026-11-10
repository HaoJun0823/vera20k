//! The per-frame decision: one order at most, and none when there is nothing new to do.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::runtime::{SimResources, SimRuntime};
use vera20k_core::sim::world::Simulation;

use crate::control::ControlState;
use crate::dispatch::Dispatcher;
use crate::executor::{Execution, Executor};
use crate::frame::{OrderKind, plan_frame};
use crate::idle::TaskBoard;
use crate::world::Snapshot;

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64, u64)> {
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
    let theirs = sim.spawn_object("MTNK", "Russians", 20, 20, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    Some((sim, rules, americans, ours, theirs))
}

/// A scene that can actually run a frame, so an admitted order leaves the unit in the
/// mission the dispatcher expects. A pure `Simulation` cannot promote a queued mission.
fn runtime_scene() -> Option<(SimRuntime, RuleSet, InternedId, u64, u64)> {
    let (sim, rules, americans, ours, theirs) = scene()?;
    let mut runtime = SimRuntime::from_simulation(sim);
    runtime.resources = SimResources::new(rules.clone());
    Some((runtime, rules, americans, ours, theirs))
}

/// Send one frame's guard order and record it, leaving the unit running that mission.
fn issue_guard(
    runtime: &mut SimRuntime,
    rules: &RuleSet,
    americans: InternedId,
    ours: u64,
    tasks: &mut TaskBoard,
    dispatcher: &mut Dispatcher,
) {
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&runtime.simulation, rules, americans);
    let plan = plan_frame(&runtime.simulation, &snapshot, &control, tasks, dispatcher)
        .expect("an order is planned");
    assert_eq!(
        plan.kind,
        OrderKind::Guard,
        "precondition: no enemy in sight"
    );
    assert_eq!(
        Executor::submit(runtime, &plan.validated),
        Execution::Committed
    );
    dispatcher.note_issued(ours, plan.payload, plan.frame, plan.expected);
    tasks.assign(ours, plan.frame);
    // Let the mission host promote the queued order into the unit's current mission.
    let _ = Executor::submit_idle(runtime);
}

/// A unit with no task is the one that gets an order; with nothing revealed it holds.
#[test]
fn a_unit_with_no_task_gets_one_order() {
    let Some((sim, rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();

    let plan = plan_frame(&sim, &snapshot, &control, &mut tasks, &mut dispatcher)
        .expect("an order is planned");
    assert_eq!(plan.validated.subject, ours);
    assert_eq!(plan.kind, OrderKind::Guard, "nothing is revealed yet");
    assert!(plan.endings.is_empty());
}

/// A visible enemy is attacked rather than held against.
#[test]
fn a_visible_enemy_is_attacked() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut snapshot = Snapshot::take(&sim, &rules, americans);
    snapshot
        .objects
        .get_mut(&theirs)
        .expect("the enemy exists")
        .visible = true;
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();

    let plan = plan_frame(&sim, &snapshot, &control, &mut tasks, &mut dispatcher)
        .expect("an order is planned");
    assert_eq!(plan.validated.subject, ours);
    assert_eq!(plan.kind, OrderKind::Attack);
}

/// An enemy the player cannot see is never targeted - that would be cheating.
#[test]
fn an_unseen_enemy_is_not_attacked() {
    let Some((sim, rules, americans, _ours, theirs)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    assert!(!snapshot.objects.get(&theirs).expect("exists").visible);
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();

    let plan = plan_frame(&sim, &snapshot, &control, &mut tasks, &mut dispatcher)
        .expect("an order is planned");
    assert_eq!(plan.kind, OrderKind::Guard);
}

/// A unit already holding a task must not get a second order on the same frame.
#[test]
fn a_unit_that_already_has_a_task_gets_no_second_order() {
    let Some((sim, rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();
    tasks.assign(ours, 0);

    assert!(plan_frame(&sim, &snapshot, &control, &mut tasks, &mut dispatcher).is_none());
}

/// Switched off: no order, no task endings, no state changed.
#[test]
fn with_autoplay_off_no_order_is_planned() {
    let Some((sim, rules, americans, _, _)) = scene() else {
        return;
    };
    let control = ControlState::disabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();

    assert!(plan_frame(&sim, &snapshot, &control, &mut tasks, &mut dispatcher).is_none());
    assert!(tasks.endings().is_empty());
}

/// Reassessment that changes nothing renews the task without re-sending the command.
/// This is the wishlist's "high-frequency orders stop the unit firing" symptom: the
/// 150-frame window expiring used to produce a fresh identical Guard every time.
#[test]
fn an_unchanged_order_renews_the_task_without_resending() {
    let Some((mut runtime, rules, americans, ours, _)) = runtime_scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();

    issue_guard(
        &mut runtime,
        &rules,
        americans,
        ours,
        &mut tasks,
        &mut dispatcher,
    );
    // A task window ending is what makes the unit unassigned again. `release` is the
    // same state change `finish(Reassess)` produces, without waiting 150 frames.
    tasks.release(ours);

    let snapshot = Snapshot::take(&runtime.simulation, &rules, americans);
    let second = plan_frame(
        &runtime.simulation,
        &snapshot,
        &control,
        &mut tasks,
        &mut dispatcher,
    );
    assert!(second.is_none(), "the same order must not be sent again");
    assert!(tasks.has(ours), "the task window is renewed, not dropped");
    assert_eq!(dispatcher.suppressed_still_running(), 1);
}

/// When the world changes, the same unit gets a genuinely new order.
#[test]
fn a_changed_order_is_not_suppressed() {
    let Some((mut runtime, rules, americans, ours, theirs)) = runtime_scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();

    issue_guard(
        &mut runtime,
        &rules,
        americans,
        ours,
        &mut tasks,
        &mut dispatcher,
    );
    tasks.release(ours);

    let mut snapshot = Snapshot::take(&runtime.simulation, &rules, americans);
    snapshot
        .objects
        .get_mut(&theirs)
        .expect("enemy exists")
        .visible = true;
    let plan = plan_frame(
        &runtime.simulation,
        &snapshot,
        &control,
        &mut tasks,
        &mut dispatcher,
    )
    .expect("a changed order goes through");
    assert_eq!(plan.kind, OrderKind::Attack);
    assert_eq!(dispatcher.suppressed_still_running(), 0);
}

/// A recorded failure must clear the way for the identical retry, or a stuck unit
/// stays stuck. This is the B3 requirement the duplicate filter must not break.
#[test]
fn a_failed_order_can_be_retried_with_the_same_payload() {
    let Some((mut runtime, rules, americans, ours, _)) = runtime_scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut tasks = TaskBoard::new();
    let mut dispatcher = Dispatcher::new();

    issue_guard(
        &mut runtime,
        &rules,
        americans,
        ours,
        &mut tasks,
        &mut dispatcher,
    );
    dispatcher.note_failed(ours);
    tasks.release(ours);

    let snapshot = Snapshot::take(&runtime.simulation, &rules, americans);
    let plan = plan_frame(
        &runtime.simulation,
        &snapshot,
        &control,
        &mut tasks,
        &mut dispatcher,
    )
    .expect("a failed order is retried");
    assert_eq!(plan.kind, OrderKind::Guard);
}
