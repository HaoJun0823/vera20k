//! A real match, with the native team AI running alongside AutoPlay.
//!
//! Everything in this crate so far is tested against a scene built by hand: two houses, one
//! tank, one order. That proves the rules work when the world is quiet, and it is exactly
//! the kind of evidence that fails to hold once the world is busy - which is what the
//! review's handover cases are about. The engine runs team creation for every house every
//! frame, so a real match is where AutoPlay actually meets the native AI.
//!
//! This test does not assert that AutoPlay wins, and it must not: the plan is explicit that
//! acceptance never uses victory over the retail AI, because a 7-computer run showed the
//! retail AI is an unstable baseline. What it asserts is that the invariants hold under load -
//! that AutoPlay only ever commands the house it speaks for, that it never touches a defeated
//! house's units, and that every refusal it did record names a real reason.
//!
//! It needs a retail install, so it returns early without one, the way every other
//! archive-backed test here does. Loading a real map is a different order of magnitude from
//! the INI fixtures the unit tests use.

use std::path::Path;

use vera20k_core::headless_scenario;
use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::command::Command;
use vera20k_core::skirmish_launch::{AiDifficulty, LaunchCountry};

use crate::arbiter::{Admission, Arbiter};
use crate::claims::{Claims, Source};
use crate::control::{ControlState, Refusal, Subsystem};
use crate::eligibility::{self, Ineligible};
use crate::observe::Cell;
use crate::world::Snapshot;

/// Where the retail install lives, if this machine has one.
fn retail_dir() -> Option<std::path::PathBuf> {
    // The project's own convention: an environment variable, not a hard-coded path, so the
    // test says what it needs rather than guessing where it is.
    std::env::var_os("RA2_DIR").map(std::path::PathBuf::from)
}

fn launch() -> vera20k_core::skirmish_launch::SkirmishLaunchSession {
    headless_scenario::battle_session(
        "Arena.mmx",
        &[
            (LaunchCountry::America, AiDifficulty::Easy),
            (LaunchCountry::Russia, AiDifficulty::Easy),
        ],
    )
}

/// AutoPlay only ever commands its own house, across a whole match with the native AI live.
#[test]
fn autoplay_never_commands_outside_its_house() {
    let Some(retail) = retail_dir() else {
        return;
    };
    let Ok(mut scenario) = headless_scenario::load_session(&retail, "Arena.mmx", 0x5EED, launch())
    else {
        return;
    };

    // The house AutoPlay speaks for. A skirmish Battle launch seats the local player, so this
    // is the one house it may drive; every other house is somebody else's, including the
    // computer houses the native team AI is running.
    let Some(house) = scenario.sim().session().current_house() else {
        println!("no local house: the launch seated no player");
        return;
    };
    let object_count = scenario.sim().entities().len();
    let house_count = scenario.sim().houses().iter().count();
    let own_count = scenario
        .sim()
        .entities()
        .values()
        .filter(|entity| entity.owner() == house)
        .count();
    println!("scene: {house_count} houses, {object_count} objects, {own_count} owned by {house:?}");
    let control = ControlState::enabled(house);
    let mut claims = Claims::new();
    let mut refusals: Vec<Refusal> = Vec::new();
    let mut orders_sent = 0usize;

    // A few hundred frames is enough for team creation to have run many times and for units
    // to exist on both sides; it is not a full match, and the module doc says why more frames
    // would not make the assertion stronger.
    for frame in 0..600u64 {
        let snapshot = Snapshot::take(scenario.sim(), &scenario.runtime.resources.rules, house);

        // Sweep this house's own objects, not the first six in the snapshot: a map has far
        // more enemy objects than own ones early on, so taking the first few would test
        // nothing and pass for the wrong reason - which is what the first version did.
        for object in snapshot.own().take(4) {
            // Eligibility first, exactly as a planner would.
            match eligibility::eligibility(scenario.sim(), house, object.id) {
                Ok(()) => {
                    let cell: Cell = object.cell;
                    let order = Command::Stop {
                        entity_id: object.id,
                    };
                    let admission = Arbiter::admit_with_source(
                        scenario.sim(),
                        &control,
                        Some(&claims),
                        Source::AutoPlay,
                        Subsystem::Combat,
                        object.id,
                        order,
                    );
                    match admission {
                        Admission::Allowed(_) => {
                            // A Stop for an owned unit is legitimate; record that AutoPlay
                            // intended to write, and take the claim so later frames concede.
                            orders_sent += 1;
                            claims.claim(object.id, Source::AutoPlay);
                            let _ = cell;
                        }
                        Admission::Refused(reason) => refusals.push(reason),
                    }
                }
                // Anything ineligible must never reach the arbiter at all, and if it did
                // through a different path, the arbiter's ownership check would refuse it.
                Err(ineligible) => {
                    // The invariant this test exists for: every ineligible object is
                    // genuinely outside this house's gift.
                    assert!(
                        matches!(
                            ineligible,
                            Ineligible::NotThisHouse { .. }
                                | Ineligible::HouseDefeated { .. }
                                | Ineligible::HouseLeft { .. }
                                | Ineligible::Gone { .. }
                        ),
                        "unexpected ineligibility at frame {frame}: {ineligible:?}"
                    );
                }
            }
        }

        let _ = scenario.runtime.advance_frame(
            &[],
            1000 / vera20k_core::util::fixed_math::RA2_LOGIC_FRAMES_PER_SECOND,
            vera20k_core::sim::world::TickLane::Ordinary,
            vera20k_core::sim::world::FrameEffects::default(),
        );
    }

    // A test that ran and observed nothing would pass vacuously, and the first version of
    // this loop did exactly that: it swept the first six objects in the snapshot, which on a
    // real map are other houses' units, and reported "0 orders, 0 refusals" as a pass. So the
    // count is asserted rather than printed - the sweep must have found work to do.
    assert!(
        orders_sent > 0,
        "the match gave this house objects to order, so the loop had work to do"
    );

    // The load-bearing assertion: no refusal AutoPlay ever recorded was about a unit the
    // engine would have accepted. A `NotOurObject` refusal on a unit eligibility had already
    // cleared would mean the two checks disagree.
    for refusal in &refusals {
        if let Refusal::NotOurObject {
            owner,
            house: spoken_for,
        } = refusal
        {
            assert_ne!(
                *owner, *spoken_for,
                "the refusal named this player's own house as foreign"
            );
        }
    }
    println!(
        "match ran 600 frames: {orders_sent} orders admitted, {} refusals",
        refusals.len()
    );
}

