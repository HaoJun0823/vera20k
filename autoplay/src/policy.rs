//! The policy: managers propose, the assignment decides, the outbox answers.
//!
//! One term of this is: read the projection, collect proposals and intents,
//! give each unit at most one job, and issue the order for a job the unit is
//! not already living out. Nothing here names a unit type, and nothing here
//! writes state: every answer leaves through the outbox, which the control
//! layer admits exactly as it admits a player's click.

use std::collections::BTreeMap;

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::behavior::goal::Goal;
use vera20k_core::sim::control::brain::{Brain, Outbox};
use vera20k_core::sim::control::observation::Observation;
use vera20k_core::sim::mission::{MissionId, MissionType};

use crate::job::{Assignments, Job, Proposal, Target};
use crate::manager::abilities::Abilities;
use crate::manager::build::Build;
use crate::manager::economy::Economy;
use crate::manager::military::Military;
use crate::manager::{Ctx, Demands, Manager};
use crate::profile::{Profiles, Roles};
use crate::registry::{JobKind, Manager as Domain};

/// How often the policy re-decides jobs, in frames: one second of the
/// simulation. Reactive decisions do not wait for it; this is the slow loop.
pub const TERM_FRAMES: u64 = 15;

/// What the policy did, for the trace line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub proposals: usize,
    pub intents: usize,
    pub assigned: usize,
    pub issued: usize,
    pub unowned: usize,
    pub refused: usize,
    /// Units that have not changed cell for six seconds while holding a job.
    pub stuck: usize,
}

pub struct Policy {
    profiles: Profiles,
    managers: Vec<Box<dyn Manager>>,
    jobs: Assignments,
    last_term: Option<u64>,
    issued: BTreeMap<u64, JobKind>,
    /// Where each unit stood when it last moved, and when: the stuck counter
    /// reads this, so "a unit that does nothing" is a number, not an opinion.
    stood: BTreeMap<u64, ((u16, u16), u64)>,
    stats: Stats,
}

