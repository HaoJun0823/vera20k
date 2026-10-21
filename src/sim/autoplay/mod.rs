//! Scripted auto play: an agent that issues the local player's commands.
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
//! **Scope: actuation only.** This module does not decide what to build or
//! attack. That is the policy layer, deliberately out of scope here — see
//! `docs/autoplay.md`, "Deferred: the brain". It also does not reproduce the
//! input layer's `What_Action` and sidebar admission trees; strict equivalence
//! with a human click is a non-goal, and [`host::AutoPlayHost`]'s gate is only
//! the minimal self-harm check the document specifies.
//!
//! Timing an agent must be written against: the host runs just before the
//! frame drains its due commands, so a command it issues is stamped with the
//! current tick, is drained by that same frame
//! (`take_due_commands`, due at `execute_tick <= tick + 1`), and takes effect
//! in that frame's tail. Nothing in the simulation remembers what the agent
//! asked for; an agent that waits for a build keeps that state itself.
//! `a_host_issued_command_is_drained_by_the_frame_that_follows_its_step` pins
//! this, because it is easy to state one frame out.
//!
//! Dependencies: `sim` only, so this stays inside the core crate and is usable
//! from `headless_scenario` with no window, renderer or UI.

pub mod agent;
pub mod host;
pub mod observation;

pub use agent::{AutoPlayAgent, ScriptStep, ScriptedAgent};
pub use host::AutoPlayHost;
pub use observation::{AutoPlayView, OwnEntity, ReadyBuilding};

#[cfg(test)]
mod autoplay_tests;
