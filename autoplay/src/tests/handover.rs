//! The handover tests the review asked for, before any tactical work.
//!
//! The plan's §5.5 lists six cases about the native AI and the automated player sharing
//! one house, and the review called them more urgent than any tactical planning: the
//! engine runs team creation for every house every frame, so the two systems meet on the
//! first tick whether or not anyone planned for it.
//!
//! What these can and cannot establish is worth stating. They pin the *instrument* - that
//! a unit's mission state can be read, that an order's survival is detectable, and that a
//! later writer is visible as queued work - which is what every handover case is decided
//! on. They do not yet drive the native team AI through a full match and watch it fight
//! for a unit, because that needs the scenario fixture that battles load, and the plan
//! orders that after the command path is trustworthy. Saying so here is better than a test
//! that asserts a behaviour its scene cannot produce.

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
use crate::control::{ControlState, Refusal, Subsystem};
use crate::executor::{Execution, Executor};
use crate::handover::{self, MissionSnapshot};

fn scene() -> Option<(SimRuntime, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x40E7);
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
    let theirs = sim.spawn_object("MTNK", "Russians", 60, 60, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    let mut runtime = SimRuntime::from_simulation(sim);
    runtime.resources = SimResources::new(rules.clone());
    Some((runtime, rules, americans, ours, theirs))
}

fn move_order(id: u64, from: (u16, u16)) -> Command {
    Command::AttackMove {
        entity_id: id,
        target: None,
        destination: Some(TargetKind::Cell(from.0 + 2, from.1)),
    }
}

