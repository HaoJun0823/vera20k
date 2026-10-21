//! Scripted auto play: an agent that issues a house's commands.
//!
//! The agent replaces the player's hands, not the game. It observes through
//! [`observation::AutoPlayView`], a read-only projection scoped to one house,
//! and answers with [`crate::sim::command::Command`]s. [`host::AutoPlayHost`]
//! wraps each answer into a [`crate::sim::command::CommandEnvelope`] and hands
//! it to `Simulation::queue_commands`, the same append-only ingress the
//! player's clicks use (`sim/world/command_schedule.rs`). Nothing downstream
//! changes and this module writes no simulation state, so an agent-driven
//! match records and replays like any other: the agent is just another command
//! producer.
//!
//! # Layering
//!
//! Two layers, with the seam between them:
//!
//! - [`policy::AutoPlayPolicy`] is the interchangeable brain. **One** policy
//!   drives a house at a time and is self-contained: it owns its own plan, its
//!   own generator and its own memory of what it asked for. Implementations
//!   are swappable — the retail AI reading the loaded rules and `AIMD.INI`, a
//!   script, a later controller — and the host knows nothing about which one
//!   is installed. See `policy.rs` for the full contract.
//! - [`host::AutoPlayHost`] is the stable hand. It decides when the policy is
//!   asked, admits what comes back, applies the per-frame budget and queues
//!   the result. It arbitrates nothing and keeps no ledger on the policy's
//!   behalf, because there is never a second issuer.
//!
//! Because the host takes an owner rather than assuming the local player, the
//! same hands can stand in for a computer player as well as a human one.
//!
//! **Scope: actuation only.** This module does not decide what to build or
//! attack; that is whichever policy is installed. It also does not reproduce
//! the input layer's `What_Action` and sidebar admission trees; strict
//! equivalence with a human click is a non-goal, and
//! [`host::AutoPlayHost`]'s gate is only the minimal self-harm check
//! `docs/autoplay.md` specifies.
//!
//! Timing: the host runs just before the frame drains its due commands, so a
//! command it issues is stamped with the current tick, is drained by that same
//! frame (`take_due_commands`, due at `execute_tick <= tick + 1`), and takes
//! effect in that frame's tail. Nothing in the simulation remembers what the
//! agent asked for — the policy keeps that — and
//! `a_host_issued_command_is_drained_by_the_frame_that_follows_its_step` pins
//! this, because it is easy to state one frame out.
//!
//! Dependencies: `sim` only, so this stays inside the core crate and is usable
//! from `headless_scenario` with no window, renderer or UI.

pub mod host;
pub mod observation;
pub mod policy;
pub mod scripted_policy;

pub use host::AutoPlayHost;
pub use observation::{AutoPlayView, OwnEntity, ReadyBuilding};
pub use policy::AutoPlayPolicy;
pub use scripted_policy::{ScriptStep, ScriptedPolicy};

#[cfg(test)]
mod autoplay_tests;
