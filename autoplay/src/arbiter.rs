//! The one door an order goes through.
//!
//! The plan's principle A says a strategy module may not publish commands to the
//! engine directly, and the review added that every state-changing operation passes a
//! single control check. This module is that door, and it is deliberately small: it
//! validates, it does not decide. Choosing *what* to do belongs to a planner; deciding
//! whether it *may* be done belongs here.
//!
//! What it checks, and why each one is not paranoia:
//!
//! - the automated player is on, and the subsystem this order belongs to is on;
//! - the object exists (a destroyed object's id is not an error to swallow);
//! - the object belongs to the house this player speaks for.
//!
//! The third is the one that cost the most to find. `vera20k-core` admits a command
//! naming an object the issuing house does not own and then drops it in dispatch, with
//! nothing in the frame output distinguishing that from success - see
//! `src/sim/world/autoplay_command_path_tests.rs`, which pins both halves.

use vera20k_core::sim::command::{Command, CommandEnvelope};
use vera20k_core::sim::world::Simulation;

use crate::control::{ControlState, Refusal, Subsystem};

/// An order that passed every check, with the frame it becomes due on.
///
/// Holding this rather than a raw `CommandEnvelope` is what makes "validated" a type
/// instead of a comment: a caller cannot hand an unvalidated envelope to the executor
/// because the executor only takes this.
#[derive(Debug, Clone)]
pub struct Validated {
    /// What the engine will receive. Built here so the due frame and the owner are
    /// right by construction rather than by the caller remembering.
    pub envelope: CommandEnvelope,
    /// Which object the order is about, kept for diagnostics: a refusal names an id,
    /// so a validation should too.
    pub subject: u64,
}

/// What became of an order.
#[derive(Debug, Clone)]
pub enum Admission {
    /// Every check passed; the caller may submit it.
    Allowed(Validated),
    /// Refused, with a reason that can be logged and acted on.
    Refused(Refusal),
}

impl Admission {
    /// Whether the order may be submitted.
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed(_))
    }

    /// The refusal, if any.
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Allowed(_) => None,
            Self::Refused(reason) => Some(reason),
        }
    }
}

/// Validates orders against the control state and the world.
///
/// Stateless: the world is passed in on every call, so there is no cached view that
/// can drift. That is a deliberate cost - re-reading an entity per order - and it is
/// the price of the review's "checked every time, not once at creation".
pub struct Arbiter;

impl Arbiter {
    /// Check one order about `subject` against `control` and the current world.
    ///
    /// `subsystem` is which switch governs this order; a move is `Combat`, a building
    /// placement is `Production`, and a deploy is `Ability`. The caller names it rather
    /// than the arbiter inferring it from the command, because the same `Command`
    /// variant can serve two subsystems and guessing wrong would silently widen
    /// authority.
    pub fn admit(
        sim: &Simulation,
        control: &ControlState,
        subsystem: Subsystem,
        subject: u64,
        payload: Command,
    ) -> Admission {
        if !control.is_global_enabled() {
            return Admission::Refused(Refusal::GlobalDisabled);
        }
        if !control.is_subsystem_enabled(subsystem) {
            return Admission::Refused(Refusal::SubsystemDisabled(subsystem));
        }
        // Existence and ownership are two failures, not one: a missing object is a
        // planner bug, while a foreign object is a world change the planner has not
        // caught up with. They need different handling and are reported differently.
        let Some(entity) = sim.entities().get(subject) else {
            return Admission::Refused(Refusal::NoSuchObject { id: subject });
        };
        let owner = entity.owner();
        if !control.may_command(owner, subsystem) {
            return Admission::Refused(Refusal::NotOurObject {
                owner,
                house: control.house(),
            });
        }
        // `execute_tick` is the frame the order becomes due, and the scheduler compares
        // it against `tick + 1`. Dating it at the current tick leaves it one frame early
        // and it never becomes due - the second thing the B1 probe had to learn.
        let due = sim.session().tick().saturating_add(1);
        Admission::Allowed(Validated {
            envelope: CommandEnvelope::new(owner, due, payload),
            subject,
        })
    }
}
