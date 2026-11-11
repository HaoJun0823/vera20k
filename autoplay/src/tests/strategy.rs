//! Layer 1: producing, building, placing and using abilities.
//!
//! These scenes start from a real `Snapshot::take` and a real `RuleSet`, so the
//! type ids and rule facts come from the production reader rather than a hand-built
//! second copy of the rules.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::control::ControlState;
use crate::strategy;
use crate::world::Snapshot;

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x5EED);
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
    let mcv = sim.spawn_object("AMCV", "Americans", 8, 8, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    Some((sim, rules, americans, mcv))
}

/// Switched off: layer 1 must not plan anything at all.
#[test]
fn a_disabled_house_plans_nothing() {
    let Some((sim, rules, americans, _)) = scene() else {
        return;
    };
    let control = ControlState::disabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    assert!(strategy::plan(&sim, &snapshot, &control, &rules).is_none());
}

/// With no construction yard, the first strategic order deploys the MCV -
/// without a yard every QueueProduction is refused by FindFactory.
#[test]
fn an_mcv_is_deployed_before_any_production() {
    let Some((sim, rules, americans, mcv)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let order = strategy::plan(&sim, &snapshot, &control, &rules)
        .expect("a house with an MCV and no yard has work to do");
    assert_eq!(order.reason, "deploy-mcv");
    assert!(matches!(
        order.command,
        Command::DeployMcv { entity_id } if entity_id == mcv
    ));
    assert_eq!(order.anchor, mcv);
}
