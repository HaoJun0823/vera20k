//! The actuation path on the retail rival arena: the host's gate and envelope,
//! the scripted policy's release schedule, and the observation reading the
//! owners it claims to read.

use crate::rules::ruleset::RuleSet;
use crate::sim::autoplay::host::AutoPlayHost;
use crate::sim::autoplay::observation::AutoPlayView;
use crate::sim::autoplay::policy::AutoPlayPolicy;
use crate::sim::autoplay::scripted_policy::{ScriptStep, ScriptedPolicy};
use crate::sim::command::{Command, CommandEnvelope};
use crate::sim::world::Simulation;

/// The 32x32 arena with its two houses — `Americans` human, `Russians` a
/// computer house — and retail rules, needed to spawn real object types. None
/// without `RA2_DIR`, as the rest of the suite does.
fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
    let retail = crate::rules::retail_ini_fixture::retail_battle_rules()?;
    let rules = retail.rules;
    let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
    sim.intern_rule_type_ids(&rules);
    sim.resolve_type_handles(&rules);
    Some((sim, rules))
}

/// The same arena on synthetic rules, for what needs no object type: it runs
/// everywhere, so the gate and the observation stay covered without RA2.
fn synthetic_arena(seed: u64) -> (Simulation, RuleSet) {
    let rules = RuleSet::from_ini(&crate::sim::arena_fixture::clear_ground_ini(""))
        .expect("synthetic rules");
    let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
    sim.intern_rule_type_ids(&rules);
    sim.resolve_type_handles(&rules);
    (sim, rules)
}

fn american(sim: &mut Simulation) -> crate::sim::intern::InternedId {
    sim.interner.intern("Americans")
}

/// A host for `Americans` playing `script`.
fn host(sim: &mut Simulation, script: Vec<(u64, Command)>) -> AutoPlayHost {
    AutoPlayHost::new(
        Box::new(ScriptedPolicy::from_pairs(script)),
        american(sim),
        usize::MAX,
    )
}

#[test]
fn host_queues_commands_with_the_house_owner_and_the_current_tick() {
    let Some((mut sim, rules)) = arena(0xA17) else {
        return;
    };
    let tank = sim
        .spawn_object("MTNK", "Americans", 16, 16, 64, &rules)
        .expect("an American tank");
    let mut host = host(&mut sim, vec![(0, Command::Stop { entity_id: tank })]);

    host.step(&mut sim);

    let queued = sim.pending_command_snapshot();
    assert_eq!(queued.len(), 1, "one admitted command reaches the queue");
    assert_eq!(queued[0].owner, american(&mut sim));
    assert_eq!(queued[0].execute_tick, sim.session().tick());
    assert_eq!(queued[0].payload, Command::Stop { entity_id: tank });
    assert_eq!(host.rejected(), 0);
}

#[test]
fn gate_rejects_an_actor_the_house_does_not_own() {
    let Some((mut sim, rules)) = arena(0xA17) else {
        return;
    };
    let theirs = sim
        .spawn_object("MTNK", "Russians", 18, 16, 64, &rules)
        .expect("a Russian tank");
    let mut host = host(&mut sim, vec![(0, Command::Stop { entity_id: theirs })]);

    host.step(&mut sim);

    assert!(
        sim.pending_command_snapshot().is_empty(),
        "an order onto another house's object never reaches the queue"
    );
    assert_eq!(host.rejected(), 1);
}

#[test]
fn gate_rejects_a_command_naming_no_live_actor() {
    let (mut sim, _rules) = synthetic_arena(0xA17);
    let mut host = host(&mut sim, vec![(0, Command::Stop { entity_id: 0xDEAD })]);

    host.step(&mut sim);

    assert!(
        sim.pending_command_snapshot().is_empty(),
        "an order onto an id no object holds is refused"
    );
    assert_eq!(host.rejected(), 1);
}

