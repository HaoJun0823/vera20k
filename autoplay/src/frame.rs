//! One frame of AutoPlay work, as a value the caller can act on.
//!
//! The headless match binary proved the task loop; this is the same decision as a
//! reusable API so the game client does not have to own a second copy of it. It decides
//! *whether* to issue an order and *what* the order is. It does not advance a frame,
//! submit anything, or choose a strategy - the caller owns all three.
//!
//! At most one order per frame: the engine's local command channel is a batch the caller
//! already owns, and keeping one decision per frame leaves the frame advance in one place.
//!
//! Phase 4's first slice: a unit with no task attacks the nearest enemy it can actually
//! see, or holds its post when there is none. [`Snapshot::enemy_visible`] is the only
//! target source - the engine knows every object, but aiming at one the player has not
//! revealed would be cheating rather than playing. Threat assessment, retreat, squad
//! composition and target priority beyond "nearest" are deliberately not here yet.

use vera20k_core::sim::command::Command;
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter, Validated};
use crate::control::{ControlState, Subsystem};
use crate::idle::{TaskBoard, TaskEnd, lapsed};
use crate::world::Snapshot;

/// What the frame's order is for, so a report can say what AutoPlay did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    /// Attack a visible enemy.
    Attack,
    /// Hold position: nothing visible to fight.
    Guard,
}

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
    /// Whether the order was an attack or a hold, for diagnostics.
    pub kind: OrderKind,
}

/// The nearest enemy this unit can actually see, by Manhattan cell distance.
///
/// Integers only - a distance used for a decision must be identical on every platform.
fn nearest_visible_enemy(snapshot: &Snapshot, unit: u64) -> Option<u64> {
    let own = snapshot.objects.get(&unit)?;
    snapshot
        .enemy_visible()
        .filter(|enemy| enemy.in_playfield)
        .min_by_key(|enemy| {
            (own.cell.0 as i32 - enemy.cell.0 as i32).abs()
                + (own.cell.1 as i32 - enemy.cell.1 as i32).abs()
        })
        .map(|enemy| enemy.id)
}

/// One unit's order for this frame, and what kind of order it was.
fn combat_order(snapshot: &Snapshot, unit: u64) -> (Command, OrderKind) {
    match nearest_visible_enemy(snapshot, unit) {
        Some(target_id) => (
            Command::Attack {
                attacker_id: unit,
                target_id,
            },
            OrderKind::Attack,
        ),
        None => (
            Command::Guard {
                entity_id: unit,
                target: None,
            },
            OrderKind::Guard,
        ),
    }
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

    let (validated, kind) = tasks
        .unassigned(snapshot, control)
        .into_iter()
        .find_map(|unit| {
            let (order, kind) = combat_order(snapshot, unit);
            match Arbiter::admit(sim, control, Subsystem::Combat, unit, order) {
                Admission::Allowed(validated) => Some((validated, kind)),
                Admission::Refused(_) => None,
            }
        })?;
    Some(FramePlan {
        validated,
        endings,
        frame,
        kind,
    })
}