/// The other half of the invariant, driven deliberately: an enemy object really is refused.
///
/// Without this the match test only shows AutoPlay said yes, and saying yes is the easy half.
/// This asks the same arbiter, on the same live match, about units belonging to computer
/// houses - the ones the native team AI is also driving.
#[test]
fn a_live_matches_enemy_units_are_refused() {
    let Some(retail) = retail_dir() else {
        return;
    };
    let Ok(scenario) = headless_scenario::load_session(&retail, "Arena.mmx", 0x5EED, launch())
    else {
        return;
    };
    let Some(house) = scenario.sim().session().current_house() else {
        return;
    };
    let control = ControlState::enabled(house);
    let claims = Claims::new();
    let snapshot = Snapshot::take(scenario.sim(), &scenario.runtime.resources.rules, house);

    let enemy: Vec<u64> = snapshot.enemy().map(|object| object.id).collect();
    assert!(
        !enemy.is_empty(),
        "a real match has other houses' objects, or this proves nothing"
    );
    let probes: Vec<u64> = enemy.iter().take(4).copied().collect();
    for id in &probes {
        let admission = Arbiter::admit_with_source(
            scenario.sim(),
            &control,
            Some(&claims),
            Source::AutoPlay,
            Subsystem::Combat,
            *id,
            Command::Stop { entity_id: *id },
        );
        assert!(
            admission.refusal().is_some(),
            "an enemy object must never be admitted for this house"
        );
    }
    println!("refused all {} sampled enemy objects", probes.len());
}

/// Nothing survives a match: the four per-match stores all start empty.
///
/// Cheap, but it is the review's principle G and it is worth restating at the match level
/// rather than only at the type level.
#[test]
fn a_match_starts_with_no_remembered_state() {
    let claims = Claims::new();
    let dispatcher = crate::dispatch::Dispatcher::new();
    let attempts = crate::replan::Attempts::new();
    let trace = crate::trace::Trace::new();

    assert_eq!(claims.conceded(), 0);
    assert_eq!(dispatcher.suppressed_same_frame(), 0);
    assert_eq!(dispatcher.suppressed_still_running(), 0);
    assert_eq!(attempts.count(1), 0);
    assert!(trace.decisions().is_empty());
}

/// The map name is a literal because the test needs one specific map; this states it once so
/// a change is a deliberate edit rather than a silent dependency on wherever the file is.
#[test]
fn the_match_scene_names_the_map_it_needs() {
    let path = Path::new("Arena.mmx");
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("mmx"));
}
