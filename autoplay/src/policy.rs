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
}

pub struct Policy {
    profiles: Profiles,
    managers: Vec<Box<dyn Manager>>,
    jobs: Assignments,
    last_term: Option<u64>,
    issued: BTreeMap<u64, JobKind>,
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

    fn expected_mission(kind: JobKind) -> Option<MissionId> {
        match kind {
            JobKind::Harvest => Some(MissionId::from_known(MissionType::Harvest)),
            JobKind::Assault => Some(MissionId::from_known(MissionType::Attack)),
            JobKind::Defend => Some(MissionId::from_known(MissionType::AreaGuard)),
            _ => None,
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
            match self.jobs.give(unit, job) {
                Some(previous) if previous.kind != proposal.kind => {
                    self.stats.assigned += 1;
                    self.issued.remove(&unit);
                }
                Some(_) => {}
                None => {
                    self.stats.assigned += 1;
                    self.issued.remove(&unit);
                }
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
            match Self::expected_mission(job.kind) {
                Some(mission) => {
                    let Some(current) = obs
                        .own_entities()
                        .find(|entity| entity.entity_id == *unit)
                        .map(|entity| entity.mission)
                    else {
                        continue;
                    };
                    if current == mission {
                        self.issued.insert(*unit, job.kind);
                        continue;
                    }
                }
                None => {
                    if holding {
                        continue;
                    }
                }
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

        // A unit with no job is the failure this policy exists to prevent.
        self.stats.unowned = eligible
            .iter()
            .filter(|(unit, _)| self.jobs.get(*unit).is_none())
            .count();

        self.issue(obs, out);

        if tick % 600 == 0 {
            log::info!(
                "policy v2: tick={tick} proposals={} intents={} assigned={} issued={} unowned={} refused={}",
                self.stats.proposals,
                self.stats.intents,
                self.stats.assigned,
                self.stats.issued,
                self.stats.unowned,
                self.stats.refused
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
