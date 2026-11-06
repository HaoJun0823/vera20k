//! Whether the automated player actually holds a unit right now.
//!
//! B3's most important tests - the review's six - are all about handover: the native AI
//! running when AutoPlay is off, not overwriting an order AutoPlay placed, being able to
//! take a unit back when AutoPlay releases it, and AutoPlay never covering a player's own
//! command. All six need the same instrument: a way to ask *who is driving this unit*.
//!
//! Ownership is not enough. A house owns its units whether or not anything is ordering
//! them, so [`crate::control::ControlState`] answers "may I" and this module answers "am I
//! actually". The difference shows up the moment two systems can both write the same
//! mission queue: an order that is admitted and then overwritten by a later writer looks
//! exactly like an order that was obeyed, unless someone checks afterwards.
//!
//! What is read is the engine's own mission state - `current`, `queued` and `suspended` -
//! rather than a shadow copy of intent. A shadow copy would be the second-owner defect the
//! plan forbids, and it would disagree with the engine exactly when it mattered.

use vera20k_core::sim::mission::MissionId;
use vera20k_core::sim::world::Simulation;

/// What a unit is doing, as the engine reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionSnapshot {
    /// The mission being executed.
    pub current: MissionId,
    /// A mission waiting to commence. Non-empty means something queued work that has not
    /// started, which is how a competing order becomes visible.
    pub queued: MissionId,
    /// A mission held while a higher-priority one runs.
    pub suspended: MissionId,
}

impl MissionSnapshot {
    /// Read one unit's mission state, or `None` if it no longer exists.
    ///
    /// A missing object is an answer, not an error: a plan that names a destroyed unit is
    /// out of date, and pretending otherwise would let a caller act on a ghost.
    pub fn of(sim: &Simulation, id: u64) -> Option<Self> {
        let entity = sim.entities().get(id)?;
        Some(Self {
            current: entity.mission.current(),
            queued: entity.mission.queued(),
            suspended: entity.mission.suspended(),
        })
    }

    /// Whether nothing is executing, queued or held.
    pub fn is_idle(&self) -> bool {
        self.current == MissionId::NONE
            && self.queued == MissionId::NONE
            && self.suspended == MissionId::NONE
    }

    /// Whether a mission is waiting to start.
    ///
    /// The one reading that distinguishes "my order is running" from "my order is queued
    /// behind somebody else's". `MissionId` has no `is_none`; the native sentinel is
    /// `-1`, which is what `NONE` is.
    pub fn has_queued(&self) -> bool {
        self.queued != MissionId::NONE
    }
}

/// Whether the automated player's order for `id` is still the one in effect.
///
/// The handover question in one function: an order was placed, the world moved on, and a
/// caller wants to know whether what it asked for survived. `expected` is the mission the
/// order should leave the unit in - `MOVE` for a move - and the answer is `false` once
/// anything else is current, which is how an overwrite becomes detectable instead of
/// invisible.
///
/// This deliberately does not claim the unit is *doing* the mission well. It answers only
/// "is the mission I asked for still the one on the unit", which is the narrow question the
/// handover tests need.
pub fn order_still_stands(sim: &Simulation, id: u64, expected: MissionId) -> bool {
    MissionSnapshot::of(sim, id).is_some_and(|snapshot| snapshot.current == expected)
}

/// Whether a unit is running a mission at all, whatever it is.
pub fn is_executing(sim: &Simulation, id: u64) -> bool {
    MissionSnapshot::of(sim, id).is_some_and(|snapshot| snapshot.current != MissionId::NONE)
}

/// Whether anything has queued new work on top of what is running.
///
/// Used to catch the case the review worried about most: the automated player switched off
/// while a team it created keeps writing. A queued mission appearing after the switch is
/// the observable trace of that, and it is visible even when the current mission has not
/// changed yet.
pub fn has_pending_replacement(sim: &Simulation, id: u64) -> bool {
    MissionSnapshot::of(sim, id).is_some_and(|snapshot| snapshot.has_queued())
}
