//! The policy interface: the interchangeable brain behind the hands.
//!
//! **Exactly one policy drives a house at a time, and it is expected to be
//! self-contained.** It owns whatever memory it needs — its own plan, its own
//! generator, its own record of what it asked for and what has happened since.
//! Nothing is shared with a second issuer, because there is no second issuer:
//! this is the operation end. A house has one pair of hands at a time.
//!
//! That is what keeps [`crate::sim::autoplay::host::AutoPlayHost`] small and
//! stable. The host decides *when* the policy is asked, admits what comes back
//! and queues it; it arbitrates nothing between competitors and keeps no ledger
//! of outstanding work on the policy's behalf. The policy decides *what*, and
//! remembers its own work across frames.
//!
//! Implementations are interchangeable, and that is the point of the seam. The
//! retail AI is one: it reads its basic choices from the loaded rules — what
//! may be built, which buildings count as economic and which as defensive —
//! and its strategy, tactics and team instruction groups from `AIMD.INI`, so a
//! mod that extends either file changes what the policy decides without
//! touching anything here. A fixed script is another. A later controller is a
//! third. Which one is installed is the caller's choice, and none of them
//! writes simulation state.
//!
//! The seam also runs the other way. Nothing here says the driven house is the
//! local player's: a policy can drive any house, so these hands can stand in
//! for a computer player rather than only for a human one.
//!
//! Determinism: the policy is part of the deterministic input stream. It may
//! not read a clock, a file or a socket while deciding, may not iterate an
//! unordered container, and may not let an `f32` reach a decision. Any
//! randomness must come from a generator it seeds and owns, never from a
//! simulation stream.

use crate::sim::autoplay::observation::AutoPlayView;
use crate::sim::command::Command;

/// A source of commands for one house: complete, self-contained, and the only
/// one attached to its host.
pub trait AutoPlayPolicy {
    /// A short name, for diagnostics and for the HUD line that names what is
    /// driving the house.
    fn name(&self) -> &'static str;

    /// Called once per frame, between frames. Returned commands are queued for
    /// the host's house and take effect in that frame's tail.
    ///
    /// The policy sees only [`AutoPlayView`]. It remembers what it needs across
    /// calls; the host does not remember anything for it.
    fn decide(&mut self, view: &AutoPlayView<'_>) -> Vec<Command>;
}
