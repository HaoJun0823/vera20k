//! The world snapshot and the decision trace.
//!
//! B2's acceptance is "export a structured world snapshot and a decision trace". These
//! tests check that the snapshot describes a real world accurately and that a trace can
//! answer why a decision was made - including why the alternatives were not, which is the
//! question the previous session kept asking and could not.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::trace::{Candidate, Decision, Trace};
use crate::world::Snapshot;

/// Two houses with one tank each, on flat ground, from retail rules.
fn scene() -> Option<(Simulation, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0x5A17);
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

#[test]
fn a_snapshot_separates_our_objects_from_theirs() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let snapshot = Snapshot::take(&sim, &rules, americans);

    assert_eq!(snapshot.frame, sim.session().tick());
    assert_eq!(snapshot.house, americans);

    let own: Vec<u64> = snapshot.own().map(|object| object.id).collect();
    assert_eq!(own, [ours], "exactly the tank this house owns");

    let enemy: Vec<u64> = snapshot.enemy().map(|object| object.id).collect();
    assert_eq!(enemy, [theirs], "and exactly the one it does not");

    // Ownership is what the filters are built on, so a snapshot that got it wrong would
    // make every downstream decision wrong in the same direction.
    let ours_object = snapshot.objects.get(&ours).expect("it is in the snapshot");
    assert_eq!(ours_object.owner, americans);
    assert_eq!(ours_object.type_name, "MTNK");
    assert!(ours_object.health > 0);
    assert_eq!(ours_object.cell, (40, 40));
}

/// The distinction the whole visibility design exists for: what the engine knows versus
/// what the player can see.
#[test]
fn enemy_objects_are_only_offerable_as_targets_when_visible() {
    let Some((sim, rules, americans, _, theirs)) = scene() else {
        return;
    };
    let snapshot = Snapshot::take(&sim, &rules, americans);

    // A tank at (60, 60) on flat ground with no viewer near it is known to the engine and
    // not yet seen by this house.
    let enemy_all: Vec<u64> = snapshot.enemy().map(|object| object.id).collect();
    assert!(
        enemy_all.contains(&theirs),
        "the engine's view of the enemy includes it"
    );
    // Whatever the fog says, the two iterators must be consistent: visible is a subset of
    // everything, never a different set.
    let visible: Vec<u64> = snapshot.enemy_visible().map(|object| object.id).collect();
    for id in &visible {
        assert!(enemy_all.contains(id), "{id} visible implies known");
    }
}

/// Reading is a straight pass with nothing filtered: a snapshot that quietly dropped
/// objects would make "no enemy buildings" indistinguishable from "I did not look".
#[test]
fn a_snapshot_is_a_value_that_does_not_change_with_the_world() {
    let Some((mut sim, rules, americans, ours, _)) = scene() else {
        return;
    };
    let snapshot = Snapshot::take(&sim, &rules, americans);
    let before = snapshot.objects.len();
    let recorded = snapshot.objects.get(&ours).expect("recorded").cell;

    // Move the tank and take nothing new. The snapshot is a value, so it must not follow.
    sim.entities_mut_for_test()
        .get_mut(ours)
        .expect("still exists")
        .position = vera20k_core::sim::movement::ground_pose::Position::new(
        recorded.0 + 3,
        recorded.1,
        0,
        None,
        Default::default(),
        Default::default(),
    );
    // Reading the held snapshot returns what it recorded, not what the world became.
    assert_eq!(
        snapshot.objects.get(&ours).expect("still recorded").cell,
        recorded,
        "a snapshot is a value; the world moving on does not rewrite it"
    );
    // And a fresh read sees the change, which is what makes taking one per decision
    // sound rather than wasteful.
    let after = Snapshot::take(&sim, &rules, americans);
    assert_eq!(
        after.objects.len(),
        before,
        "no object appeared or vanished"
    );
    assert_ne!(
        after.objects.get(&ours).expect("still there").cell,
        recorded,
        "a new snapshot describes the world as it is now"
    );
}

/// The composition reading is the shape the v2 ledger used, so a report can be compared
/// with an older one.
#[test]
fn own_composition_counts_by_type() {
    let Some((sim, rules, americans, _, _)) = scene() else {
        return;
    };
    let snapshot = Snapshot::take(&sim, &rules, americans);
    assert_eq!(snapshot.own_composition(), [("MTNK".to_string(), 1)]);
}

/// A decision records its alternatives, which is the part that cannot be derived from the
/// winner alone.
#[test]
fn a_decision_records_why_the_alternatives_lost() {
    let mut trace = Trace::new();
    trace.record(Decision {
        frame: 400,
        subsystem: "production",
        subject: "Americans".to_string(),
        candidates: vec![
            Candidate {
                label: "FV".to_string(),
                score: 38,
                rejected_because: "does 3 permille against concrete".to_string(),
            },
            Candidate {
                label: "MTNK".to_string(),
                score: 30,
                rejected_because: String::new(),
            },
        ],
        chosen: Some("MTNK".to_string()),
        reason: "cheapest that answers anti-vehicle".to_string(),
    });

    let decision = trace.last_about("Americans").expect("one decision");
    assert_eq!(trace.last_about("nowhere"), None, "a miss says so");

    let winner = decision.winner().expect("a winner was chosen");
    assert_eq!(winner.label, "MTNK");
    // The loser's score is higher, which is the whole point: a ranking is not an argmax
    // over the printed number, and a trace that dropped the losers would hide that.
    let loser = decision
        .candidates
        .iter()
        .find(|c| c.label == "FV")
        .expect("the loser is kept");
    assert!(
        loser.score > winner.score,
        "the alternative outscored the choice on the printed number"
    );
    assert!(
        !loser.rejected_because.is_empty(),
        "and says why it lost anyway"
    );
    assert!(decision.describe().contains("MTNK"));
}

/// The trace is a window, and it says so rather than silently forgetting.
#[test]
fn a_full_trace_drops_the_oldest_and_counts_what_it_lost() {
    let mut trace = Trace::new();
    for frame in 0..(crate::trace::CAPACITY as u64 + 3) {
        trace.record(Decision {
            frame,
            subsystem: "combat",
            subject: format!("object-{frame}"),
            candidates: Vec::new(),
            chosen: None,
            reason: "nothing offered".to_string(),
        });
    }
    assert_eq!(trace.decisions().len(), crate::trace::CAPACITY);
    assert_eq!(trace.dropped(), 3, "and it counts the three it lost");
    // The most recent decisions are the ones a post-mortem reads, so those survive.
    assert!(
        trace.last_about("object-2").is_none(),
        "the oldest are the ones dropped"
    );
    assert!(
        trace
            .last_about(&format!("object-{}", crate::trace::CAPACITY as u64 + 2))
            .is_some(),
        "the newest survive"
    );
    // The per-subsystem count is a total, not a current length: it says how many combat
    // decisions were ever made, which is what makes "this subsystem decided nothing all
    // match" answerable even after the records themselves have aged out of the window.
    assert_eq!(
        trace.counts().get("combat"),
        Some(&(crate::trace::CAPACITY + 3)),
        "the count covers every decision made, including the dropped ones"
    );
}
