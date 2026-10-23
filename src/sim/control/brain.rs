//! The decision port: what a brain is, and the box it answers into.
//!
//! The contract is declared here, on L2's side, so the dependency arrow points
//! one way only: a brain is something L2 drives, and L2 never names a concrete
//! brain. [`crate::sim::strategy`] implements it; an external controller can
//! implement it too without touching anything else (see
//! [`super::external`]).
//!
//! A brain is **read-only over the simulation**. It sees
//! [`Observation`](super::observation::Observation), returns
//! [`Order`](super::intent::Order)s and [`Intent`](super::intent::Intent)s, and
//! writes nothing: the control layer applies what it asks for through the
//! owning modules, in one place, with one admission rule. That is what lets a
//! second brain — a script, a replay, a network — drop in without a second
//! implementation of "may I issue this".

use crate::rules::ruleset::RuleSet;
use crate::sim::behavior::goal::Goal;
use crate::sim::command::Command;
use crate::sim::control::intent::{Intent, Order};
use crate::sim::control::observation::Observation;

/// Everything a brain asks for in one frame.
#[derive(Debug, Default)]
pub struct Outbox {
    /// Object orders. Applied through the L1 funnel, in order.
    pub orders: Vec<Order>,
    /// House-scoped requests: production, placement, teams. Applied by the
    /// owning native module, in order.
    pub intents: Vec<Intent>,
    /// Commands named directly, for a source that is not deciding *what to do*
    /// but replaying a command stream: a script, a recorded click sequence, a
    /// test. Admission is unchanged — the domain switch, the budget and the
    /// order gate apply exactly as they do to an order — so this is a shorter
    /// path to the same door, not a second one.
    pub commands: Vec<Command>,
}

impl Outbox {
    /// Ask for one object order.
    pub fn order(&mut self, actor: u64, goal: Goal) {
        self.orders.push(Order { actor, goal });
    }

    /// Ask for one house-scoped request.
    pub fn intent(&mut self, intent: Intent) {
        self.intents.push(intent);
    }

    /// Name one command directly, for a command-level source.
    pub fn command(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub fn is_empty(&self) -> bool {
        self.orders.is_empty() && self.intents.is_empty() && self.commands.is_empty()
    }

    /// Drop every order naming `actor`, keeping the intents. The control layer
    /// uses it so a managed object has one issuer: its routine.
    pub fn drop_orders_for(&mut self, actor: u64) {
        self.orders.retain(|order| order.actor != actor);
    }
}

/// A decision source for one house.
pub trait Brain {
    /// A short name, for diagnostics and for the HUD line that names what is
    /// driving the house.
    fn name(&self) -> &'static str;

    /// Called once per frame, before the frame advances. Everything returned in
    /// `out` is considered for that frame's tail.
    ///
    /// Determinism: no clock, no file, no socket, no `f32` in a decision and no
    /// unordered container. Randomness comes from a generator the brain seeds
    /// and owns, never from a simulation stream.
    fn think(&mut self, rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox);
}
