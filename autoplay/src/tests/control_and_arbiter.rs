//! Control state and the arbiter, tested against a real engine world.
//!
//! Each test builds a scene the way a skirmish does - retail rules, two houses, a real
//! tank - because a fixture with synthetic rules would agree with the test by
//! construction and prove nothing about the engine's actual behaviour.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::runtime::{SimResources, SimRuntime};
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter};
use crate::control::{ControlState, Refusal, Subsystem};
use crate::executor::{Execution, Executor};

/// Two houses and one tank each, on flat ground, from retail rules.
fn scene() -> Option<(SimRuntime, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0xA17B);
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

fn move_to(entity_id: u64, rx: u16, ry: u16) -> Command {
    Command::AttackMove {
        entity_id,
        target: None,
        destination: Some(TargetKind::Cell(rx, ry)),
    }
}

/// The whole slice, through this crate's three modules: a validated order for an owned
/// unit commits, and the unit moves.
#[test]
fn an_admitted_order_for_an_owned_unit_commits_and_moves_it() {
    let Some((mut runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let start = {
        let entity = runtime.simulation.entities().get(ours).expect("exists");
        (entity.position.rx(), entity.position.ry())
    };
    let Admission::Allowed(validated) = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        move_to(ours, start.0 + 2, start.1),
    ) else {
        panic!("an owned unit's order should be admitted");
    };
    assert_eq!(
        Executor::submit(&mut runtime, &validated),
        Execution::Committed,
        "the frame commits the order"
    );

    let mut moved = false;
    for _ in 0..80 {
        let _ = Executor::submit_idle(&mut runtime);
        let now = runtime
            .simulation
            .entities()
            .get(ours)
            .map(|entity| (entity.position.rx(), entity.position.ry()));
        if now.is_some_and(|cell| cell != start) {
            moved = true;
            break;
        }
    }
    assert!(moved, "the order was committed and the tank walked");
}

/// The check that blocked B1 for two runs: an order naming somebody else's tank.
///
/// The engine would admit this and then ignore it, so the arbiter refusing here is the
/// only thing standing between a planner and a silent failure.
#[test]
fn an_order_for_a_foreign_object_is_refused_by_ownership() {
    let Some((runtime, _rules, americans, _, theirs)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let admission = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        theirs,
        move_to(theirs, 62, 60),
    );
    match admission {
        Admission::Refused(Refusal::NotOurObject { owner, house }) => {
            assert_eq!(house, americans);
            assert_ne!(owner, americans, "the refusal names the real holder");
        }
        other => panic!("expected an ownership refusal, got {other:?}"),
    }
}

/// A destroyed object is a planner bug, not a world change, and is reported apart.
#[test]
fn an_order_for_a_missing_object_says_so() {
    let Some((runtime, _rules, americans, _, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let admission = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        999_999,
        move_to(999_999, 40, 40),
    );
    assert!(matches!(
        admission.refusal(),
        Some(Refusal::NoSuchObject { id: 999_999 })
    ));
}

/// Switching the whole automated player off stops everything, including a subsystem
/// that was left enabled.
#[test]
fn the_global_switch_overrides_every_subsystem() {
    let Some((runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let mut control = ControlState::enabled(americans);
    control.set_global_enabled(false);
    assert!(!control.is_subsystem_enabled(Subsystem::Combat));
    assert!(!control.is_subsystem_enabled(Subsystem::Production));
    let admission = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        move_to(ours, 42, 40),
    );
    assert!(matches!(admission.refusal(), Some(Refusal::GlobalDisabled)));

    // Turning it back on restores the previous arrangement rather than enabling
    // everything, so a subsystem switched off beforehand stays off.
    control.set_subsystem_enabled(Subsystem::Ability, false);
    control.set_global_enabled(true);
    assert!(control.is_subsystem_enabled(Subsystem::Combat));
    assert!(
        !control.is_subsystem_enabled(Subsystem::Ability),
        "the ability switch was off before the global one went off and stays off"
    );
}

/// Subsystems run independently: production off does not stop combat.
#[test]
fn subsystems_are_independent() {
    let Some((runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let mut control = ControlState::enabled(americans);
    control.set_subsystem_enabled(Subsystem::Production, false);
    // A move is combat, so it still passes.
    assert!(
        Arbiter::admit(
            &runtime.simulation,
            &control,
            Subsystem::Combat,
            ours,
            move_to(ours, 42, 40),
        )
        .is_allowed()
    );
    // A production order would not.
    let refused = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Production,
        ours,
        move_to(ours, 42, 40),
    );
    assert!(matches!(
        refused.refusal(),
        Some(Refusal::SubsystemDisabled(Subsystem::Production))
    ));
}

/// A state that grants nothing grants nothing, and says which house it would have
/// spoken for so a log can tell two disabled houses apart.
#[test]
fn a_disabled_state_grants_nothing() {
    let Some((runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::disabled(americans);
    assert_eq!(control.house(), americans);
    assert!(!control.is_global_enabled());
    assert!(matches!(
        Arbiter::admit(
            &runtime.simulation,
            &control,
            Subsystem::Combat,
            ours,
            move_to(ours, 42, 40),
        )
        .refusal(),
        Some(Refusal::GlobalDisabled)
    ));
}