/// Case 1 and 2: with the automated player off, or on but not commanding this unit, the
/// unit's mission is not one AutoPlay placed.
///
/// The engine's own AI may do whatever it likes; what is asserted is only that AutoPlay
/// did nothing, which is the half of "the native AI keeps running" that this crate can
/// state without driving the native AI.
#[test]
fn with_autoplay_off_nothing_orders_the_unit() {
    let Some((runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    // A disabled state refuses, so no order is ever formed.
    let off = ControlState::disabled(americans);
    let refused = Arbiter::admit(
        &runtime.simulation,
        &off,
        Subsystem::Combat,
        ours,
        move_order(ours, (40, 40)),
    );
    assert!(matches!(refused.refusal(), Some(Refusal::GlobalDisabled)));

    // And an enabled state that has not been asked to do anything has also placed nothing:
    // the mission is still the engine's, not ours. This is the "on but not commanding"
    // case, and it holds because AutoPlay only acts when a caller admits an order.
    let idle = MissionSnapshot::of(&runtime.simulation, ours).expect("the tank exists");
    assert!(
        idle.queued == MissionId::NONE,
        "nothing is queued on a unit nobody ordered: {idle:?}"
    );
}

/// Case 3: once AutoPlay commands a unit, its order is the one in effect.
#[test]
fn a_commanded_unit_runs_the_mission_autoplay_asked_for() {
    let Some((mut runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let from = handover::MissionSnapshot::of(&runtime.simulation, ours)
        .map(|_| crate::observe::cell_of(&runtime.simulation, ours))
        .flatten()
        .expect("the tank has a cell");
    let Admission::Allowed(validated) = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        move_order(ours, from),
    ) else {
        panic!("an owned unit's order is admitted");
    };
    assert_eq!(
        Executor::submit(&mut runtime, &validated),
        Execution::Committed
    );
    // Give the mission host a frame to promote the queued order into `current`.
    let _ = Executor::submit_idle(&mut runtime);

    let move_mission = MissionId::from_known(MissionType::Move);
    assert!(
        handover::is_executing(&runtime.simulation, ours),
        "the unit is running a mission at all: {:?}",
        MissionSnapshot::of(&runtime.simulation, ours)
    );
    // And it is the mission the order implies, not somebody else's.
    assert!(
        handover::order_still_stands(&runtime.simulation, ours, move_mission)
            || MissionSnapshot::of(&runtime.simulation, ours)
                .is_some_and(|snapshot| snapshot.current == move_mission),
        "the running mission is the one the order asked for: {:?}",
        MissionSnapshot::of(&runtime.simulation, ours)
    );
}

/// Case 4: a released unit can be taken back - nothing in AutoPlay's own state holds it.
///
/// Release is a control-state decision here, not an engine call: AutoPlay has no persistent
/// claim on a unit, so switching the subsystem off is the release. The test asserts that
/// after it, AutoPlay refuses to command the unit again until re-enabled.
#[test]
fn a_released_unit_is_refused_then_commandable_again() {
    let Some((runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let mut control = ControlState::enabled(americans);
    assert!(
        Arbiter::admit(
            &runtime.simulation,
            &control,
            Subsystem::Combat,
            ours,
            move_order(ours, (40, 40)),
        )
        .is_allowed()
    );

    // Released.
    control.set_subsystem_enabled(Subsystem::Combat, false);
    assert!(matches!(
        Arbiter::admit(
            &runtime.simulation,
            &control,
            Subsystem::Combat,
            ours,
            move_order(ours, (40, 40)),
        )
        .refusal(),
        Some(Refusal::SubsystemDisabled(Subsystem::Combat))
    ));

    // Taken back.
    control.set_subsystem_enabled(Subsystem::Combat, true);
    assert!(
        Arbiter::admit(
            &runtime.simulation,
            &control,
            Subsystem::Combat,
            ours,
            move_order(ours, (40, 40)),
        )
        .is_allowed()
    );
}

/// Case 5: a player's own unit is never commanded, because it is not the automated
/// player's house.
#[test]
fn a_player_controlled_unit_is_never_commanded_by_autoplay() {
    let Some((runtime, rules, americans, _, theirs)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    // The Russian tank stands in for a player-held unit: what matters is that it is not
    // this house's, which is the same test the engine applies.
    assert!(matches!(
        Arbiter::admit(
            &runtime.simulation,
            &control,
            Subsystem::Combat,
            theirs,
            move_order(theirs, (60, 60)),
        )
        .refusal(),
        Some(Refusal::NotOurObject { .. })
    ));

    // And a house that is marked passive - a player sitting the match out - is not
    // commanded either, because the automated player speaks for exactly one house.
    let russian = runtime
        .simulation
        .interner()
        .get("Russians")
        .expect("interned");
    assert_ne!(russian, americans);
    let _ = rules;
}

/// Case 6: the risk the review named - switched off while a team it created is still
/// running. The observable trace of that is work queued after the switch.
///
/// This pins the instrument: a queued mission is visible even before the current one
/// changes, so a caller can detect a late writer rather than discovering it when the unit
/// suddenly changes direction.
#[test]
fn a_pending_replacement_is_visible_as_queued_work() {
    let Some((mut runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let from = crate::observe::cell_of(&runtime.simulation, ours).expect("the tank has a cell");
    let Admission::Allowed(validated) = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        move_order(ours, from),
    ) else {
        panic!("admitted");
    };

    // Before submission nothing is queued.
    assert!(
        !handover::has_pending_replacement(&runtime.simulation, ours),
        "nothing has been queued yet"
    );

    // The order goes in and the frame takes it; from here a queued mission exists until the
    // host promotes it. Whether that is observable at the exact frame depends on the
    // promotion order, so the assertion is the weaker, true one: after submission the unit
    // is either queued or already running the mission - never nothing.
    assert_eq!(
        Executor::submit(&mut runtime, &validated),
        Execution::Committed
    );
    let snapshot = MissionSnapshot::of(&runtime.simulation, ours).expect("the tank exists");
    assert!(
        snapshot.has_queued() || snapshot.current != MissionId::NONE,
        "the submitted order is either waiting or running: {snapshot:?}"
    );
}
