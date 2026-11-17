//! The assist through the real pipeline.
//!
//! The pure tests pin the rule; this one pins the plumbing: a brain that names
//! the right pairs but never gets them out of the Outbox is not an assist, it is
//! an intention. Here the board admits the orders, the simulation executes
//! them, and the intruder's health is the answer.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::control::board::ControlBoard;
use vera20k_core::sim::control::observation::Observation;
use vera20k_core::sim::intern::InternedId;

use crate::assist::Assist;

fn arena() -> Option<(vera20k_core::sim::world::Simulation, RuleSet, InternedId, InternedId)> {
    let retail = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?;
    let rules: RuleSet = retail.rules;
    let mut sim = vera20k_core::sim::arena_fixture::rival_arena(&rules, 0xA17);
    sim.intern_rule_type_ids(&rules);
    sim.resolve_type_handles(&rules);
    let ours = sim.intern("Americans");
    let theirs = sim
        .houses()
        .keys()
        .copied()
        .find(|house| *house != ours)?;
    Some((sim, rules, ours, theirs))
}

#[test]
fn an_intruder_at_the_door_is_shot_through_the_real_board() {
    let Some((mut sim, rules, ours, theirs)) = arena() else {
        return;
    };
    let _base = sim
        .spawn_object("GAPOWR", "Americans", 6, 6, 0, &rules)
        .expect("a base");
    let _soldier = sim
        .spawn_object("MTNK", "Americans", 9, 6, 0, &rules)
        .expect("a soldier");
    let theirs_name = sim.interner().resolve(theirs).to_owned();
    let intruder = sim
        .spawn_object("HTNK", &theirs_name, 11, 6, 0, &rules)
        .expect("an intruder");

    let mut board = ControlBoard::new(Box::new(Assist), ours, 512);
    let mut hurt = false;
    for _ in 0..400 {
        board.step(&mut sim, &rules);
        let _ = sim.take_due_commands();
        sim.advance_tick(&[], Some(&rules), None, None, 33);
        let observation = Observation::new(&sim, ours);
        match observation.health(intruder) {
            None => {
                hurt = true;
                break;
            }
            Some(current) if current < 400 => {
                hurt = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        hurt,
        "an intruder standing five cells from the base was never shot; rejected={}",
        board.rejected()
    );
}