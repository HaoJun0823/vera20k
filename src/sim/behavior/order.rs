//! The single order funnel: readiness, the command, and a refusal that is not
//! silent.
//!
//! Every execution behaviour — the retail brain's direct orders and every
//! managed routine — reaches the game through [`Funnel::prepare`]. It answers
//! one of three things, and the caller must handle all three:
//!
//! - the [`Command`] to issue, when the object exists and its mission engine is
//!   ready to take a new order (`Simulation::mission_ready_to_commence`,
//!   `0x005B362D`'s answer as the per-tick promotion asks it);
//! - [`Refusal::NotReady`], when the object is mid-transition. The order is
//!   *retryable*: nothing was written, and asking again a frame later is
//!   correct. A caller that records an order it never issued stalls the unit —
//!   this is the failure the `NotReady` arm exists to prevent, so it is a
//!   typed answer rather than a silent `None`;
//! - [`Refusal::NoSuchActor`], when the object is gone. Not retryable.
//!
//! The funnel writes no simulation state. The command it hands back travels
//! the same append-only ingress a player's click uses
//! (`Simulation::queue_commands`), so an agent-driven match records and
//! replays like any other; the MEGAMISSION family reaches
//! `Simulation::begin_megamission_retask`, the one native retask funnel, inside
//! that receiver. Nothing here touches the mission authority directly, because
//! a second writer would fork the receiver.

use crate::rules::ruleset::RuleSet;
use crate::sim::behavior::goal::Goal;
use crate::sim::command::Command;
use crate::sim::intern::InternedId;
use crate::sim::world::Simulation;

/// What an order attempt produced, as the issuer sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The command left for the queue and will be drained by this frame.
    Queued,
    /// The object was mid-transition: nothing was written, ask again later.
    NotReady,
    /// The order was dropped: gone actor, another house's object, a switched-off
    /// domain, a managed object, or no budget left.
    Rejected,
}

impl ApplyOutcome {
    pub fn is_queued(self) -> bool {
        self == Self::Queued
    }
}

/// Why [`Funnel::prepare`] produced no command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The object is not in the store: it died, left, or was never ours.
    NoSuchActor,
    /// The object's mission engine is mid-transition.
    NotReady,
}

impl Refusal {
    /// The [`ApplyOutcome`] this refusal maps to.
    pub fn outcome(self) -> ApplyOutcome {
        match self {
            Self::NoSuchActor => ApplyOutcome::Rejected,
            Self::NotReady => ApplyOutcome::NotReady,
        }
    }
}

/// One house's view of the funnel, for one frame.
pub struct Funnel<'a> {
    sim: &'a Simulation,
    rules: &'a RuleSet,
    house: InternedId,
}

impl<'a> Funnel<'a> {
    pub fn new(sim: &'a Simulation, rules: &'a RuleSet, house: InternedId) -> Self {
        Self { sim, rules, house }
    }

    pub fn house(&self) -> InternedId {
        self.house
    }

    /// Whether `actor` is this house's object. Admission's ownership half; the
    /// funnel's readiness answer is the other.
    pub fn owns(&self, actor: u64) -> bool {
        self.sim
            .entities()
            .get(actor)
            .is_some_and(|entity| entity.owner() == self.house)
    }

    /// Translate `goal` into the command to issue for `actor`.
    pub fn prepare(&self, actor: u64, goal: Goal) -> Result<Command, Refusal> {
        if self.sim.entities().get(actor).is_none() {
            return Err(Refusal::NoSuchActor);
        }
        if !self.sim.mission_ready_to_commence(actor, self.rules) {
            return Err(Refusal::NotReady);
        }
        Ok(goal.command(actor))
    }
}
