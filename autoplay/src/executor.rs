//! Submits validated orders and reports what actually happened.
//!
//! The plan's principle E: publishing a command is not the same as it succeeding, and
//! the executor is the module that has to tell the difference. The first B1 probe is
//! the cautionary case - a frame reported an order admitted, the object stayed alive,
//! and the unit never moved. Without a separate "did it happen" step, that reads as
//! success.
//!
//! What this executor does not do yet: retry, replan, or refuse an order that is
//! technically valid but hopeless. Those belong to a task layer that does not exist,
//! and the plan orders it after this phase.

use vera20k_core::sim::world::{FrameEffects, TickLane};

use crate::arbiter::Validated;

/// What became of a submitted order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Execution {
    /// The frame committed and took the order into dispatch.
    Committed,
    /// The frame ran but did not commit, which means the world refused the order at a
    /// later stage than admission. Not an error in this crate; a fact to report.
    NotCommitted,
    /// The frame itself failed.
    FrameFailed,
}

impl Execution {
    /// Whether the order reached dispatch.
    pub const fn is_committed(&self) -> bool {
        matches!(self, Self::Committed)
    }
}

/// Submits orders to the engine.
///
/// Holds nothing, so it cannot disagree with the world between calls.
pub struct Executor;

impl Executor {
    /// Advance one frame with `validated` in flight and report whether it committed.
    ///
    /// The order travels in the `commands` slice of `advance_frame`, which is the path
    /// that works from outside the engine. `issue_local_command` is the other door and it
    /// feeds a FIFO that `HeadlessScenario::tick` never drains - pinned by
    /// `src/sim/world/autoplay_command_path_tests.rs` so the next caller does not
    /// rediscover it.
    ///
    /// The resources come from the runtime the caller already holds rather than from a
    /// cached second copy that could drift from the rules the simulation was built with.
    pub fn submit(
        runtime: &mut vera20k_core::sim::runtime::SimRuntime,
        validated: &Validated,
    ) -> Execution {
        let frame = runtime.advance_frame(
            std::slice::from_ref(&validated.envelope),
            1000 / vera20k_core::util::fixed_math::RA2_LOGIC_FRAMES_PER_SECOND,
            TickLane::Ordinary,
            FrameEffects::default(),
        );
        match frame {
            Ok(output) => {
                if output.tick.frame_committed {
                    Execution::Committed
                } else {
                    Execution::NotCommitted
                }
            }
            Err(_) => Execution::FrameFailed,
        }
    }

    /// Advance one frame with nothing in flight.
    ///
    /// An order takes more than one frame to have an effect - a tank has to be given a
    /// mission, then path, then walk - so a caller that submits and reads the position
    /// immediately will always see it unchanged. This exists so the "wait and watch"
    /// loop is not each caller's private invention.
    pub fn submit_idle(runtime: &mut vera20k_core::sim::runtime::SimRuntime) -> Execution {
        let frame = runtime.advance_frame(
            &[],
            1000 / vera20k_core::util::fixed_math::RA2_LOGIC_FRAMES_PER_SECOND,
            TickLane::Ordinary,
            FrameEffects::default(),
        );
        match frame {
            Ok(output) => {
                if output.tick.frame_committed {
                    Execution::Committed
                } else {
                    Execution::NotCommitted
                }
            }
            Err(_) => Execution::FrameFailed,
        }
    }
}
