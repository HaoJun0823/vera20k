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
//!
//! A task does not last forever. [`lapsed`] finds the ones whose unit can no longer carry
//! them - destroyed, captured, out of the playfield, or AutoPlay switched off - and
//! [`TaskBoard::finish`] records why. An ending is named rather than a silent removal:
//! a destroyed unit wants no replacement, while a switched-off one wants its task back,
//! and a report that cannot tell them apart will re-plan into a ghost.

use std::collections::{BTreeMap, BTreeSet};

use crate::control::ControlState;
use crate::world::Snapshot;

/// Why a task stopped.
///
/// Named so a report can say what happened rather than "task gone". A destroyed unit
/// wants no replacement; one whose owner changed is no longer ours; one whose player was
/// switched off wants its task back when the switch comes on. One reason cannot stand for
/// all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskEnd {
    /// The object left the world.
    ObjectGone,
    /// It changed hands, so it is no longer this house's to drive.
    OwnerChanged,
    /// It left the playfield.
    OutOfPlayfield,
    /// AutoPlay was switched off.
    Disabled,
}

/// AutoPlay's record of which units it has given work to. One per match.
#[derive(Debug, Default)]
pub struct TaskBoard {
    assigned: BTreeSet<u64>,
    /// How many tasks ended for each reason, across the whole match.
    endings: BTreeMap<TaskEnd, usize>,
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

    /// End a task with a named reason. Only an active task counts, so a stale
    /// notification cannot inflate the report.
    pub fn finish(&mut self, unit: u64, end: TaskEnd) {
        if self.assigned.remove(&unit) {
            *self.endings.entry(end).or_insert(0) += 1;
        }
    }

    /// How many tasks ended for each reason.
    pub fn endings(&self) -> &BTreeMap<TaskEnd, usize> {
        &self.endings
    }

    /// How many units currently hold a task.
    pub fn assignment_count(&self) -> usize {
        self.assigned.len()
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

/// Active tasks whose unit can no longer carry them, with a named reason.
///
/// This reads the same snapshot the caller already holds; it does not reach into the
/// engine a second time, so the report and the decision describe one frame. The board is
/// not changed here - the caller decides what to do with each ending, which is what
/// keeps a read from quietly becoming a policy.
pub fn lapsed(
    snapshot: &Snapshot,
    control: &ControlState,
    board: &TaskBoard,
) -> Vec<(u64, TaskEnd)> {
    if !control.is_global_enabled() {
        return board
            .assigned
            .iter()
            .map(|&unit| (unit, TaskEnd::Disabled))
            .collect();
    }
    board
        .assigned
        .iter()
        .filter_map(|&unit| match snapshot.objects.get(&unit) {
            None => Some((unit, TaskEnd::ObjectGone)),
            Some(object) if object.owner != control.house() => Some((unit, TaskEnd::OwnerChanged)),
            Some(object) if !object.in_playfield => Some((unit, TaskEnd::OutOfPlayfield)),
            Some(_) => None,
        })
        .collect()
}
