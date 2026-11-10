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
//! Phase 4: a unit with no task either attacks the nearest visible enemy it can hurt,
//! breaks off when the odds are against it, or holds its post. [`crate::threat::assess`]
//! owns the odds; only revealed objects are considered, because aiming at one the player
//! has not seen would be cheating rather than playing. Squad composition, focus fire and
//! target priority beyond "nearest" are deliberately not here yet.

use vera20k_core::sim::command::Command;
use vera20k_core::sim::mission::{MissionId, MissionType};
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter, Validated};
use crate::control::{ControlState, Subsystem};
use crate::dispatch::Dispatcher;
use crate::idle::{TaskBoard, TaskEnd, lapsed};
use crate::threat::{self, Verdict};
use crate::world::Snapshot;

/// What the frame's order is for, so a report can say what AutoPlay did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    /// Attack a visible enemy (including fighting on when there is nowhere to run).
    Attack,
    /// Break off to a safer cell: the odds are against this unit.
    Retreat,
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
    /// The command that was admitted. The caller records it as issued only after the
    /// submit succeeds, so a refused frame does not poison the duplicate filter.
    pub payload: Command,
    /// The mission this order should leave the unit in, declared here rather than
    /// derived from the `Command` (several variants map to the same mission).
    pub expected: MissionId,
}

/// One unit's order for this frame, and what kind of order it was.
fn combat_order(snapshot: &Snapshot, unit: u64, bounds: (u16, u16)) -> (Command, OrderKind) {
    match threat::assess(snapshot, unit, bounds) {
        Verdict::Engage(target_id) => (
            Command::Attack {
                attacker_id: unit,
                target_id,
            },
            OrderKind::Attack,
        ),
        Verdict::Retreat((target_rx, target_ry)) => (
            Command::Move {
                entity_id: unit,
                target_rx,
                target_ry,
            },
            OrderKind::Retreat,
        ),
        Verdict::Hold => (
            Command::Guard {
                entity_id: unit,
                target: None,
            },
            OrderKind::Guard,
        ),
    }
}

/// The mission an order should leave the unit in.
///
/// Declared here, not derived from the `Command`: several variants map to the same
/// mission, and a wrong guess would silently block a retry. `world_commands.rs:1992`
/// turns `Attack` into `MissionType::Attack`, `:2036` turns `Guard` into
/// `MissionType::AreaGuard` and `:1815` turns `Move` into `MissionType::Move`.
fn expected_mission(kind: OrderKind) -> MissionId {
    match kind {
        OrderKind::Attack => MissionId::from_known(MissionType::Attack),
        OrderKind::Retreat => MissionId::from_known(MissionType::Move),
        OrderKind::Guard => MissionId::from_known(MissionType::AreaGuard),
    }
}

/// Decide one frame's work, applying any lapsed tasks to `tasks`.
///
/// `dispatcher` is AutoPlay's record of what it last told each unit. A unit whose task
/// came up for reassessment but is still carrying out the very same order gets its task
/// window renewed rather than a second copy of the command - re-sending an unchanged
/// instruction every 150 frames is the "flood the queue so the unit cannot fire" symptom.
/// A changed order, a finished mission or a recorded failure all go through as new work.
///
/// Returns `None` when AutoPlay is off, no unit needs a *new* order, or no order could be
/// admitted. The caller still has to advance the frame in every case, and must call
/// [`Dispatcher::note_issued`] with the returned `payload`/`expected` only after the
/// submit succeeded.
pub fn plan_frame(
    sim: &Simulation,
    snapshot: &Snapshot,
    control: &ControlState,
    tasks: &mut TaskBoard,
    dispatcher: &mut Dispatcher,
) -> Option<FramePlan> {
    if !control.is_global_enabled() {
        return None;
    }

    let frame = sim.session().tick();
    let bounds = (sim.session().map_width(), sim.session().map_height());
    let endings = lapsed(snapshot, control, tasks, frame);
    for (unit, end) in &endings {
        tasks.finish(*unit, *end);
    }

    let (validated, kind, payload) =
        tasks
            .unassigned(snapshot, control)
            .into_iter()
            .find_map(|unit| {
                let (order, kind) = combat_order(snapshot, unit, bounds);
                if let Some(suppression) = dispatcher.repeat_of(sim, unit, &order, frame) {
                    dispatcher.note_suppressed(suppression);
                    // Still doing exactly this. Renewing the window keeps the unit owned;
                    // dropping the task instead would make it look idle on every later frame.
                    tasks.assign(unit, frame);
                    return None;
                }
                match Arbiter::admit(sim, control, Subsystem::Combat, unit, order.clone()) {
                    Admission::Allowed(validated) => Some((validated, kind, order)),
                    Admission::Refused(_) => None,
                }
            })?;
    Some(FramePlan {
        validated,
        endings,
        frame,
        kind,
        payload,
        expected: expected_mission(kind),
    })
}