impl Policy {
    pub fn new(rules: &RuleSet) -> Self {
        Self {
            profiles: Profiles::build(rules),
            managers: vec![
                Box::new(Economy),
                Box::new(Build),
                Box::new(Military),
                Box::new(Abilities),
            ],
            jobs: Assignments::default(),
            last_term: None,
            issued: BTreeMap::new(),
            stood: BTreeMap::new(),
            stats: Stats::default(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn profiles(&self) -> &Profiles {
        &self.profiles
    }

    pub fn jobs(&self) -> &Assignments {
        &self.jobs
    }

    /// Whether a type may hold this job. The predicate reads the profile, never
    /// a name, so a mod's harvester is as good as the stock one.
    fn eligible(&self, kind: JobKind, roles: Roles) -> bool {
        match kind {
            JobKind::Harvest | JobKind::ReturnOre => roles.contains(Roles::HARVESTER),
            JobKind::Deploy => roles.contains(Roles::DEPLOYER),
            JobKind::Repair => roles.contains(Roles::STRUCTURE),
            JobKind::Capture => roles.contains(Roles::ENGINEER) || roles.contains(Roles::AGENT),
            JobKind::Garrison => roles.contains(Roles::OCCUPIER),
            JobKind::Transport => roles.contains(Roles::TRANSPORT),
            JobKind::Breach | JobKind::Assault | JobKind::Defend => roles.contains(Roles::ARMED),
            JobKind::Scout | JobKind::Retreat => true,
        }
    }

    /// Whether the unit's own mission decides that this job is already live.
    ///
    /// A job without one is sent once and never re-sent, because re-sending the
    /// same order every term is what makes a unit unable to act.
    fn watched_by_mission(kind: JobKind) -> bool {
        matches!(kind, JobKind::Harvest | JobKind::Assault | JobKind::Defend)
    }

    /// Whether the unit's current mission already means this job is being lived
    /// out, so the policy has to leave the unit alone.
    ///
    /// The harvester is the case that matters. The engine hands a docking miner
    /// to `Enter` for the refinery contact and `Unload` for the dump
    /// (`MissionType::Enter` queued at `0x0073E093`, the dump under
    /// `Mission_Unload`), so a policy that recognized only `Harvest` would send
    /// a MegaMission into the middle of every dock. A MegaMission is
    /// `Queue_Mission(mission, 1)` (`sim/mission/mega_mission.rs`): it replaces
    /// the queued `Enter` that `per_cell_release_dock_contact` (`0x0073ACD7`)
    /// needs to keep the contact, the radioed refinery is dropped at the next
    /// track end, and the truck cycles back to the ore field still full —
    /// measured at 527 dock entries against 14 empty-hold arrivals in one match
    /// before this change, and one dock per unload after it.
    fn living_out(kind: JobKind, mission: MissionId) -> bool {
        match kind {
            JobKind::Harvest => {
                mission == MissionId::from_known(MissionType::Harvest)
                    || mission == MissionId::from_known(MissionType::Enter)
                    || mission == MissionId::from_known(MissionType::Unload)
            }
            JobKind::Assault => {
                // An attack-move that carries a destination lands on `Move`
                // with the attack-move latch set, not on `Attack` (native
                // `mega_mission.rs::foot_assign_target_command`, the
                // `0x004DF141` destination arm), so `Attack` alone would
                // re-send the order every term — the same failure the
                // harvester's dock cycle has.
                mission == MissionId::from_known(MissionType::Attack)
                    || mission == MissionId::from_known(MissionType::Move)
            }
            JobKind::Defend => mission == MissionId::from_known(MissionType::AreaGuard),
            _ => false,
        }
    }

    fn goal(job: &Job) -> Option<Goal> {
        match (job.kind, job.target) {
            (JobKind::Harvest, Target::Cell(cell)) => Some(Goal::Harvest { cell }),
            (JobKind::Assault, Target::Cell(cell)) => Some(Goal::AttackMove { cell }),
            (JobKind::Defend, Target::Cell(cell)) => Some(Goal::GuardAt { cell }),
            (JobKind::Deploy, _) => Some(Goal::Deploy),
            _ => None,
        }
    }

    /// Give each unit at most one job, strongest proposal first. Deterministic:
    /// the order is priority, then kind, then unit id.
    fn assign(&mut self, mut proposals: Vec<Proposal>, tick: u64) {
        proposals.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| match (a.actor, b.actor) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    _ => std::cmp::Ordering::Equal,
                })
        });
        for proposal in proposals {
            let Some(unit) = proposal.actor else {
                // A demand no unit can fill yet: counted, never silently
                // dropped.
                self.stats.unowned += 1;
                continue;
            };
            let job = Job {
                kind: proposal.kind,
                target: proposal.target,
                priority: proposal.priority,
                given: tick,
            };
            // The strongest proposal wins: a later, weaker one must not
            // overwrite the job the unit was just given.
            if let Some(current) = self.jobs.get(unit)
                && current.priority >= job.priority
            {
                continue;
            }
            let changed = match self.jobs.give(unit, job) {
                Some(previous) => previous.kind != proposal.kind,
                None => true,
            };
            if changed {
                self.stats.assigned += 1;
                self.issued.remove(&unit);
            }
        }
    }

    /// Issue the order for a job the unit is not already living out.
    ///
    /// A unit whose mission already names the job is left alone; re-sending the
    /// same order every term is what makes a unit unable to fire. A one-shot
    /// job, one with no mission of its own, is sent once.
    fn issue(&mut self, obs: &Observation<'_>, out: &mut Outbox) {
        let mut orders = Vec::new();
        for (unit, job) in self.jobs.iter() {
            let Some(goal) = Self::goal(job) else {
                continue;
            };
            let holding = self.issued.get(unit) == Some(&job.kind);
            if Self::watched_by_mission(job.kind) {
                let Some(current) = obs
                    .own_entities()
                    .find(|entity| entity.entity_id == *unit)
                    .map(|entity| entity.mission)
                else {
                    continue;
                };
                if Self::living_out(job.kind, current) {
                    self.issued.insert(*unit, job.kind);
                    continue;
                }
            } else if holding {
                continue;
            }
            orders.push((*unit, goal, job.kind));
        }
        for (unit, goal, kind) in orders {
            out.order(unit, goal);
            self.issued.insert(unit, kind);
            self.stats.issued += 1;
        }
    }
}

