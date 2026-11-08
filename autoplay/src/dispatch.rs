//! Stopping the same order from being sent twice, without stopping a retry.
//!
//! A planner runs every frame. It looks at the world, decides a unit should move to a cell,
//! and sends the order - and then does the same next frame, and the next, because from its
//! point of view the unit has the same job it had a moment ago. Left alone that floods the
//! mission queue with identical records and turns every tick into a replan.
//!
//! The obvious fix - drop an order if the previous one was the same - is wrong, and wrong in
//! a way that matters: the plan's B3 also asks for *replanning after failure*, and a unit
//! that refused its order needs exactly that same order sent again. A filter that cannot
//! tell "already doing it" from "already failed at it" would convert a stuck unit into a
//! permanently stuck one.
//!
//! So suppression here has three preconditions, and it needs all three:
//!
//! - the payload is identical to the last one for that object;
//! - that last order is not known to have failed;
//! - the unit is still running the mission that order implies.
//!
//! Anything else goes through. `SameFrame` catches a caller that loops within one frame;
//! `StillRunning` catches the ordinary replay; a recorded failure clears the way for the retry.

use std::collections::BTreeMap;

use vera20k_core::sim::command::Command;
use vera20k_core::sim::mission::state::MissionId;
use vera20k_core::sim::world::Simulation;

use crate::handover;

/// Why an order was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suppression {
    /// The same order for the same object was already sent on this frame.
    SameFrame,
    /// The same order is still the one the unit is carrying out.
    StillRunning,
}

impl Suppression {
    /// One line for a log.
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::SameFrame => "the same order was already sent this frame",
            Self::StillRunning => "the unit is already carrying out this order",
        }
    }
}

/// What was last sent for one object.
#[derive(Debug, Clone)]
struct Issued {
    payload: Command,
    frame: u64,
    /// The mission this order should leave the unit in. Declared by the caller, because
    /// deriving a mission from a `Command` variant would be a guess - several variants map
    /// to the same mission, and a wrong guess here silently blocks retries.
    expected: MissionId,
    /// Whether the order is still believed to be in force. Cleared by
    /// [`Dispatcher::note_failed`], which is what lets a retry through.
    in_force: bool,
}

/// Remembers what each object was last told to do.
///
/// Owned by the match, like the trace: nothing survives into the next one. A dispatcher
/// carried between matches would suppress a legitimate first order because a previous game
/// happened to ask for the same thing.
#[derive(Debug, Default)]
pub struct Dispatcher {
    orders: BTreeMap<u64, Issued>,
    suppressed_same_frame: usize,
    suppressed_still_running: usize,
}

impl Dispatcher {
    /// An empty dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `payload` for `id` repeats what was already sent.
    ///
    /// Reads the world to see whether the unit is still doing the last thing it was told,
    /// so this is not a pure memory check - a unit that finished its move has an empty
    /// mission and will accept the same order again, which is correct: asking a settled
    /// unit to do the same thing again is a new instruction, not a duplicate.
    pub fn repeat_of(
        &self,
        sim: &Simulation,
        id: u64,
        payload: &Command,
        frame: u64,
    ) -> Option<Suppression> {
        let issued = self.orders.get(&id)?;
        if &issued.payload != payload {
            return None;
        }
        if issued.frame == frame {
            return Some(Suppression::SameFrame);
        }
        // A failed order may be retried with the same payload: that is replanning, not a
        // duplicate. Without this line a unit that refuses would stay refused forever.
        if !issued.in_force {
            return None;
        }
        // Still doing what it was told. The mission check is what distinguishes "already
        // obeyed" from "already finished", and the second case must be allowed through.
        handover::order_still_stands(sim, id, issued.expected).then_some(Suppression::StillRunning)
    }

    /// Count a suppression, keeping the two reasons apart so a report can say which.
    pub fn note_suppressed(&mut self, suppression: Suppression) {
        match suppression {
            Suppression::SameFrame => self.suppressed_same_frame += 1,
            Suppression::StillRunning => self.suppressed_still_running += 1,
        }
    }

    /// Record that an order went out.
    pub fn note_issued(&mut self, id: u64, payload: Command, frame: u64, expected: MissionId) {
        self.orders.insert(
            id,
            Issued {
                payload,
                frame,
                expected,
                in_force: true,
            },
        );
    }

    /// Record that the last order for `id` did not take.
    ///
    /// After this the same payload is admitted again, which is exactly what replanning on
    /// failure needs. Called from an observed outcome, so the decision rests on what the
    /// world did rather than on what the planner hoped.
    pub fn note_failed(&mut self, id: u64) {
        if let Some(issued) = self.orders.get_mut(&id) {
            issued.in_force = false;
        }
    }

    /// How many orders were dropped, by reason.
    pub const fn suppressed_same_frame(&self) -> usize {
        self.suppressed_same_frame
    }

    /// How many orders were dropped because the unit was still running them.
    pub const fn suppressed_still_running(&self) -> usize {
        self.suppressed_still_running
    }

    /// Whether anything is remembered about `id`.
    pub fn knows(&self, id: u64) -> bool {
        self.orders.contains_key(&id)
    }

    /// Forget one object, as a destroyed or captured unit should be.
    pub fn forget(&mut self, id: u64) {
        self.orders.remove(&id);
    }
}
