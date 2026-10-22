//! The control board: the one place a frame's requests become commands.
//!
//! Per frame, in this order:
//!
//! 1. the brain's **state** requests are applied through their owners (the
//!    team block is the only one today);
//! 2. the brain **thinks** against a fresh projection and answers orders and
//!    intents;
//! 3. attached objects are stepped — garbage-collected, then the rotation for
//!    this frame — and each routine's answer becomes a goal;
//! 4. every order and intent is admitted: domain switch, per-frame budget, the
//!    acting object's ownership and the native order gate;
//! 5. what survived is stamped with the house and the current tick and queued
//!    through `Simulation::queue_commands`, the append-only ingress a player's
//!    clicks use.
//!
//! Step 4 is why this layer exists: the same admission applies to a retail
//! brain, a script and a managed routine, and a refusal is *counted and
//! reported*, never silent. A unit that stops is either waiting on a native
//! transition (`NotReady`) or has left the house's control (rejected), and the
//! counters say which.
//!
//! Nothing above this file may issue a command on its own — that is the whole
//! contract. The board writes no simulation state except through the owners it
//! calls, and owns no gameplay rule.

use crate::rules::ruleset::RuleSet;
use crate::sim::behavior::managed::{FallbackReason, MicroKind, MicroParams};
use crate::sim::behavior::order::{ApplyOutcome, Funnel};
use crate::sim::behavior::view::ThreatView;
use crate::sim::behavior::{Goal, MicroStep};
use crate::sim::command::{Command, CommandEnvelope};
use crate::sim::control::brain::{Brain, Outbox};
use crate::sim::control::budget::Budget;
use crate::sim::control::intent::Intent;
use crate::sim::control::micro::MicroTable;
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;
use crate::sim::world::{FrameEffects, Simulation};

/// Which half of the work the player lets auto play do.
///
/// Two keys, two switches: the master toggle installs the board with both, and
/// a second key flips the forces half on its own, so a player can keep the base
/// growing while commanding the troops, or hand the troops over and take
/// building back. Turning one off stops new commands of that kind; it never
/// undoes what was already issued or queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoPlayDomains {
    /// Production, building and placement: the base's own work.
    pub production: bool,
    /// Unit orders, superweapon use and managed routines.
    pub forces: bool,
}

impl AutoPlayDomains {
    pub const ALL: Self = Self {
        production: true,
        forces: true,
    };

    /// Whether a command of this kind may leave the board now.
    pub fn admits(&self, command: &Command) -> bool {
        if belongs_to_production(command) {
            self.production
        } else {
            self.forces
        }
    }

    /// The switch as a player-facing phrase, for the message line.
    pub fn describe(&self) -> &'static str {
        match (self.production, self.forces) {
            (true, true) => "build + forces",
            (true, false) => "build only",
            (false, true) => "forces only",
            (false, false) => "paused",
        }
    }
}

/// Whether a command is the base's work rather than the troops'.
///
/// Production, placement and the commands that tidy a build up are the base's;
/// everything that moves, aims, fires, captures or launches is the forces'.
/// `DeployMcv` counts as the base's because it is how a base starts.
fn belongs_to_production(command: &Command) -> bool {
    matches!(
        command,
        Command::QueueProduction { .. }
            | Command::SuspendProduction { .. }
            | Command::CycleProducerFocus { .. }
            | Command::PlaceReadyBuilding { .. }
            | Command::CancelProductionByType { .. }
            | Command::SellBuilding { .. }
            | Command::ToggleRepair { .. }
            | Command::SetRally { .. }
            | Command::TogglePrimaryFactory { .. }
            | Command::DeployMcv { .. }
            | Command::UndeployBuilding { .. }
    )
}

/// Drives one brain and one house.
pub struct ControlBoard {
    brain: Box<dyn Brain>,
    owner: InternedId,
    domains: AutoPlayDomains,
    budget: Budget,
    micro: MicroTable,
    /// Commands the gate dropped, and orders the funnel called not-ready.
    /// Diagnostics: the queue is the authority, not these counts.
    rejected: usize,
    not_ready: usize,
}

impl ControlBoard {
    pub fn new(brain: Box<dyn Brain>, owner: InternedId, max_commands_per_frame: usize) -> Self {
        Self {
            brain,
            owner,
            domains: AutoPlayDomains::ALL,
            budget: Budget::new(max_commands_per_frame),
            micro: MicroTable::default(),
            rejected: 0,
            not_ready: 0,
        }
    }

