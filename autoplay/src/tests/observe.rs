//! Watching an order take effect, or fail to.
//!
//! The point of these tests is the distinction the module exists for: an order can be
//! admitted, the frame can commit, the object can stay alive, and nothing can have
//! happened. Every layer reporting success except the world is not a hypothetical - it
//! is what the first B1 probe measured.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::runtime::{SimResources, SimRuntime};
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter};
use crate::control::{ControlState, Subsystem};
use crate::executor::{Execution, Executor};
use crate::observe::{self, Outcome};

fn scene() -> Option<(SimRuntime, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x0B5E);
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

/// A committed order, watched to its end, reports movement rather than only admission.
#[test]
fn a_committed_order_is_observed_to_move_the_unit() {
    let Some((mut runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let Some(from) = observe::cell_of(&runtime.simulation, ours) else {
        panic!("the tank has a cell");
    };
    let Admission::Allowed(validated) = Arbiter::admit(
        &runtime.simulation,
        &control,
        Subsystem::Combat,
        ours,
        Command::AttackMove {
            entity_id: ours,
            target: None,
            destination: Some(TargetKind::Cell(from.0 + 2, from.1)),
        },
    ) else {
        panic!("an owned tank's order is admitted");
    };
    assert_eq!(
        Executor::submit(&mut runtime, &validated),
        Execution::Committed
    );

    match observe::watch_move(&mut runtime, ours, from, 80) {
        Outcome::Moved { from: f, to, .. } => {
            assert_eq!(f, from, "it reports the cell it started from");
            assert_ne!(to, from, "and a different cell it reached");
        }
        other => panic!("expected movement, got {}", other.describe()),
    }
}

/// The failure that is silent everywhere else: an order the engine accepts for an
/// object it will not act on.
#[test]
fn an_order_the_world_ignores_is_reported_as_stationary_not_as_success() {
    let Some((mut runtime, rules, americans, _, _)) = scene() else {
        return;
    };
    // Submit directly, bypassing the arbiter on purpose: this is the state a caller
    // reaches when it forgets the ownership check, which is what the arbiter exists to
    // prevent and what the observation layer has to be able to describe.
    let foreign = runtime
        .simulation
        .spawn_object("MTNK", "Russians", 70, 70, 0, &rules)
        .expect("the third tank spawns");
    let from = observe::cell_of(&runtime.simulation, foreign).expect("it has a cell");
    let due = runtime.simulation.session().tick().saturating_add(1);
    let envelope = vera20k_core::sim::command::CommandEnvelope::new(
        americans,
        due,
        Command::AttackMove {
            entity_id: foreign,
            target: None,
            destination: Some(TargetKind::Cell(from.0 + 2, from.1)),
        },
    );
    let mut first = runtime
        .advance_frame(
            std::slice::from_ref(&envelope),
            vera20k_core::headless_scenario::SIM_TICK_MS,
            vera20k_core::sim::world::TickLane::Ordinary,
            vera20k_core::sim::world::FrameEffects::default(),
        )
        .expect("the frame runs");
    assert_eq!(
        first.take_admitted_commands().len(),
        1,
        "the engine admits it, which is why admission cannot be the success signal"
    );

    let outcome = observe::watch_move(&mut runtime, foreign, from, 80);
    assert_eq!(
        outcome,
        Outcome::Stationary {
            at: from,
            frames: 80
        },
        "the world ignored the order and the observation says so"
    );
    assert!(
        !outcome.moved(),
        "and it does not claim the order worked: {}",
        outcome.describe()
    );
}

/// An object destroyed while the window runs is reported apart from one that stayed put.
#[test]
fn an_object_that_disappears_is_reported_as_gone() {
    let Some((mut runtime, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let from = observe::cell_of(&runtime.simulation, ours).expect("it has a cell");
    // Remove the subject before watching. `Outcome::Gone` is a result, not a failure to
    // observe, and a planner must not treat it as "did not move, try again".
    runtime.simulation.entities_mut_for_test().remove(ours);
    let _ = americans;
    let outcome = observe::watch_move(&mut runtime, ours, from, 10);
    assert!(
        matches!(outcome, Outcome::Gone { .. }),
        "expected Gone, got {}",
        outcome.describe()
    );
}
