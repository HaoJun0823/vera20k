//! The two things a brain may ask for, and where each one is applied.
//!
//! - [`Order`]: one object, one [`Goal`]. Translated by the L1 funnel into the
//!   command a click would produce, then admitted and queued.
//! - [`Intent`]: house-scoped work — a build, a placement, a team block — that
//!   a player's hands do through the sidebar rather than by clicking the map.
//!
//! Both are *requests*. Naming them separately from
//! [`Command`](crate::sim::command::Command) is the point of the layer: a brain
//! says what it wants, L2 decides whether the house may issue it now and which
//! native owner performs it. Production, placement, sale and the team block
//! each keep exactly one owner — this module holds no state of its own and
//! re-implements none of them.

use crate::sim::ai_team_creation::TeamCreationPlan;
use crate::sim::behavior::goal::Goal;
use crate::sim::command::Command;
use crate::sim::intern::InternedId;

/// One object order, before admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    pub actor: u64,
    pub goal: Goal,
}

/// House-scoped work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// The house's own team selector raised these teams
    /// (`sim::ai_team_creation`): applied through that module, which owns the
    /// eviction, the `Autocreate=` marks, the new teams and the timer restart.
    RaiseTeams(TeamCreationPlan),
    /// Run the house's own Strategy tick now
    /// (`HouseClass::AI_Building_Strategy`, `0x004FD500`): applied through
    /// `sim::house_strategy`, which owns the enemy search, the defeated-enemy
    /// forget, the superweapon fire, the emergency state with its fire sale and
    /// all-to-hunt, the missing-factory reaction, `AI_Manage_Build_Queue` and
    /// the timer restart.
    ///
    /// Native runs it from `HouseClass::Update` for a house no human controls;
    /// this is the same tick for the house a brain plays.
    AdvanceStrategy,
    /// Ask the type's factory to start building it (`HouseClass::Begin_Production`).
    QueueProduction { type_id: InternedId },
    /// Cancel the queued copies of a type (`HouseClass::Abandon_Production`).
    CancelProduction { type_id: InternedId, all: bool },
    /// Place a finished building that waits for a cell (`Place_Production`).
    PlaceBuilding {
        type_id: InternedId,
        cell: (u16, u16),
    },
    /// Sell a standing building.
    SellBuilding { entity_id: u64 },
}

impl Intent {
    /// The command this request is issued as, when it is one.
    ///
    /// `RaiseTeams` and `AdvanceStrategy` are not commands: a player's hands
    /// cannot raise an AI team or run a house's AI tick, so they are applied
    /// through their owning modules instead.
    pub fn command(self) -> Option<Command> {
        Some(match self {
            Self::RaiseTeams(_) | Self::AdvanceStrategy => return None,
            Self::QueueProduction { type_id } => Command::QueueProduction { type_id },
            Self::CancelProduction { type_id, all } => {
                Command::CancelProductionByType { type_id, all }
            }
            Self::PlaceBuilding { type_id, cell } => Command::PlaceReadyBuilding {
                type_id,
                rx: cell.0,
                ry: cell.1,
            },
            Self::SellBuilding { entity_id } => Command::SellBuilding { entity_id },
        })
    }
}
