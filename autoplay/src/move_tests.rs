//! Does a `Goal::Move` carry a unit across the map?
//!
//! The scout probe in status.md gave units a move and they stayed inside a
//! seventeen-cell strip around the base. Two explanations fit: the order never
//! travels — something downstream of admission drops it — or the policy re-sends
//! it every term and the unit restarts from wherever it has got to. This test is
//! the first half: one unit, one order, no policy. If the unit arrives, the fault
//! is in the policy's re-ordering and not in the order.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::behavior::goal::Goal;
use vera20k_core::sim::behavior::order::Funnel;
use vera20k_core::sim::command::CommandEnvelope;
use vera20k_core::sim::control::observation::Observation;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

fn arena() -> Option<(Simulation, RuleSet)> {
    let retail = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?;
    let rules = retail.rules;
    let mut sim = vera20k_core::sim::arena_fixture::rival_arena(&rules, 0xA17);
    sim.intern_rule_type_ids(&rules);
    sim.resolve_type_handles(&rules);
    Some((sim, rules))
}

#[test]
fn a_single_move_order_carries_a_tank_across_the_arena() {
    let Some((mut sim, rules)) = arena() else {
        return;
    };
    let americans: InternedId = sim.intern("Americans");
    let tank = sim
        .spawn_object("MTNK", "Americans", 6, 6, 0, &rules)
        .expect("a tank");
    let target = (26, 26);

    // One order, through the same funnel a player's click and the policy both
    // use, and then the engine drives it. Nothing re-sends it.
    let funnel = Funnel::new(&sim, &rules, americans);
    let command = funnel
        .prepare(tank, Goal::Move { cell: target })
        .expect("a move is admissible");
    sim.queue_commands(vec![CommandEnvelope::new(
        americans,
        sim.session().tick(),
        command,
    )]);

    let mut closest = 999;
    let mut arrived_at = None;
    for frame in 0..1200 {
        let _ = sim.take_due_commands();
        sim.advance_tick(&[], Some(&rules), None, None, 33);
        let observation = Observation::new(&sim, americans);
        let Some(cell) = observation
            .own_entities()
            .find(|entity| entity.entity_id == tank)
            .map(|entity| entity.cell)
        else {
            break;
        };
        let away = i32::from(cell.0.abs_diff(target.0)).max(i32::from(cell.1.abs_diff(target.1)));
        closest = closest.min(away);
        if away <= 1 {
            arrived_at = Some(frame);
            break;
        }
    }
    assert!(
        arrived_at.is_some(),
        "a single Move must carry the tank to {target:?}; it got within {closest} cells"
    );
}

#[test]
fn a_single_attack_move_order_carries_a_tank_across_the_arena() {
    let Some((mut sim, rules)) = arena() else {
        return;
    };
    let americans: InternedId = sim.intern("Americans");
    let tank = sim
        .spawn_object("MTNK", "Americans", 6, 6, 0, &rules)
        .expect("a tank");
    let target = (26, 26);

    // The same single order as the test above, with the goal the policy uses for
    // a march to a cell.
    //
    // `Assault` on a `Target::Cell` maps to `Goal::AttackMove`, and three
    // measurements say the army does not move under it: five IFVs held
    // `job=Assault target=Cell((112, 52))` with `standing=7575` at `(91-95,
    // 26-32)`, only 67 of 4940 readings ever reached that destination, and
    // `MissionId(1)` appeared on 85 of 6041 readings — the army barely fired.
    //
    // This is the half that tells the two explanations apart: if one order, with
    // nothing re-sending it, does not carry the unit, the fault is in the goal and
    // not in the policy's re-ordering.
    let funnel = Funnel::new(&sim, &rules, americans);
    let command = funnel
        .prepare(tank, Goal::AttackMove { cell: target })
        .expect("an attack-move is admissible");
    sim.queue_commands(vec![CommandEnvelope::new(
        americans,
        sim.session().tick(),
        command,
    )]);

    let mut closest = 999;
    let mut arrived_at = None;
    for frame in 0..1200 {
        let _ = sim.take_due_commands();
        sim.advance_tick(&[], Some(&rules), None, None, 33);
        let observation = Observation::new(&sim, americans);
        let Some(cell) = observation
            .own_entities()
            .find(|entity| entity.entity_id == tank)
            .map(|entity| entity.cell)
        else {
            break;
        };
        let away = i32::from(cell.0.abs_diff(target.0)).max(i32::from(cell.1.abs_diff(target.1)));
        closest = closest.min(away);
        if away <= 1 {
            arrived_at = Some(frame);
            break;
        }
    }
    assert!(
        arrived_at.is_some(),
        "a single AttackMove must carry the tank to {target:?}; it got within {closest} cells"
    );
}
