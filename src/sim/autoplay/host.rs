//! The auto play driver: observation → agent → gate → command queue.
//!
//! [`AutoPlayHost::step`] is the whole path. It runs once per frame, before the
//! frame advances, and ends at `Simulation::queue_commands` — the same
//! append-only ingress the player's clicks reach. It writes no simulation
//! state of its own.
//!
//! The gate is deliberately minimal. Strict equivalence with a human click is
//! a non-goal (`docs/autoplay.md`), so this does not re-derive `What_Action`,
//! the sidebar's cameo decision or fog. It only stops the agent from issuing a
//! command a player could not have issued at all:
//!
//! - the acting object is owned by the house (`Simulation::entity_owned_by_id`);
//! - the acting object admits orders (`Simulation::order_actor_admits`: alive,
//!   above zero strength, out of limbo);
//! - at most `max_per_frame` commands per frame;
//! - nothing at all while the session's input is locked.
//!
//! Both predicates are the production ones, reused rather than restated:
//! `world_commands.rs` documents that the MegaMission receiver resolves its
//! actor independently of the event house, so without the ownership half an
//! agent could order an object it does not own.
//!
//! Residuals: commands that name no single actor are passed through ungated,
//! including `SetRally` (which names several producers) and every house-scoped
//! production, sale and superweapon command; they still meet their own
//! receiver-side checks. `actor_of` is admission's question — which object
//! must belong to the house — and is separate from
//! `Command::megamission_actor`, which classifies commands for MegaMission
//! scheduling and planning cancellation and is untouched here.

use crate::sim::autoplay::agent::AutoPlayAgent;
use crate::sim::autoplay::observation::AutoPlayView;
use crate::sim::command::{Command, CommandEnvelope};
use crate::sim::intern::InternedId;
use crate::sim::world::Simulation;

/// Drives one agent for one house.
pub struct AutoPlayHost {
    agent: Box<dyn AutoPlayAgent>,
    owner: InternedId,
    max_per_frame: usize,
    rejected: usize,
}

impl AutoPlayHost {
    pub fn new(agent: Box<dyn AutoPlayAgent>, owner: InternedId, max_per_frame: usize) -> Self {
        Self {
            agent,
            owner,
            max_per_frame,
            rejected: 0,
        }
    }

    /// The house every issued command is stamped with.
    pub fn owner(&self) -> InternedId {
        self.owner
    }

    /// Commands the gate has dropped since the host was built. Diagnostics
    /// only: the queue is the authority, not this count.
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// One frame of auto play. Call it before the frame advances.
    pub fn step(&mut self, sim: &mut Simulation) {
        if sim.session().input_locked() {
            return;
        }
        let owner = self.owner;
        if !sim.houses().contains_key(&owner) {
            return;
        }
        let owner_name = sim.interner().resolve(owner).to_owned();
        let commands = self.agent.on_frame(&AutoPlayView::new(sim, owner));
        let execute_tick = sim.session().tick();
        let mut admitted = Vec::new();
        for command in commands {
            if admitted.len() >= self.max_per_frame || !admits(sim, &owner_name, &command) {
                self.rejected += 1;
                continue;
            }
            admitted.push(CommandEnvelope::new(owner, execute_tick, command));
        }
        sim.queue_commands(admitted);
    }
}

/// Whether the house may issue this command, per the gate above.
fn admits(sim: &Simulation, owner: &str, command: &Command) -> bool {
    match actor_of(command) {
        None => true,
        Some(entity_id) => {
            sim.entity_owned_by_id(owner, entity_id) && sim.order_actor_admits(entity_id)
        }
    }
}

/// The object a command acts on, when it names exactly one.
fn actor_of(command: &Command) -> Option<u64> {
    Some(match command {
        Command::Move { entity_id, .. }
        | Command::MoveToObject { entity_id, .. }
        | Command::Stop { entity_id }
        | Command::AttackMove { entity_id, .. }
        | Command::Guard { entity_id, .. }
        | Command::DeployMcv { entity_id }
        | Command::UndeployBuilding { entity_id }
        | Command::SellBuilding { entity_id }
        | Command::ToggleRepair { entity_id }
        | Command::UnloadPassengers {
            transport_id: entity_id,
        }
        | Command::ToggleInfantryDeploy { entity_id }
        | Command::Scatter { entity_id }
        | Command::TogglePrimaryFactory { entity_id }
        | Command::MegaMission { entity_id, .. }
        | Command::SetBuildingOnLine { entity_id, .. }
        | Command::ArchiveObject { entity_id, .. } => *entity_id,
        Command::Attack { attacker_id, .. }
        | Command::ForceAttackCell { attacker_id, .. }
        | Command::PlantC4 { attacker_id, .. } => *attacker_id,
        Command::EjectBunker { bunker_id } => *bunker_id,
        Command::CaptureBuilding { engineer_id, .. } => *engineer_id,
        // House-scoped commands: production, sale of a wall cell, superweapon
        // targeting, speed, exit, diplomacy and the rest. Admission belongs to
        // their receivers.
        _ => return None,
    })
}
