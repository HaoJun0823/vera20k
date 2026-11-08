//! Replanning after failure, and the three cases that must not be treated as failure.
//!
//! The value of this module is mostly in what it refuses: a `Moved` outcome clears rather
//! than retries, a `Gone` outcome stops rather than retries, an `Unobservable` outcome
//! decides nothing at all, and a `Stationary` outcome is only retried a bounded number of
//! times. A planner that retried every bad-looking outcome would loop at frame rate; a
//! planner that retried into a failed frame would send orders into a simulation nobody
//! understands.

use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::mission::state::MissionId;

use crate::dispatch::Dispatcher;
use crate::observe::Outcome;
use crate::replan::{Attempts, FollowUp, MAX_ATTEMPTS};

fn moved() -> Outcome {
    Outcome::Moved {
        from: (40, 40),
        to: (42, 40),
        frames: 12,
    }
}

fn stationary(frames: u32) -> Outcome {
    Outcome::Stationary {
        at: (40, 40),
        frames,
    }
}

fn payload(id: u64) -> Command {
    Command::AttackMove {
        entity_id: id,
        target: None,
        destination: Some(TargetKind::Cell(42, 40)),
    }
}

/// The ordinary failure: one retry, then another, then stop.
#[test]
fn a_failed_order_is_retried_a_bounded_number_of_times() {
    let mut attempts = Attempts::new();
    let mut dispatcher = Dispatcher::new();

    assert_eq!(
        attempts.decide(&mut dispatcher, 1, stationary(40)),
        FollowUp::Retry { attempt: 1 }
    );
    assert_eq!(
        attempts.decide(&mut dispatcher, 1, stationary(40)),
        FollowUp::Retry { attempt: 2 }
    );
    assert_eq!(
        attempts.decide(&mut dispatcher, 1, stationary(40)),
        FollowUp::GiveUp {
            attempts: MAX_ATTEMPTS
        },
        "the third failure ends it: a fourth identical attempt has a worse record"
    );
    assert_eq!(attempts.count(1), MAX_ATTEMPTS);
}

/// A retry must survive the duplicate filter, which is the whole reason `decide` touches
/// the dispatcher.
#[test]
fn a_failure_clears_the_order_so_the_retry_is_not_suppressed() {
    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(1, payload(1), 7, MissionId::NONE);
    let mut attempts = Attempts::new();

    // The world says it did not take.
    attempts.decide(&mut dispatcher, 1, stationary(40));
    // A failed order is no longer in force, so the duplicate filter must let the same
    // payload through. That interaction is the reason `decide` touches the dispatcher at
    // all, and asserting it here is what keeps the two modules from drifting apart.
    assert_eq!(
        dispatcher.is_in_force(1),
        Some(false),
        "the order is recorded but no longer in force"
    );
    assert_eq!(attempts.count(1), 1);
}

/// Success clears the history so a later order of the same shape is not suppressed.
#[test]
fn success_clears_the_record_and_the_count() {
    let mut attempts = Attempts::new();
    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(1, payload(1), 3, MissionId::NONE);
    attempts.decide(&mut dispatcher, 1, stationary(40));
    assert_eq!(attempts.count(1), 1);

    assert_eq!(attempts.decide(&mut dispatcher, 1, moved()), FollowUp::Done);
    assert_eq!(attempts.count(1), 0, "a success resets the count");
    assert!(!dispatcher.knows(1), "and forgets the order");
}

/// A destroyed object stops, and leaves nothing behind for a recycled id to inherit.
#[test]
fn an_object_that_vanishes_stops_without_retrying() {
    let mut attempts = Attempts::new();
    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(1, payload(1), 3, MissionId::NONE);
    attempts.decide(&mut dispatcher, 1, stationary(40));
    assert_eq!(attempts.count(1), 1);

    assert_eq!(
        attempts.decide(&mut dispatcher, 1, Outcome::Gone { frames: 5 }),
        FollowUp::Done,
        "a gone object is not a failure to retry"
    );
    assert_eq!(attempts.count(1), 0);
    assert!(!dispatcher.knows(1));
}

/// The one that must decide nothing: a failed frame means the world is unknown.
#[test]
fn an_unobservable_outcome_decides_nothing() {
    let mut attempts = Attempts::new();
    let mut dispatcher = Dispatcher::new();
    dispatcher.note_issued(1, payload(1), 3, MissionId::NONE);

    let follow = attempts.decide(&mut dispatcher, 1, Outcome::Unobservable { frames: 2 });
    assert_eq!(follow, FollowUp::CannotDecide);
    assert!(
        !follow.should_retry(),
        "retrying into a failed frame is not a decision"
    );
    // And nothing was mutated: the attempt count is unchanged and the order is still in
    // force, because nothing was learned about it.
    assert_eq!(attempts.count(1), 0);
    assert!(
        dispatcher.knows(1),
        "an unknown outcome does not clear the record"
    );
}

/// Only failures want another attempt; everything else says something else.
#[test]
fn only_a_stationary_outcome_asks_for_another_attempt() {
    let cases = [
        (moved(), FollowUp::Done),
        (Outcome::Gone { frames: 3 }, FollowUp::Done),
        (Outcome::Unobservable { frames: 3 }, FollowUp::CannotDecide),
        (stationary(20), FollowUp::Retry { attempt: 1 }),
    ];
    for (outcome, expected) in cases {
        let mut attempts = Attempts::new();
        let mut dispatcher = Dispatcher::new();
        let got = attempts.decide(&mut dispatcher, 2, outcome);
        assert_eq!(got, expected, "for {outcome:?}");
        assert_eq!(
            got.should_retry(),
            expected == FollowUp::Retry { attempt: 1 },
            "should_retry agrees with the variant for {outcome:?}"
        );
    }
}