impl Brain for Policy {
    fn name(&self) -> &'static str {
        "v2"
    }

    fn think(&mut self, rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
        let tick = obs.tick();
        let due = self.last_term.is_none_or(|last| tick >= last + TERM_FRAMES);
        if !due {
            return;
        }
        self.last_term = Some(tick);
        self.stats = Stats::default();

        let mut demands = Demands::default();
        {
            let ctx = Ctx {
                rules,
                obs,
                profiles: &self.profiles,
                tick,
            };
            for manager in &mut self.managers {
                manager.propose(&ctx, &mut demands);
            }
        }

        // Only units that exist and can take an order are eligible.
        let mut eligible = Vec::new();
        for unit in obs.own_entities() {
            let Some(ty) = obs.object_type(rules, unit.type_id) else {
                continue;
            };
            let Some(profile) = self.profiles.get(&ty.id) else {
                continue;
            };
            // A structure takes house-level intents, not a unit job: counting
            // one as unowned would hide the units that really are.
            if unit.orderable
                && !profile.roles.contains(Roles::STRUCTURE)
                && obs.inside_transport(unit.entity_id).is_none()
            {
                eligible.push((unit.entity_id, profile.roles));
            }
        }
        demands.jobs.retain(|proposal| {
            let Some(unit) = proposal.actor else {
                return true;
            };
            eligible
                .iter()
                .any(|(id, roles)| *id == unit && self.eligible(proposal.kind, *roles))
        });
        self.stats.proposals = demands.jobs.len();
        self.stats.intents = demands.intents.len();
        self.assign(demands.jobs, tick);

        for intent in demands.intents {
            out.intent(intent);
        }

        // A unit that has not moved for six seconds while holding a job is
        // stuck: the one reading the old policy had no way to report.
        const STUCK_FRAMES: u64 = 90;
        self.stats.stuck = 0;
        let mut stood = BTreeMap::new();
        let mut stuck = Vec::new();
        for unit in obs.own_entities() {
            // Holding a post is standing still on purpose; only a job that
            // names somewhere to be counts as stuck when the cell stops
            // changing.
            let held = self.jobs.get(unit.entity_id).is_some_and(|job| {
                matches!(
                    job.kind,
                    JobKind::Harvest
                        | JobKind::ReturnOre
                        | JobKind::Assault
                        | JobKind::Breach
                        | JobKind::Scout
                        | JobKind::Transport
                )
            });
            let since = match self.stood.get(&unit.entity_id) {
                Some((cell, since)) if *cell == unit.cell => *since,
                _ => tick,
            };
            if held && tick.saturating_sub(since) >= STUCK_FRAMES {
                self.stats.stuck += 1;
                // "A unit did nothing" has to be actionable, so the reading
                // names the unit, its job and where it stands still.
                if let Some(job) = self.jobs.get(unit.entity_id) {
                    stuck.push(format!(
                        "{} id={} job={:?} target={:?} cell={:?} still={}",
                        obs.object_type(rules, unit.type_id)
                            .map_or("?", |ty| ty.id.as_str()),
                        unit.entity_id,
                        job.kind,
                        job.target,
                        unit.cell,
                        tick.saturating_sub(since)
                    ));
                }
            }
            stood.insert(unit.entity_id, (unit.cell, since));
        }
        self.stood = stood;

        // A unit with no job is the failure this policy exists to prevent.
        self.stats.unowned = eligible
            .iter()
            .filter(|(unit, _)| self.jobs.get(*unit).is_none())
            .count();

        self.issue(obs, out);

        if tick % 600 == 0 && self.stats.unowned > 0 {
            let idle: Vec<String> = obs
                .own_entities()
                .filter(|unit| self.jobs.get(unit.entity_id).is_none())
                .filter_map(|unit| obs.object_type(rules, unit.type_id).map(|ty| ty.id.clone()))
                .collect();
            log::info!("policy v2: unowned at {tick}: {:?}", idle);
        }
        if tick % 600 == 0 && !stuck.is_empty() {
            log::info!("policy v2: stuck at {tick}: {:?}", stuck);
        }
        if tick % 600 == 0 {
            log::info!(
                "policy v2: tick={tick} proposals={} intents={} assigned={} issued={} unowned={} refused={} stuck={}",
                self.stats.proposals,
                self.stats.intents,
                self.stats.assigned,
                self.stats.issued,
                self.stats.unowned,
                self.stats.refused,
                self.stats.stuck
            );
        }
    }

    fn orders_refused(&mut self, refused: &[(u64, Goal)]) {
        for (unit, _) in refused {
            if self.jobs.take(*unit).is_some() {
                self.stats.refused += 1;
            }
            self.issued.remove(unit);
        }
    }
}

