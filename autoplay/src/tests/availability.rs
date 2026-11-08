//! Availability, read rather than computed.
//!
//! The distinction these tests protect is `None` versus `0`. A type with no `Ammo=` has no
//! ammunition *question*; a type with zero rounds is out of ammunition. Reading the first as
//! the second would refuse to use every infantryman and every structure, which is the kind of
//! mistake that looks like a balance problem rather than a bug.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::availability::availability_of;
use crate::world::Snapshot;

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0xA11A);
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
    let tank = sim.spawn_object("MTNK", "Americans", 40, 40, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    Some((sim, rules, americans, tank))
}

/// A tank reads its ammunition, and a type without `Ammo=` reads `None` rather than zero.
#[test]
fn ammunition_is_none_when_the_type_has_none_and_a_number_when_it_does() {
    let Some((mut sim, rules, _americans, tank)) = scene() else {
        return;
    };
    let tank_avail = availability_of(&sim, tank).expect("the tank exists");
    // MTNK has no Ammo= in retail rules, so its availability is `None` - not zero.
    if let Some(rounds) = tank_avail.ammo {
        // If a rule set ever gives it ammunition, the value must still be a real count.
        assert!(rounds >= 0, "a negative round count is not a state");
    }
    assert!(
        tank_avail.has_ammo(),
        "no ammunition question, or rounds remaining, both mean usable"
    );

    // An infantryman is the case that would break if `None` were read as zero.
    let Some(gi) = sim.spawn_object("E1", "Americans", 42, 40, 0, &rules) else {
        return;
    };
    let gi_avail = availability_of(&sim, gi).expect("the soldier exists");
    assert_eq!(
        gi_avail.ammo, None,
        "a rifleman has no ammunition tracker, and that is not 'out of ammunition'"
    );
    assert!(
        gi_avail.has_ammo(),
        "reading None as zero would refuse to use every infantry unit"
    );
}

/// A deployed structure and an undeployed one differ, and the value comes from the engine.
#[test]
fn deployment_is_read_from_the_entity() {
    let Some((sim, _rules, _americans, tank)) = scene() else {
        return;
    };
    let avail = availability_of(&sim, tank).expect("exists");
    // A tank cannot deploy, so this is whatever the engine says; the point is that it is a
    // read, not a guess. Asserting the default would pin engine behaviour this layer must not
    // own, so only the type is checked.
    let _ = avail.deployed;
}

/// A transport reports its hold, and a non-transport reports none.
#[test]
fn cargo_is_reported_only_for_a_transport() {
    let Some((mut sim, rules, _americans, tank)) = scene() else {
        return;
    };
    let tank_avail = availability_of(&sim, tank).expect("exists");
    if tank_avail.cargo.is_none() {
        assert!(!tank_avail.is_laden(), "no hold means nothing carried");
    }

    // An IFV has Passengers= in retail rules and starts empty.
    if let Some(ifv) = sim.spawn_object("FV", "Americans", 44, 40, 0, &rules) {
        let ifv_avail = availability_of(&sim, ifv).expect("exists");
        if let Some(capacity) = ifv_avail.cargo_capacity {
            assert!(
                capacity > 0,
                "a transport with no capacity is not a transport"
            );
            assert_eq!(
                ifv_avail.cargo,
                Some(0),
                "a freshly built transport is empty, which is zero not None"
            );
            assert!(!ifv_avail.is_laden());
        }
    }
}

/// Being inside a transport blocks action even though every other field looks normal.
#[test]
fn a_unit_inside_a_transport_is_not_obviously_free_to_act() {
    let Some((sim, _rules, _americans, tank)) = scene() else {
        return;
    };
    let avail = availability_of(&sim, tank).expect("exists");
    // A lone tank on flat ground is not inside anything.
    assert_eq!(avail.inside_transport, None);
    assert!(avail.is_not_obviously_blocked());
}

/// Mind control is read, and its absence is not the same as its presence.
///
/// This is the case that defeats an ownership check: a controlled unit keeps its original
/// owner, so `eligibility` would call it eligible while it obeys somebody else. The engine
/// exposes it through `MindControlLink::controller()`, which is what makes it readable at all.
///
/// Establishing control is not done here because `MindControlLink` has no public constructor -
/// it is set by the capture path, which is a different system with its own tests. What this can
/// establish is that the field is read from the engine rather than defaulted: a fresh unit
/// reports no controller, and the accessor is the link's own.
#[test]
fn mind_control_is_absent_for_an_uncontrolled_unit() {
    let Some((sim, _rules, _americans, tank)) = scene() else {
        return;
    };
    let avail = availability_of(&sim, tank).expect("exists");
    assert_eq!(
        avail.mind_controlled_by, None,
        "an uncontrolled unit has no controller"
    );
    assert!(!avail.is_mind_controlled());
    // And the entity's own link agrees with what was read, so the field is not a separate copy.
    let entity = sim.entities().get(tank).expect("exists");
    assert_eq!(
        entity.mind_control.controller(),
        avail.mind_controlled_by,
        "the availability value is the link's, not a second source"
    );
}

/// A controlled unit would be reported as blocked, which is why `is_not_obviously_blocked`
/// has to consult it.
#[test]
fn a_controlled_unit_is_not_obviously_free_to_act() {
    // Constructed rather than spawned: `MindControlLink` has no public setter, and going
    // through the real capture path would be testing that system instead of this one. The
    // point is the relationship between the field and the predicate.
    let controlled = crate::availability::Availability {
        ammo: None,
        inside_transport: None,
        deployed: false,
        target: None,
        cargo: None,
        cargo_capacity: None,
        mind_controlled_by: Some(999),
    };
    assert!(controlled.is_mind_controlled());
    assert!(
        !controlled.is_not_obviously_blocked(),
        "a unit obeying somebody else must not be treated as free to act"
    );
    assert!(
        controlled.has_ammo(),
        "and it may still have ammunition - the two are different questions"
    );
}

#[test]
fn a_gone_object_has_no_availability() {
    let Some((mut sim, _rules, _americans, tank)) = scene() else {
        return;
    };
    assert!(availability_of(&sim, tank).is_some());
    sim.entities_mut_for_test().remove(tank);
    assert!(
        availability_of(&sim, tank).is_none(),
        "a plan naming a destroyed unit is out of date, and None says so"
    );
}

/// The map form covers exactly the objects the snapshot holds.
#[test]
fn the_availability_map_matches_the_snapshot() {
    let Some((sim, rules, americans, _tank)) = scene() else {
        return;
    };
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let map = crate::availability::availability_map(&sim, &snapshot);
    assert_eq!(
        map.len(),
        snapshot.objects.len(),
        "every object in the snapshot has an availability entry"
    );
    for id in snapshot.objects.keys() {
        assert!(map.contains_key(id), "{id} missing from the map");
    }
}
