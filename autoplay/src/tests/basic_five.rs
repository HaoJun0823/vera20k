//! The architecture document's five basic cases, as far as this crate can state them.
//!
//! §18.1 asks for five tests before any advanced arbitration: a unit that cannot be
//! controlled is not taken over, a player's operation is not covered, the end of a match
//! clears state, a change of allegiance updates correctly, and a unit that is not this
//! house's is never driven. Four of the five are questions about whether a unit is *this
//! playership's to drive*, which is what [`crate::eligibility`] answers.
//!
//! The fifth - a unit in a player's formation - is not answerable here and is called out
//! rather than faked: selection lives in the app layer and never reaches the simulation, so
//! an automated player sharing a house with a human would need that fact plumbed in. The
//! plan has one house per controller, and the test below records the assumption instead of
//! asserting a behaviour nothing can produce.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter};
use crate::control::{ControlState, Subsystem};
use crate::eligibility::{self, Ineligible};

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x18A1);
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
    Some((sim, rules, americans, ours, theirs))
}

fn order(id: u64, from: (u16, u16)) -> Command {
    Command::AttackMove {
        entity_id: id,
        target: None,
        destination: Some(TargetKind::Cell(from.0 + 2, from.1)),
    }
}

/// Case 1: a unit that cannot be controlled is not taken over.
#[test]
fn a_unit_whose_house_is_defeated_is_not_taken_over() {
    let Some((mut sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    assert!(
        eligibility::eligibility(&sim, americans, ours).is_ok(),
        "eligible to begin with"
    );

    // Defeat the house that holds it. The unit still exists and still has that owner; what
    // changed is that nobody has business driving it any more.
    if let Some(house) = sim.houses_mut_for_test().get_mut(&americans) {
        house.set_is_defeated_for_test(true);
    }
    match eligibility::eligibility(&sim, americans, ours) {
        Err(Ineligible::HouseDefeated { house }) => assert_eq!(house, americans),
        other => panic!("expected a defeated-house refusal, got {other:?}"),
    }

    // And the arbiter refuses too, because a planner that only checks permission would
    // otherwise keep issuing orders into a lost match.
    let control = ControlState::enabled(americans);
    assert!(matches!(
        Arbiter::admit(
            &sim,
            &control,
            Subsystem::Combat,
            ours,
            order(ours, (40, 40))
        )
        .refusal(),
        Some(_)
    ));
}

/// Case 2: another house's unit is never driven.
#[test]
fn another_houses_unit_is_never_driven() {
    let Some((sim, _rules, americans, _, theirs)) = scene() else {
        return;
    };
    match eligibility::eligibility(&sim, americans, theirs) {
        Err(Ineligible::NotThisHouse { owner, house }) => {
            assert_ne!(owner, house);
            assert_eq!(house, americans);
        }
        other => panic!("expected a foreign-house refusal, got {other:?}"),
    }
}

/// Case 3: the end of a match clears the automated player's state.
///
/// Between matches nothing may survive: the review's principle G. `Trace` belongs to a
/// match, and a fresh one starts empty - which is what makes a post-mortem about *this*
/// game rather than about the process's history.
#[test]
fn a_new_match_starts_with_a_clean_trace() {
    let mut first = crate::trace::Trace::new();
    first.record(crate::trace::Decision {
        frame: 900,
        subsystem: "combat",
        subject: "Americans".to_string(),
        candidates: Vec::new(),
        chosen: None,
        reason: "the old match".to_string(),
    });
    assert_eq!(first.decisions().len(), 1);

    // A new trace is what a new match gets. There is no reset to forget: state lives in the
    // value, so dropping it is the cleanup.
    let second = crate::trace::Trace::new();
    assert!(
        second.decisions().is_empty(),
        "a new match carries nothing from the last one"
    );
    assert!(second.counts().is_empty());
    assert_eq!(second.dropped(), 0);
}

/// Case 4: a change of allegiance updates what may be driven.
///
/// Read every time rather than cached, which is the only way a capture or a mind control
/// can be noticed. The test changes ownership and requires the answer to change with it.
#[test]
fn an_object_that_changes_hands_changes_eligibility() {
    let Some((mut sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    assert!(eligibility::eligibility(&sim, americans, ours).is_ok());

    // Hand it to the other house, as a capture or a mind control would.
    let russians = sim.interner().get("Russians").expect("interned");
    sim.entities_mut_for_test().change_owner(ours, russians);

    match eligibility::eligibility(&sim, americans, ours) {
        Err(Ineligible::NotThisHouse { owner, house }) => {
            assert_eq!(owner, russians, "the refusal names the new holder");
            assert_eq!(house, americans);
        }
        other => panic!("expected the ownership change to be seen, got {other:?}"),
    }
    // A cache would have kept answering yes; reading per call is what makes it right.
}

/// Case 5: an object that no longer exists is not driven, and says so plainly.
#[test]
fn a_destroyed_object_is_reported_as_gone() {
    let Some((mut sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    sim.entities_mut_for_test().remove(ours);
    match eligibility::eligibility(&sim, americans, ours) {
        Err(Ineligible::Gone { id }) => assert_eq!(id, ours),
        other => panic!("expected Gone, got {other:?}"),
    }
}

/// The fifth §18.1 case cannot be answered from the simulation, and this records why
/// rather than pretending otherwise.
///
/// "A unit in a player's formation is not taken over" needs to know that a player has it
/// selected. Selection is app-layer state - `MatchInputState.selection_order` and the group
/// lists beside it - and it never reaches `Simulation`. An automated player that shares a
/// house with a human would therefore need the selection plumbed in, and the plan's
/// arrangement is one controller per house, which makes the case moot until that changes.
///
/// Asserting it with a simulation-only scene would pass for the wrong reason: nothing is
/// selected in such a scene, so "not selected implies not taken over" is vacuously true.
#[test]
fn selection_based_handover_needs_app_state_that_the_simulation_does_not_have() {
    let Some((sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    // Nothing in the simulation records a selection, so the only honest statement is that
    // eligibility cannot see one. A `Snapshot` has the same limitation by design: it reads
    // the world, and a selection is not part of the world.
    let snapshot = crate::world::Snapshot::take(&sim, &_rules, americans);
    assert!(
        snapshot.objects.contains_key(&ours),
        "the object is in the world"
    );
    assert!(
        eligibility::eligibility(&sim, americans, ours).is_ok(),
        "and eligibility says it may be driven, because nothing here knows a player has it"
    );
    // The plan's answer is structural: one house, one controller, so there is no human to
    // share with. When that changes this assertion is the one that has to change.
}
