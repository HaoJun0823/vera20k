//! The command path a policy outside the engine has to use, pinned as a test.
//!
//! Phase B1 of the plan asks for one vertical slice: read a unit, decide it may be
//! commanded, form an order, admit it, submit it, and confirm the unit moved. Three
//! things about that path were found by probing and are recorded here as regression
//! tests, because each one costs an hour to rediscover:
//!
//! 1. Commands reach a frame through the `commands` argument of `advance_frame`.
//!    `issue_local_command` feeds a FIFO that `HeadlessScenario::tick` never drains,
//!    since that method passes a hard-coded empty slice.
//! 2. `execute_tick` is compared against `tick + 1`. An envelope dated at the current
//!    tick is one frame early and never becomes due.
//! 3. A command naming an object the issuing house does not own is admitted and then
//!    silently dropped - no error, no refusal, nothing in the frame output. This is
//!    the condition the arbiter has to check, and it is why the first probe spent two
//!    runs concluding that movement did not work.

use super::{FrameEffects, Simulation, TickLane};
use crate::sim::combat::TargetKind;
use crate::sim::command::{Command, CommandEnvelope};
use crate::sim::house_state::HouseState;
use crate::sim::runtime::{SimResources, SimRuntime};

/// Two houses, an owned Grizzly and a cell for it to walk to.
///
/// Uses retail rules so the tank is a real tank: its speed, locomotor and mission
/// behaviour all come from the INI rather than from a fixture that could agree with
/// the test by construction.
fn owned_tank_scene() -> Option<(SimRuntime, crate::rules::ruleset::RuleSet, u64, (u16, u16))> {
    let rules = crate::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x51CE);
    crate::sim::arena_fixture::flat_ground(&mut sim, &rules);
    sim.intern_rule_type_ids(&rules);
    sim.resolve_type_handles(&rules);
    for (name, human) in [("Americans", true), ("Russians", false)] {
        let owner = sim.intern(name);
        let side = rules.country_side_index(name)?.0;
        let mut house = HouseState::new(owner, side, Some(owner), human, 0, 10);
        house.project_country_mults(&rules, sim.interner());
        sim.houses_mut_for_test().register(house);
    }
    let tank = sim.spawn_object("MTNK", "Americans", 40, 40, 0, &rules)?;
    // `from_simulation` binds empty resources; the frame needs the retail rules to
    // resolve anything, so the field is replaced with the ones this scene was built
    // from. Both fields are public, and a fixture that ran on empty rules would be
    // testing a different game from the one the probe measured.
    let mut runtime = SimRuntime::from_simulation(sim);
    runtime.resources = SimResources::new(rules.clone());
    Some((runtime, rules, tank, (40, 40)))
}

/// The whole slice: an order for an owned unit makes it move.
#[test]
fn a_command_for_an_owned_unit_moves_it() {
    let Some((mut runtime, _rules, tank, start)) = owned_tank_scene() else {
        return;
    };
    let target = (start.0 + 2, start.1);
    let owner = runtime
        .simulation
        .entities()
        .get(tank)
        .expect("the tank exists")
        .owner();
    // `execute_tick` is the frame the order becomes due; the scheduler compares it to
    // `tick + 1`, so it is dated one ahead of the current frame.
    let due = runtime.simulation.session().tick().saturating_add(1);
    let envelope = CommandEnvelope::new(
        owner,
        due,
        Command::AttackMove {
            entity_id: tank,
            target: None,
            destination: Some(TargetKind::Cell(target.0, target.1)),
        },
    );

    let mut first = runtime
        .advance_frame(
            std::slice::from_ref(&envelope),
            67,
            TickLane::Ordinary,
            FrameEffects::default(),
        )
        .expect("the frame runs");
    assert_eq!(
        first.take_admitted_commands().len(),
        1,
        "the frame takes the order"
    );

    let mut moved = false;
    for _ in 0..80 {
        runtime
            .advance_frame(&[], 67, TickLane::Ordinary, FrameEffects::default())
            .expect("the frame runs");
        let now = runtime
            .simulation
            .entities()
            .get(tank)
            .map(|entity| (entity.position.rx(), entity.position.ry()));
        if now.is_some_and(|cell| cell != start) {
            moved = true;
            break;
        }
    }
    assert!(moved, "an owned tank takes a move order and walks");
}

/// The condition the arbiter exists to check: ownership.
///
/// A command for someone else's object is admitted - the queue has no opinion about
/// it - and dispatch then ignores it. Writing this down matters because the failure is
/// silent: the frame reports the order as admitted, the object stays alive and in
/// place, and nothing distinguishes it from a unit that refused to move.
#[test]
fn a_command_for_an_unowned_unit_is_admitted_then_ignored() {
    let Some((mut runtime, rules, _tank, _)) = owned_tank_scene() else {
        return;
    };
    // Somebody else's tank, far from the player's.
    let foreign = runtime
        .simulation
        .spawn_object("MTNK", "Russians", 60, 60, 0, &rules)
        .expect("the second tank spawns");
    let foreign_start = {
        let entity = runtime.simulation.entities().get(foreign).expect("exists");
        (entity.position.rx(), entity.position.ry())
    };
    let americans = runtime
        .simulation
        .interner()
        .get("Americans")
        .expect("interned");
    let due = runtime.simulation.session().tick().saturating_add(1);
    // The envelope claims the American house while naming a Russian object.
    let envelope = CommandEnvelope::new(
        americans,
        due,
        Command::AttackMove {
            entity_id: foreign,
            target: None,
            destination: Some(TargetKind::Cell(foreign_start.0 + 2, foreign_start.1)),
        },
    );

    let mut first = runtime
        .advance_frame(
            std::slice::from_ref(&envelope),
            67,
            TickLane::Ordinary,
            FrameEffects::default(),
        )
        .expect("the frame runs");
    assert_eq!(
        first.take_admitted_commands().len(),
        1,
        "the queue takes it: admission says nothing about ownership"
    );

    for _ in 0..80 {
        runtime
            .advance_frame(&[], 67, TickLane::Ordinary, FrameEffects::default())
            .expect("the frame runs");
    }
    let now = runtime
        .simulation
        .entities()
        .get(foreign)
        .map(|entity| (entity.position.rx(), entity.position.ry()));
    assert_eq!(
        now,
        Some(foreign_start),
        "and dispatch ignores it, so the object never moves"
    );
}
