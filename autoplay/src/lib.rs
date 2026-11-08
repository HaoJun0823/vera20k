//! Automated player for Vera20k.
//!
//! ## What exists, and what does not
//!
//! Phase B1 of the plan is a vertical slice, not a layer: read a unit, decide it may
//! be commanded, form an order, admit it, submit it, and confirm the effect. This crate
//! currently holds the middle of that slice - control state and the arbiter - plus the
//! executor that closes it. A world model, a task manager and a planner do not exist
//! yet, and the plan deliberately orders them later.
//!
//! ## Why a separate crate
//!
//! An automated player that reaches into the engine cannot be tested against a
//! different build of it. The crate boundary makes the compiler enforce that everything
//! here goes through `vera20k-core`'s public API, which is also the boundary the plan
//! wants for a controller that must not fork engine behaviour.
//!
//! ## The rule this crate is built around
//!
//! Every operation that can change game state passes [`arbiter::Arbiter::admit`], and
//! the check is per order rather than per object or per match. The plan's review asked
//! for that specifically, because a permission that was true when a team was formed can
//! stop being true when a player takes the units, a mind control flips them, or the
//! automated player is switched off mid-match.

pub mod arbiter;
pub mod claims;
pub mod control;
pub mod dispatch;
pub mod eligibility;
pub mod executor;
pub mod handover;
pub mod observe;
pub mod replan;
pub mod trace;
pub mod world;

#[cfg(test)]
mod tests;

pub use arbiter::{Admission, Arbiter, Validated};
pub use claims::{Claims, Conceded, Source};
pub use control::{ControlState, Refusal, Subsystem};
pub use dispatch::{Dispatcher, Suppression};
pub use eligibility::{Ineligible, eligibility};
pub use executor::{Execution, Executor};
pub use handover::{MissionSnapshot, has_pending_replacement, order_still_stands};
pub use observe::{Cell, Outcome};
pub use replan::{Attempts, FollowUp, MAX_ATTEMPTS};
pub use trace::{Candidate, Decision, Trace};
pub use world::{Object, Snapshot};
