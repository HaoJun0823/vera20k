//! The order vocabulary: what a player's click means, as [`Goal`]s.
//!
//! One variant per action the game already understands, so the vocabulary adds
//! no behaviour of its own: [`Goal::command`] produces the same
//! [`Command`](crate::sim::command::Command) the input layer produces for that
//! click, and the existing receivers do the rest. Nothing here inspects the
//! world; a goal is a request, and [`super::order`] decides whether it may be
//! issued now.
//!
//! Adding a goal is one variant plus its `command` arm. Because the mapping is
//! an exhaustive `match`, the compiler lists the micro routines that must
//! decide what to do with it — see `behavior/managed/mod.rs`.

use crate::sim::combat::TargetKind;
use crate::sim::command::Command;
use crate::sim::mission::MissionType;

/// A single object-scoped order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    /// Attack the object with this stable id (`Command::Attack`).
    Attack { target: u64 },
    /// Force-fire on a cell (`Command::ForceAttackCell`).
    AttackCell { cell: (u16, u16) },
    /// Attack-move to a cell (`Command::AttackMove`).
    AttackMove { cell: (u16, u16) },
    /// Move to a cell (`Command::Move`).
    Move { cell: (u16, u16) },
    /// Move onto an object (`Command::MoveToObject`).
    MoveToObject { target: u64 },
    /// Area guard where the object stands (`Command::Guard`, null post).
    Guard,
    /// Area guard around a post (`Command::Guard` with a cell post).
    GuardAt { cell: (u16, u16) },
    /// The Capture click on a building (`Command::CaptureBuilding`).
    Capture { target: u64 },
    /// Enter a building or transport (`Command::MegaMission`, mission Enter).
    Enter { target: u64 },
    /// Harvest an ore cell (`Command::MegaMission`, mission Harvest).
    Harvest { cell: (u16, u16) },
    /// Deploy an MCV or a simple deployer (`Command::DeployMcv`).
    Deploy,
    /// Stop (`Command::Stop`).
    Stop,
    /// Scatter (`Command::Scatter`).
    Scatter,
    /// Unload a transport or garrison (`Command::UnloadPassengers`).
    Unload,
}

impl Goal {
    /// The command this goal is issued as, for its actor.
    ///
    /// Every arm is a command the input layer already produces for the same
    /// click, so the goal adds no receiver and no second admission rule. The
    /// MEGAMISSION family reaches `Simulation::begin_megamission_retask`
    /// through `Simulation::queue_megamission`, the single native retask
    /// funnel, exactly as a player's click does.
    pub fn command(self, actor: u64) -> Command {
        match self {
            Self::Attack { target } => Command::Attack {
                attacker_id: actor,
                target_id: target,
            },
            Self::AttackCell { cell } => Command::ForceAttackCell {
                attacker_id: actor,
                target_rx: cell.0,
                target_ry: cell.1,
            },
            Self::AttackMove { cell } => Command::AttackMove {
                entity_id: actor,
                target: None,
                destination: Some(TargetKind::Cell(cell.0, cell.1)),
            },
            Self::Move { cell } => Command::Move {
                entity_id: actor,
                target_rx: cell.0,
                target_ry: cell.1,
            },
            Self::MoveToObject { target } => Command::MoveToObject {
                entity_id: actor,
                target_id: target,
            },
            Self::Guard => Command::Guard {
                entity_id: actor,
                target: None,
            },
            Self::GuardAt { cell } => Command::Guard {
                entity_id: actor,
                target: Some(TargetKind::Cell(cell.0, cell.1)),
            },
            Self::Capture { target } => Command::CaptureBuilding {
                engineer_id: actor,
                target_building_id: target,
            },
            Self::Enter { target } => Command::MegaMission {
                entity_id: actor,
                mission: MissionType::Enter,
                target: Some(TargetKind::Entity(target)),
                destination: None,
            },
            Self::Harvest { cell } => Command::MegaMission {
                entity_id: actor,
                mission: MissionType::Harvest,
                target: None,
                destination: Some(TargetKind::Cell(cell.0, cell.1)),
            },
            Self::Deploy => Command::DeployMcv { entity_id: actor },
            Self::Stop => Command::Stop { entity_id: actor },
            Self::Scatter => Command::Scatter { entity_id: actor },
            Self::Unload => Command::UnloadPassengers {
                transport_id: actor,
            },
        }
    }
}
