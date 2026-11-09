//! One frame of AutoPlay work, as a value the caller can act on.
//!
//! The headless match binary proved the task loop; this is the same decision as a
//! reusable API so the game client does not have to own a second copy of it. It decides
//! *whether* to issue an order and *what* the order is. It does not advance a frame,
//! submit anything, or choose a strategy - the caller owns all three.
//!
//! At most one order per frame: the engine's local command channel is a batch the caller
//! already owns, and keeping one decision per frame leaves the frame advance in one place.

use vera20k_core::sim::command::Command;
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter, Validated};
use crate::control::{ControlState, Subsystem};
use crate::idle::{TaskBoard, TaskEnd, lapsed};
use crate::world::Snapshot;

/// What AutoPlay asks for on one frame.
pub struct FramePlan {
    /// The order, already admitted. The caller injects `envelope` into the local
    /// command channel; only on success should it assign `subject` on the task board.
    pub validated: Validated,
    /// Task endings detected this frame, with their named reasons. They have already
    /// been applied to the board, so this is a report, not a to-do list.
    pub endings: Vec<(u64, TaskEnd)>,
    /// The simulation frame this decision was made on, so a successful assignment
    /// starts its reassessment window at the right tick.
    pub frame: u64,
}

/// Decide one frame's work, applying any lapsed tasks to `tasks`.
///
/// Returns `None` when AutoPlay is off, no unit needs work, or no order could be
/// admitted. The caller still has to advance the frame in every case.
pub fn plan_frame(
    sim: &Simulation,
    snapshot: &Snapshot,
    control: &ControlState,
    tasks: &mut TaskBoard,
) -> Option<FramePlan> {
    if !control.is_global_enabled() {
        return None;
    }

    let frame = sim.session().tick();
    let endings = lapsed(snapshot, control, tasks, frame);
    for (unit, end) in &endings {
        tasks.finish(*unit, *end);
    }

    let validated = tasks
        .unassigned(snapshot, control)
        .into_iter()
        .find_map(|unit| {
            let order = Command::Guard {
                entity_id: unit,
                target: None,
            };
            match Arbiter::admit(sim, control, Subsystem::Combat, unit, order) {
                Admission::Allowed(validated) => Some(validated),
                Admission::Refused(_) => None,
            }
        })?;
    Some(FramePlan {
        validated,
        endings,
        frame,
    })
}