#[test]
fn gate_passes_house_scoped_commands_without_an_actor() {
    let (mut sim, _rules) = synthetic_arena(0xA17);
    let power_plant = sim.interner.intern("GAPOWR");
    let mut host = host(
        &mut sim,
        vec![(
            0,
            Command::QueueProduction {
                type_id: power_plant,
            },
        )],
    );

    host.step(&mut sim);

    assert_eq!(
        sim.pending_command_snapshot().len(),
        1,
        "production names no actor and is admitted"
    );
}

#[test]
fn host_issues_nothing_while_input_is_locked() {
    let Some((mut sim, rules)) = arena(0xA17) else {
        return;
    };
    let tank = sim
        .spawn_object("MTNK", "Americans", 16, 16, 64, &rules)
        .expect("an American tank");
    let mut host = host(&mut sim, vec![(0, Command::Stop { entity_id: tank })]);
    sim.session.set_input_locked_for_test(true);

    host.step(&mut sim);

    assert!(
        sim.pending_command_snapshot().is_empty(),
        "a locked session takes no agent commands"
    );
}

#[test]
fn the_per_frame_cap_drops_the_overflow() {
    let Some((mut sim, rules)) = arena(0xA17) else {
        return;
    };
    let tank = sim
        .spawn_object("MTNK", "Americans", 16, 16, 64, &rules)
        .expect("an American tank");
    let owner = american(&mut sim);
    let mut host = AutoPlayHost::new(
        Box::new(ScriptedPolicy::from_pairs([
            (0, Command::Stop { entity_id: tank }),
            (0, Command::Scatter { entity_id: tank }),
            (
                0,
                Command::Guard {
                    entity_id: tank,
                    target: None,
                },
            ),
        ])),
        owner,
        2,
    );

    host.step(&mut sim);

    assert_eq!(
        sim.pending_command_snapshot().len(),
        2,
        "the third command of the frame is dropped"
    );
    assert_eq!(host.rejected(), 1);
}

#[test]
fn the_scripted_policy_releases_each_step_once_at_its_frame() {
    let Some((mut sim, rules)) = arena(0xA17) else {
        return;
    };
    let tank = sim
        .spawn_object("MTNK", "Americans", 16, 16, 64, &rules)
        .expect("an American tank");
    let mut agent = ScriptedPolicy::new(vec![
        ScriptStep {
            frame: 5,
            command: Command::Stop { entity_id: tank },
        },
        ScriptStep {
            frame: 5,
            command: Command::Scatter { entity_id: tank },
        },
        ScriptStep {
            frame: 9,
            command: Command::Guard {
                entity_id: tank,
                target: None,
            },
        },
    ]);
    let owner = american(&mut sim);

    // Out of order input is sorted, and same-frame steps keep their given order.
    assert_eq!(agent.pending().len(), 3);
    assert!(agent.decide(&AutoPlayView::new(&sim, owner)).is_empty());
    assert_eq!(sim.session().tick(), 0);

    // Frame 5 releases both of its steps, once, in script order.
    advance_to(&mut sim, &rules, 5);
    assert_eq!(sim.session().tick(), 5);
    let released = agent.decide(&AutoPlayView::new(&sim, owner));
    assert_eq!(
        released,
        vec![
            Command::Stop { entity_id: tank },
            Command::Scatter { entity_id: tank }
        ]
    );
    assert!(
        agent.decide(&AutoPlayView::new(&sim, owner)).is_empty(),
        "a step is released once, not every frame"
    );

    // Frame 9 releases the last one.
    advance_to(&mut sim, &rules, 9);
    assert_eq!(
        agent.decide(&AutoPlayView::new(&sim, owner)),
        vec![Command::Guard {
            entity_id: tank,
            target: None
        }]
    );
    assert!(agent.pending().is_empty());
}

