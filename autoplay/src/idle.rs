//! Which of this house's units have no task from AutoPlay.
//!
//! An engine unit is never truly "idle": construction gives it a default mission, so
//! asking the engine whether it has a mission always answers yes - even when the unit has
//! stood in one place for the whole match. A probe on a freshly spawned tank read
//! `current: MissionId(5)` with nothing queued or suspended. That is exactly the symptom
//! the wishlist calls "units idle at home", and it is why
//! [`crate::handover::MissionSnapshot::is_idle`] cannot answer this question.
//!
//! The authority here is AutoPlay's own record of what it has assigned. A task is
//! AutoPlay's decision, not the engine's, so keeping it here is not a second copy of
//! engine state; the architecture book's Task Management chapter puts that record on this
//! side of the line. Nothing is stored yet about what the task is or how it is going -
//! those belong to the planner this board will serve.
//!
//! What this does not do: choose tasks, or claim a unit is working well. It answers only
//! "have we given this unit anything to do", which is the gap Phase 2 has to close.

use std::collections::BTreeSet;

use crate::control::ControlState;
use crate::world::Snapshot;

/// AutoPlay's record of which units it has given work to. One per match.
#[derive(Debug, Default)]
pub struct TaskBoard {
    assigned: BTreeSet<u64>,
}

impl TaskBoard {
    /// A match with nothing assigned yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Give a unit a task. Idempotent: assigning twice is not two tasks.
    pub fn assign(&mut self, unit: u64) {
        self.assigned.insert(unit);
    }

    /// Withdraw the task, so the unit is reported as needing work again. A released,
    /// captured or destroyed unit should be withdrawn, or the board would keep treating
    /// a unit that is no longer ours as busy.
    pub fn release(&mut self, unit: u64) {
        self.assigned.remove(&unit);
    }

    /// Whether this unit currently holds a task from AutoPlay.
    pub fn has(&self, unit: u64) -> bool {
        self.assigned.contains(&unit)
    }

    /// Units of this house with no task, in id order.
    pub fn unassigned(&self, snapshot: &Snapshot, control: &ControlState) -> Vec<u64> {
        if !control.is_global_enabled() {
            return Vec::new();
        }
        snapshot
            .objects
            .values()
            .filter(|object| object.owner == control.house() && object.in_playfield)
            .filter(|object| !self.assigned.contains(&object.id))
            .map(|object| object.id)
            .collect()
    }
}