    /// The house every issued command is stamped with. Not necessarily the
    /// local player's: a computer house can be driven the same way.
    pub fn owner(&self) -> InternedId {
        self.owner
    }

    /// Which halves of the work the board lets through now.
    pub fn domains(&self) -> AutoPlayDomains {
        self.domains
    }

    /// Flip the switches. A kind turned off from here on issues nothing more of
    /// its own; what is already queued in the simulation keeps running.
    pub fn set_domains(&mut self, domains: AutoPlayDomains) {
        self.domains = domains;
    }

    /// The brain driving the house, for diagnostics.
    pub fn brain_name(&self) -> &'static str {
        self.brain.name()
    }

    pub fn rejected(&self) -> usize {
        self.rejected
    }

    pub fn not_ready(&self) -> usize {
        self.not_ready
    }

    pub fn managed(&self) -> &MicroTable {
        &self.micro
    }

    /// Put an object under a managed routine, with an optional post. `false`
    /// when it is already managed.
    pub fn attach(
        &mut self,
        entity: u64,
        kind: MicroKind,
        params: MicroParams,
        post: Option<(u16, u16)>,
    ) -> bool {
        self.micro.attach(entity, kind, params, post)
    }

    /// Return an object to the brain's direct orders.
    pub fn detach(&mut self, entity: u64) -> bool {
        self.micro.detach(entity).is_some()
    }

    /// One frame of auto play. Call it before the frame advances.
    pub fn step(&mut self, sim: &mut Simulation, rules: &RuleSet) {
        if sim.session().input_locked() {
            return;
        }
        let owner = self.owner;
        if !sim.houses().contains_key(&owner) {
            return;
        }
        // The brain plays the house's AI blocks as well as its commands, so the
        // sim's own gates are told once that a brain owns it: `house_state`
        // owns the latch, and the factory-exit economy block and everything
        // else that gates on "no human controls it" reads it.
        if let Some(house) = sim.houses.get_mut(&owner) {
            house.set_policy_played();
        }
        self.budget.begin_frame();
        let owner_name = sim.interner().resolve(owner).to_owned();

        let mut pending: Vec<Command> = Vec::new();

        // 1. State the brain asks the simulation to hold, through its owners.
        let mut state = Outbox::default();
        {
            let obs = Observation::new(sim, owner);
            self.brain.state(rules, &obs, &mut state);
        }
        for intent in state.intents {
            self.apply_state_intent(sim, rules, owner, intent);
        }

        // 2. Decisions.
        let mut out = Outbox::default();
        {
            let obs = Observation::new(sim, owner);
            self.brain.think(rules, &obs, &mut out);
        }
        for intent in out.intents {
            if matches!(intent, Intent::RaiseTeams(_) | Intent::AdvanceStrategy) {
                self.apply_state_intent(sim, rules, owner, intent);
            } else if let Some(command) = intent.command() {
                self.admit(sim, &owner_name, &command, &mut pending);
            }
        }
        for command in &out.commands {
            self.admit(sim, &owner_name, command, &mut pending);
        }
        for order in &out.orders {
            if self.micro.is_managed(order.actor) {
                continue;
            }
            self.issue(
                sim,
                rules,
                &owner_name,
                order.actor,
                order.goal,
                &mut pending,
            );
        }

        // 3. Managed objects, when the forces half is on.
        if self.domains.forces {
            self.step_managed(sim, rules, &owner_name, &mut pending);
        }

        // 4. One ingress, stamped for this frame.
        let execute_tick = sim.session().tick();
        let envelopes = pending
            .into_iter()
            .map(|command| CommandEnvelope::new(owner, execute_tick, command));
        sim.queue_commands(envelopes);
    }

    /// Apply one intent that is house state rather than a command.
    ///
    /// A team block and a Strategy tick are both things `HouseClass::Update`
    /// does for a house no human controls; neither rides the command queue, so
    /// each is applied by its owning module here.
    fn apply_state_intent(
        &mut self,
        sim: &mut Simulation,
        rules: &RuleSet,
        owner: InternedId,
        intent: Intent,
    ) {
        match intent {
            Intent::RaiseTeams(plan) => self.raise_teams(sim, rules, owner, plan),
            Intent::AdvanceStrategy => crate::sim::house_strategy::update_strategy_policy(
                sim,
                rules,
                owner,
                None,
                FrameEffects::default(),
            ),
            // Nothing else is house state; an intent that turns up here has a
            // command and reaches the queue through `Intent::command`.
            _ => {}
        }
    }

    /// Apply a raised team block through its owner.
    ///
    /// A team is not a command: a player's hands cannot raise one, so it cannot
    /// ride the queue. `sim::ai_team_creation` applies it — the eviction, the
    /// `Autocreate=` marks, the teams and the timer restart — for a house its
    /// own gate would skip.
    fn raise_teams(
        &mut self,
        sim: &mut Simulation,
        rules: &RuleSet,
        owner: InternedId,
        plan: crate::sim::ai_team_creation::TeamCreationPlan,
    ) {
        let frame = sim.session().binary_frame() as i32;
        crate::sim::ai_team_creation::apply_team_creation(
            sim,
            rules,
            owner,
            plan,
            frame,
            FrameEffects::default(),
        );
    }

    /// Step this frame's rotation of attached objects and issue what they ask.
    fn step_managed(
        &mut self,
        sim: &mut Simulation,
        rules: &RuleSet,
        owner_name: &str,
        pending: &mut Vec<Command>,
    ) {
        let owner = self.owner;
        for (entity, reason) in self.micro.retain_live(sim, owner) {
            log::info!(
                "retail-ai: {} handed back to the brain ({})",
                entity,
                reason.describe()
            );
        }
        if self.micro.is_empty() {
            return;
        }
        let ids = self
            .micro
            .rotation(self.budget.rotation_start(), self.budget.micro_steps());
        if ids.is_empty() {
            return;
        }

        let mut steps: Vec<(u64, MicroStep)> = Vec::new();
        {
            let threats = ThreatView::new(sim, owner);
            for &entity in &ids {
                if let Some(step) = self.micro.step_one(entity, sim, rules, &threats) {
                    steps.push((entity, step));
                }
            }
        }
        if let Some(&last) = ids.last() {
            self.budget.set_rotation_start(last.wrapping_add(1));
        }

        for (entity, step) in steps {
            match step {
                MicroStep::Wait { .. } => {}
                MicroStep::Done => {
                    self.micro.detach(entity);
                }
                MicroStep::Fallback(reason) => {
                    self.fall_back(entity, reason);
                }
                MicroStep::Goal(goal) => {
                    let outcome = self.issue(sim, rules, owner_name, entity, goal, pending);
                    if let Some(reason) = self.micro.record_outcome(entity, outcome.is_queued()) {
                        self.fall_back(entity, reason);
                    }
                }
            }
        }
    }

    fn fall_back(&mut self, entity: u64, reason: FallbackReason) {
        self.micro.detach(entity);
        log::info!(
            "retail-ai: micro routine on {} handed back to the brain ({})",
            entity,
            reason.describe()
        );
    }

    /// Turn one goal into a command and admit it.
    fn issue(
        &mut self,
        sim: &Simulation,
        rules: &RuleSet,
        owner_name: &str,
        actor: u64,
        goal: Goal,
        pending: &mut Vec<Command>,
    ) -> ApplyOutcome {
        let funnel = Funnel::new(sim, rules, self.owner);
        match funnel.prepare(actor, goal) {
            Ok(command) => {
                if self.admit(sim, owner_name, &command, pending) {
                    ApplyOutcome::Queued
                } else {
                    ApplyOutcome::Rejected
                }
            }
            Err(refusal) => {
                let outcome = refusal.outcome();
                if outcome == ApplyOutcome::NotReady {
                    self.not_ready += 1;
                } else {
                    self.rejected += 1;
                }
                outcome
            }
        }
    }

    /// The gate: budget, domain switch, ownership and the native order gate.
    fn admit(
        &mut self,
        sim: &Simulation,
        owner_name: &str,
        command: &Command,
        pending: &mut Vec<Command>,
    ) -> bool {
        if !self.domains.admits(command) || !admits(sim, owner_name, command) {
            self.rejected += 1;
            return false;
        }
        if !self.budget.try_command() {
            self.rejected += 1;
            return false;
        }
        pending.push(command.clone());
        true
    }
}

/// Whether the house may issue this command: the acting object, when the
/// command names exactly one, must be the house's and must admit orders.
///
/// Both predicates are the production ones, reused rather than restated:
/// `world_commands.rs` documents that the MegaMission receiver resolves its
/// actor independently of the event house, so without the ownership half an
/// agent could order an object it does not own.
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
