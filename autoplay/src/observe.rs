//! Watching an order take effect, or fail to.
//!
//! The plan's principle E is that publishing a command is not the same as it
//! succeeding, and B1's chain ends at "confirm the order had the intended effect".
//! The executor reports whether a *frame committed*; this module reports whether the
//! *world changed*, which is a different question and the one a planner has to act on.
//!
//! The distinction is not academic. The first B1 probe saw a frame admit an order, saw
//! the object still alive, and saw it never move - because the order named an object its
//! issuer did not own. Every layer reported success except the world. So an order that
//! is admitted and a unit that is still standing in the same cell has to be a *named
//! outcome*, not an absence of information.
//!
//! What this deliberately does not do: retry, replan, or decide how long is too long.
//! A timeout is a policy choice and belongs to whoever asked for the order; this reports
//! what happened within a window the caller chose.

use vera20k_core::sim::world::Simulation;

/// A cell in map coordinates, as the engine reports positions.
pub type Cell = (u16, u16);

/// What became of an order after the world was given time to act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The object left its starting cell, which is what a move was for.
    Moved {
        /// Where it started.
        from: Cell,
        /// Where it is now.
        to: Cell,
        /// How many frames it took.
        frames: u32,
    },
    /// The object is still in its starting cell after the whole window.
    Stationary {
        /// Where it still is.
        at: Cell,
        /// How many frames were watched.
        frames: u32,
    },
    /// The object no longer exists. A move that killed its subject is a result, not a
    /// failure to observe, and is reported separately from staying put.
    Gone {
        /// The frame it was first found missing.
        frames: u32,
    },
    /// The frame itself failed while watching, so nothing can be concluded.
    ///
    /// Kept distinct from `Stationary`: a frame failure means the world is in an
    /// unknown state, and a planner that treated it as "did not move" would retry into
    /// a broken simulation.
    Unobservable {
        /// How many frames ran before the failure.
        frames: u32,
    },
}

impl Outcome {
    /// Whether the order achieved what a move order is for.
    pub const fn moved(&self) -> bool {
        matches!(self, Self::Moved { .. })
    }

    /// One line for a log, saying what happened rather than only that something did.
    pub fn describe(&self) -> String {
        match self {
            Self::Moved { from, to, frames } => {
                format!(
                    "moved from ({}, {}) to ({}, {}) in {frames} frames",
                    from.0, from.1, to.0, to.1
                )
            }
            Self::Stationary { at, frames } => {
                format!("still at ({}, {}) after {frames} frames", at.0, at.1)
            }
            Self::Gone { frames } => format!("gone after {frames} frames"),
            Self::Unobservable { frames } => {
                format!("a frame failed after {frames} frames, so nothing was observed")
            }
        }
    }
}

/// Where an object is, if it still is anywhere.
pub fn cell_of(sim: &Simulation, id: u64) -> Option<Cell> {
    sim.entities()
        .get(id)
        .map(|entity| (entity.position.rx(), entity.position.ry()))
}

/// Watch `id` for up to `frames` frames, advancing the world, and report what it did.
///
/// The window is the caller's: how long a walk should take depends on the distance, the
/// unit's speed and the terrain, and this module has no business guessing. What it
/// guarantees is that the answer is one of four named outcomes rather than "it did not
/// move and I am not sure why".
pub fn watch_move(
    runtime: &mut vera20k_core::sim::runtime::SimRuntime,
    id: u64,
    from: Cell,
    frames: u32,
) -> Outcome {
    for elapsed in 1..=frames {
        let frame = runtime.advance_frame(
            &[],
            vera20k_core::headless_scenario::SIM_TICK_MS,
            vera20k_core::sim::world::TickLane::Ordinary,
            vera20k_core::sim::world::FrameEffects::default(),
        );
        if frame.is_err() {
            return Outcome::Unobservable { frames: elapsed };
        }
        match cell_of(&runtime.simulation, id) {
            // Out of the starting cell: the order took.
            Some(cell) if cell != from => {
                return Outcome::Moved {
                    from,
                    to: cell,
                    frames: elapsed,
                };
            }
            Some(_) => {}
            None => return Outcome::Gone { frames: elapsed },
        }
    }
    Outcome::Stationary { at: from, frames }
}