/// The host stamps a command with the tick it stepped on, and the frame drains
/// everything due at `execute_tick <= tick + 1`. Which frame's drain picks it
/// up is easy to state wrongly, so pin it on a real frame loop.
#[test]
fn a_host_issued_command_is_drained_by_the_frame_that_follows_its_step() {
    let Some((mut sim, rules)) = arena(0xA17) else {
        return;
    };
    let tank = sim
        .spawn_object("MTNK", "Americans", 16, 16, 64, &rules)
        .expect("an American tank");
    let owner = american(&mut sim);
    let mut host = AutoPlayHost::new(
        Box::new(ScriptedPolicy::from_pairs([(
            5,
            Command::Stop { entity_id: tank },
        )])),
        owner,
        usize::MAX,
    );

    let mut drained_per_frame = Vec::new();
    for _ in 0..8 {
        host.step(&mut sim);
        let due = sim.take_due_commands();
        drained_per_frame.push(due.len());
        sim.advance_tick(&due, Some(&rules), None, None, 33);
    }

    assert_eq!(
        drained_per_frame,
        [0, 0, 0, 0, 0, 1, 0, 0],
        "the step on tick 5 is drained by that same frame, not the next one"
    );
}

#[test]
fn observation_reads_the_wallet_and_power_from_their_owners() {
    let (mut sim, _rules) = synthetic_arena(0xA17);
    let owner = american(&mut sim);
    let view = AutoPlayView::new(&sim, owner);

    assert_eq!(view.house(), owner);
    assert_eq!(view.tick(), sim.session().tick());
    assert_eq!(
        view.credits(),
        sim.houses().get(&owner).unwrap().economy().credits(),
        "credits is the house wallet, not a presentation value"
    );
    let state = sim.power_states().get(&owner);
    assert_eq!(view.power_output(), state.map_or(0, |s| s.total_output()));
    assert_eq!(view.power_drain(), state.map_or(0, |s| s.total_drain()));
    assert_eq!(
        view.power_balance(),
        view.power_output().wrapping_sub(view.power_drain())
    );
}

#[test]
fn observation_lists_only_the_houses_own_objects() {
    let Some((mut sim, rules)) = arena(0xA17) else {
        return;
    };
    let tank = sim
        .spawn_object("MTNK", "Americans", 16, 16, 64, &rules)
        .expect("an American tank");
    let theirs = sim
        .spawn_object("MTNK", "Russians", 18, 16, 64, &rules)
        .expect("a Russian tank");
    let owner = american(&mut sim);

    let view = AutoPlayView::new(&sim, owner);
    let ids: Vec<u64> = view.own_entities().map(|entity| entity.entity_id).collect();
    assert!(ids.contains(&tank));
    assert!(
        !ids.contains(&theirs),
        "the rival house's tank is not visible"
    );

    let mine = view
        .own_entities()
        .find(|entity| entity.entity_id == tank)
        .expect("the tank in the view");
    assert_eq!(mine.cell, (16, 16));
    assert!(
        view.max_health(&rules, mine.type_id)
            .is_some_and(|hp| hp > 0),
        "strength comes from the frozen rules the object was built with"
    );
    assert!(view.is_ground_walkable(16, 17), "the arena is flat ground");
}

/// The queue is the authority; this only counts what the host has handed it.
#[test]
fn a_house_with_no_registration_issues_nothing() {
    let (mut sim, _rules) = synthetic_arena(0xA17);
    let stranger = sim.interner.intern("Stranger");
    let mut host = AutoPlayHost::new(
        Box::new(ScriptedPolicy::from_pairs([(0, Command::Cheer)])),
        stranger,
        usize::MAX,
    );

    host.step(&mut sim);

    assert!(sim.pending_command_snapshot().is_empty());
}

/// Drains the queue the way the production frame loop does, up to `tick`.
fn advance_to(sim: &mut Simulation, rules: &RuleSet, tick: u64) {
    while sim.session().tick() < tick {
        let _: Vec<CommandEnvelope> = sim.take_due_commands();
        sim.advance_tick(&[], Some(rules), None, None, 33);
    }
}
