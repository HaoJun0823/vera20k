//! What to do when an order did not take.
//!
//! The plan's B3 asks for replanning after failure, and the pieces already exist: `observe`
//! says what happened, `dispatch` records that an order failed. What is missing is the
//! decision between them - and the decision is not "retry", because a planner that retries
//! every failure is a loop that runs at frame rate forever.
//!
//! Three things have to be told apart, and they are told apart by what the world did:
//!
//! - the order worked. Nothing to plan; clear the record.
//! - the order is in force and the unit simply has not finished yet. Waiting is the right
//!   answer, and a retry here is the duplicate `dispatch` already suppresses.
//! - the order failed. This is the only case that wants a new attempt, and it is capped:
//!   after a few tries the honest conclusion is that the order cannot be carried out, and
//!   saying so is more useful than a fourth identical attempt.
//!
//! A fourth case is deliberately *not* retried at all. `Outcome::Unobservable` means a frame
//! failed and the world is in an unknown state; retrying into that would be sending orders
//! into a simulation whose state nobody knows. The plan's own rule about needing evidence
//! before acting applies here most of all.
//!
//! What this module does not do: choose a *different* order. Falling back to a second
//! objective is planning, and this layer only decides whether another attempt is worth
//! making. Keeping it that way stops replanning from quietly becoming a planner.

use crate::dispatch::Dispatcher;
use crate::observe::Outcome;

/// How many tries before the attempt is declared unable to succeed.
///
/// Small on purpose. A move that has not worked twice is usually a move that cannot work -
/// the destination is unreachable, the unit is immobilised, the path is blocked - and a
/// third attempt at the same order has a worse record than the first two. The number is low
/// enough to notice in a report and high enough to absorb a transient.
pub const MAX_ATTEMPTS: u32 = 3;

/// What to do next with one order that was already watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    /// The order took. Nothing more to do for this object.
    Done,
    /// The order is still in force; give it more frames rather than send it again.
    Wait,
    /// The order failed and another attempt is permitted.
    Retry {
        /// Which attempt this would be, counting from one.
        attempt: u32,
    },
    /// The order failed and has run out of attempts.
    GiveUp {
        /// How many were made.
        attempts: u32,
    },
    /// The world is in an unknown state; nothing may be concluded.
    CannotDecide,
}

impl FollowUp {
    /// Whether another attempt should be made.
    pub const fn should_retry(&self) -> bool {
        matches!(self, Self::Retry { .. })
    }

    /// One line for a log, saying what happens next rather than only what happened.
    pub fn describe(&self) -> String {
        match self {
            Self::Done => "the order took".to_string(),
            Self::Wait => "still in force; waiting rather than resending".to_string(),
            Self::Retry { attempt } => format!("failed; attempt {attempt} of {MAX_ATTEMPTS}"),
            Self::GiveUp { attempts } => {
                format!("failed {attempts} times; the order cannot be carried out")
            }
            Self::CannotDecide => "a frame failed, so nothing is concluded".to_string(),
        }
    }
}

/// What has been tried for one object, and how it went.
///
/// Owned by the match, like the dispatcher and the trace. A fresh one is a fresh match.
#[derive(Debug, Default)]
pub struct Attempts {
    tries: std::collections::BTreeMap<u64, u32>,
}

impl Attempts {
    /// Nothing tried yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many attempts `id` has had.
    pub fn count(&self, id: u64) -> u32 {
        self.tries.get(&id).copied().unwrap_or(0)
    }

    /// Decide what happens next, given what the world did.
    ///
    /// Reads nothing and writes nothing: the caller has already watched the outcome, and
    /// this turns it into a decision. Passing the outcome rather than re-observing keeps the
    /// window - which is a policy the caller chose - out of this layer.
    pub fn decide(&mut self, dispatcher: &mut Dispatcher, id: u64, outcome: Outcome) -> FollowUp {
        match outcome {
            Outcome::Moved { .. } => {
                // It worked. Clear the failure record so a later order of the same shape is
                // not suppressed by history, and reset the count so an object that succeeds
                // today is not penalised tomorrow.
                dispatcher.forget(id);
                self.tries.remove(&id);
                FollowUp::Done
            }
            Outcome::Stationary { .. } => {
                let attempt = self.tries.entry(id).or_insert(0);
                *attempt += 1;
                // The dispatcher has to know this order did not take, or the retry below
                // would be suppressed as a duplicate of itself.
                dispatcher.note_failed(id);
                if *attempt >= MAX_ATTEMPTS {
                    FollowUp::GiveUp { attempts: *attempt }
                } else {
                    FollowUp::Retry { attempt: *attempt }
                }
            }
            Outcome::Gone { .. } => {
                // The object does not exist. Nothing to retry and nothing to remember; a
                // recycled id must not inherit this one's history.
                dispatcher.forget(id);
                self.tries.remove(&id);
                FollowUp::Done
            }
            Outcome::Unobservable { .. } => FollowUp::CannotDecide,
        }
    }

    /// Forget one object, as a destroyed or captured unit should be.
    pub fn forget(&mut self, id: u64) {
        self.tries.remove(&id);
    }
}