/// The managers the registry names, for a test that the two lists agree.
pub fn manager_domains() -> [Domain; 4] {
    [
        Domain::Economy,
        Domain::Build,
        Domain::Military,
        Domain::Abilities,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mission(known: MissionType) -> MissionId {
        MissionId::from_known(known)
    }

    #[test]
    fn a_docking_harvester_is_already_harvesting() {
        // The engine's own dock hands the miner to Enter and Unload; a policy
        // that ordered it again here would break the dock it is waiting on.
        for known in [
            MissionType::Harvest,
            MissionType::Enter,
            MissionType::Unload,
        ] {
            assert!(
                Policy::living_out(JobKind::Harvest, mission(known)),
                "a miner on the harvest cycle must be left alone"
            );
        }
    }

    #[test]
    fn a_harvester_that_stopped_is_ordered_again() {
        for idle in [
            MissionId::NONE,
            mission(MissionType::Guard),
            mission(MissionType::AreaGuard),
        ] {
            assert!(
                !Policy::living_out(JobKind::Harvest, idle),
                "a truck that is not harvesting has to be told to harvest"
            );
        }
    }

    #[test]
    fn an_attacking_unit_is_not_re_ordered_every_term() {
        // The policy's assault order is an attack-move with a destination, and
        // the engine lands that on Move with the latch set; a predicate that
        // knew only Attack would spam the order.
        for on_the_way in [MissionType::Attack, MissionType::Move] {
            assert!(
                Policy::living_out(JobKind::Assault, mission(on_the_way)),
                "an attack-move is not a unit waiting for orders"
            );
        }
        for idle in [MissionId::NONE, mission(MissionType::Guard)] {
            assert!(!Policy::living_out(JobKind::Assault, idle));
        }
    }

    #[test]
    fn only_a_job_with_a_mission_of_its_own_is_watched() {
        for watched in [JobKind::Harvest, JobKind::Assault, JobKind::Defend] {
            assert!(Policy::watched_by_mission(watched));
        }
        for once in [
            JobKind::Deploy,
            JobKind::Capture,
            JobKind::Garrison,
            JobKind::Repair,
            JobKind::Transport,
            JobKind::Breach,
            JobKind::Scout,
            JobKind::Retreat,
            JobKind::ReturnOre,
        ] {
            assert!(
                !Policy::watched_by_mission(once),
                "sent once, never re-sent"
            );
        }
    }
}
