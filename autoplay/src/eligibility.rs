//! Whether a unit is one this player should be driving at all.
//!
//! The arbiter asks "may I". This answers a different and prior question: *is this mine to
//! drive*. The difference is the one the architecture document's §18.1 five cases turn on -
//! a unit that is defeated, a unit its house has lost, a unit that belongs to a player rather
//! than to this house, a unit whose house has left the match - and none of those are matters
//! of permission. They are matters of the unit not being an automated player's business.
//!
//! Separating the two keeps refusals honest. "I am not allowed" and "this is not mine" want
//! different responses from a planner, and collapsing them into one boolean would make a
//! defeated house's units look like a permission problem to be retried.
//!
//! What cannot be answered here: whether a *player* has that unit selected. Selection lives
//! in the app layer and never reaches the simulation, so an automated player sharing a match
//! with a human in the same house would need that fact plumbed in rather than inferred. The
//! plan has no such arrangement - one house, one controller - and this module says so in the
//! one place a reader would look for it.

use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

/// Why a unit is not the automated player's to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
    /// The object no longer exists.
    Gone {
        /// The id that was asked about.
        id: u64,
    },
    /// The house that holds it has been defeated. Its units are no longer anybody's to
    /// command, and a planner that kept ordering them would be fighting a lost war.
    HouseDefeated {
        /// The defeated house, which is the object's holder.
        house: InternedId,
    },
    /// The house that holds it has left the match.
    HouseLeft {
        /// The house that left.
        house: InternedId,
    },
    /// It belongs to another house, which is a player's or an opponent's.
    NotThisHouse {
        /// Who holds it.
        owner: InternedId,
        /// Who this player speaks for.
        house: InternedId,
    },
}

impl Ineligible {
    /// One line, for a log or a test failure.
    pub fn describe(&self) -> String {
        match self {
            Self::Gone { id } => format!("object {id} no longer exists"),
            Self::HouseDefeated { house } => format!("{house:?} has been defeated"),
            Self::HouseLeft { house } => format!("{house:?} has left the match"),
            Self::NotThisHouse { owner, house } => {
                format!("object belongs to {owner:?}, this player speaks for {house:?}")
            }
        }
    }
}

/// Whether `id` is a unit this player may drive, and why not when it is not.
///
/// Reads the world on every call: a house's defeat and an object's existence both change
/// during a match, and the plan's review was explicit that a permission true at creation
/// does not stay true.
pub fn eligibility(sim: &Simulation, house: InternedId, id: u64) -> Result<(), Ineligible> {
    let Some(entity) = sim.entities().get(id) else {
        return Err(Ineligible::Gone { id });
    };
    let owner = entity.owner();
    // The holder's state, not the asker's: a player may still be alive and well while the
    // house holding this unit is defeated and gone.
    let Some(holder) = sim.houses().get(&owner) else {
        return Err(Ineligible::HouseLeft { house: owner });
    };
    if holder.is_defeated() {
        return Err(Ineligible::HouseDefeated { house: owner });
    }
    if holder.left_game() {
        return Err(Ineligible::HouseLeft { house: owner });
    }
    if owner != house {
        return Err(Ineligible::NotThisHouse { owner, house });
    }
    // The asker's own state too: a defeated player has nothing to plan for, and a caller
    // that keeps trying is a loop that can never succeed.
    if let Some(asker) = sim.houses().get(&house)
        && (asker.is_defeated() || asker.left_game())
    {
        return Err(Ineligible::HouseDefeated { house });
    }
    Ok(())
}
