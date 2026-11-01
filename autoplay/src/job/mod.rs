//! Jobs: the only thing a unit can be given, and the one place that decides
//! which unit gets which.
//!
//! A job declares what it needs (a capability predicate, never a type name),
//! what it targets, and when it ends. The assignment is deterministic,
//! integer-keyed, one job per unit, and it must leave no unit unowned.
//!
//! Not implemented yet. See docs/autoplay-v2.md, step 3.

use crate::registry::JobKind;

/// What a manager asks the assignment for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub kind: JobKind,
    /// The object or cell the job works on.
    pub target: Target,
    /// Larger runs first. Computed from the manager's state, never a constant
    /// per kind.
    pub priority: i64,
}

/// What a job points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Object(u64),
    Cell((u16, u16)),
    None,
}
