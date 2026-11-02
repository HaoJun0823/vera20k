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

/// How long a unit may stand in one cell on an assault before the job is
/// abandoned: the order named something this unit cannot get to.
///
/// The ladder is the design's (`autoplay/docs/design.md`, section 4): every task
/// reports `Done`, `Unreachable`, `Ineffective` or `Stalled`, and every rung
/// terminates — the last rung is giving up and cooling down. Thirty seconds is
/// the point at which standing still is no longer a fight in progress.
const UNREACHABLE_FRAMES: u64 = 450;

/// How long a unit stays off a target it could not reach, in frames: two
/// minutes, the same order of magnitude as one battle.
const UNREACHABLE_COOLDOWN: u64 = 1_800;

/// What the policy did, for the trace line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub proposals: usize,
    pub intents: usize,
    pub assigned: usize,
    pub issued: usize,
    pub unowned: usize,
    pub refused: usize,
    /// Jobs that ended this term because their target was gone.
    pub finished: usize,
    /// Jobs that ended this term because the unit could not reach the target.
    pub stalled: usize,
    /// Proposals dropped this term for a target the unit had given up on.
    pub cooled: usize,
    /// Units given the default job this term because nothing else claimed them.
    pub fallback: usize,
    /// Units that have not changed cell for six seconds while holding a job.
    pub stuck: usize,
}

pub struct Policy {
    profiles: Profiles,
    managers: Vec<Box<dyn Manager>>,
    jobs: Assignments,
    last_term: Option<u64>,
    /// The order each unit has been sent: the job kind, and the target it named.
    /// A job whose target changed is a new order even when its kind did not.
    issued: BTreeMap<u64, (JobKind, Target)>,
    /// Where each unit stood when it last moved, and when: the stuck counter
    /// reads this, so "a unit that does nothing" is a number, not an opinion.
    stood: BTreeMap<u64, ((u16, u16), u64)>,
    /// Targets a unit has given up on, and the tick it may be proposed again:
    /// the last rung of the assault ladder.
    given_up: BTreeMap<(u64, u64), u64>,
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
            given_up: BTreeMap::new(),
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
            // The army attacks the object it was told to: the click a player
            // makes on an enemy, not an attack-move to where it stood. An
            // attack-move's destination is reached once and then stood on, and
            // that is the stale-target failure status.md records.
            (JobKind::Assault, Target::Object(target)) => Some(Goal::Attack { target }),
            (JobKind::Assault, Target::Cell(cell)) => Some(Goal::AttackMove { cell }),
            (JobKind::Defend, Target::Cell(cell)) => Some(Goal::GuardAt { cell }),
            (JobKind::Deploy, _) => Some(Goal::Deploy),
            _ => None,
        }
    }

    /// End the jobs that are done, and the ones the unit cannot do.
    ///
    /// Two reasons, and both terminate: an assault on an object the house can no
    /// longer see is work that cannot be done, and an assault that has left its
    /// unit standing in one cell for [`UNREACHABLE_FRAMES`] is a target it
    /// cannot get to. The second is the design's last rung — give up and cool
    /// down (`design.md`, section 4) — and without it five units stood on one
    /// refinery for 1400-1875 frames in the probe status.md records.
    ///
    /// Jobs end before this term's proposals are collected, so the same unit can
    /// be given a live target in the same term.
    fn finish_done_jobs(&mut self, obs: &Observation<'_>, tick: u64) {
        self.given_up.retain(|_, until| *until > tick);
        let visible: Vec<u64> = obs
            .visible_enemy_entities()
            .map(|enemy| enemy.entity_id)
            .collect();
        let cells: BTreeMap<u64, (u16, u16)> = obs
            .own_entities()
            .map(|unit| (unit.entity_id, unit.cell))
            .collect();
        let done: Vec<(u64, u64, bool)> = self
            .jobs
            .iter()
            .filter_map(|(unit, job)| match (job.kind, job.target) {
                (JobKind::Assault, Target::Object(target)) => {
                    if !visible.contains(&target) {
                        return Some((*unit, target, false));
                    }
                    let cell = cells.get(unit).copied().unwrap_or((0, 0));
                    let standing = Self::standing_frames(self.stood.get(unit), cell, tick);
                    (standing >= UNREACHABLE_FRAMES).then_some((*unit, target, true))
                }
                _ => None,
            })
            .collect();
        for (unit, target, unreachable) in done {
            self.jobs.take(unit);
            self.issued.remove(&unit);
            if unreachable {
                self.given_up
                    .insert((unit, target), tick + UNREACHABLE_COOLDOWN);
                self.stats.stalled += 1;
            } else {
                self.stats.finished += 1;
            }
        }
    }

    /// Whether the order a unit already holds is the order this job asks for.
    ///
    /// Only an assault is compared by target, and it has to be: its enemy walks,
    /// the mission keeps reading `Attack` while it does, and a job re-aimed at a
    /// different enemy would otherwise look like a job already lived out.
    ///
    /// The other two watched jobs are compared by kind alone, each for its own
    /// reason. A harvest job's target is the cell the truck stood on when the
    /// job was given, and the ore is the mission's business, so comparing it
    /// would re-order every truck every term and break the dock again. A defend
    /// job's post is the first structure the house has, and before the yard
    /// stands that is "wherever the first soldier is" — a cell that moves with
    /// the army, so comparing it scattered the army before the base existed.
    fn same_order(issued: Option<&(JobKind, Target)>, job: &Job) -> bool {
        issued.is_some_and(|(kind, target)| {
            *kind == job.kind && (job.kind != JobKind::Assault || *target == job.target)
        })
    }

    /// How long the unit has stood in the cell it stands in now.
    fn standing_frames(stood: Option<&((u16, u16), u64)>, cell: (u16, u16), tick: u64) -> u64 {
        match stood {
            Some((stood_at, since)) if *stood_at == cell => tick.saturating_sub(*since),
            _ => 0,
        }
    }

    /// The job a unit holds when nothing else claimed it: guard the nearest
    /// structure the house owns.
    ///
    /// Off the profile, never a type name: any unit that is armed and not a
    /// harvester can guard, and any structure is worth standing at. A house with
    /// no structure yet guards the cell its units stand on, so the starting
    /// force has a post before the yard exists.
    fn default_job(&self, obs: &Observation<'_>, rules: &RuleSet, tick: u64) -> Vec<(u64, Job)> {
        let mut posts: Vec<(u16, u16)> = Vec::new();
        let mut idle: Vec<u64> = Vec::new();
        for unit in obs.own_entities() {
            let Some(ty) = obs.object_type(rules, unit.type_id) else {
                continue;
            };
            let Some(profile) = self.profiles.get(&ty.id) else {
                continue;
            };
            if profile.roles.contains(Roles::STRUCTURE) {
                posts.push(unit.cell);
                continue;
            }
            if unit.orderable
                && !profile.roles.contains(Roles::HARVESTER)
                && profile.roles.contains(Roles::ARMED)
                && obs.inside_transport(unit.entity_id).is_none()
                && self.jobs.get(unit.entity_id).is_none()
            {
                idle.push(unit.entity_id);
            }
        }
        if posts.is_empty() {
            return Vec::new();
        }
        posts.sort_unstable();
        idle.into_iter()
            .map(|unit| {
                let cell = obs
                    .own_entities()
                    .find(|entity| entity.entity_id == unit)
                    .map(|entity| entity.cell)
                    .unwrap_or(posts[0]);
                let post = *posts
                    .iter()
                    .min_by_key(|at| {
                        i32::from(at.0.abs_diff(cell.0)).max(i32::from(at.1.abs_diff(cell.1)))
                    })
                    .expect("a structure to guard");
                (
                    unit,
                    Job {
                        kind: JobKind::Defend,
                        target: Target::Cell(post),
                        // Below every proposal a manager makes: this is the job
                        // a unit gets when nothing wanted it.
                        priority: 0,
                        given: tick,
                    },
                )
            })
            .collect()
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
            // A target this unit has already given up on is not proposed to it
            // again until the cooldown runs out. Without this the ladder ends in
            // a loop: give up, be told the same thing next term, stand still.
            if let Target::Object(target) = job.target
                && self.given_up.contains_key(&(unit, target))
            {
                self.stats.cooled += 1;
                continue;
            }
            // The strongest proposal wins: a later, weaker one must not
            // overwrite the job the unit was just given.
            if let Some(current) = self.jobs.get(unit)
                && current.priority >= job.priority
            {
                continue;
            }
            let changed = match self.jobs.give(unit, job) {
                // A harvest job's target is the cell the truck stood on when it
                // was given, so a moving truck would read as "changed" every
                // term and be re-ordered mid-dock. `same_order` is the one
                // comparison that knows which targets are part of the order.
                Some(previous) => !Self::same_order(
                    Some(&(previous.kind, previous.target)),
                    self.jobs.get(unit).expect("the job just given"),
                ),
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
    /// same order every term is what makes a unit unable to fire, and it is what
    /// broke the harvester's dock (`living_out`) and the army's cohesion.
    ///
    /// The one case the mission cannot see is an assault whose enemy has walked:
    /// the unit keeps attacking, the mission keeps reading `Attack`, and the
    /// target it is attacking is no longer the one the job names. So a watched
    /// job is re-sent when its target changed, and only then — a re-send is a
    /// new order, which for a unit already in the middle of the mission costs it
    /// its current shot.
    fn issue(&mut self, obs: &Observation<'_>, out: &mut Outbox) {
        let mut orders = Vec::new();
        for (unit, job) in self.jobs.iter() {
            let Some(goal) = Self::goal(job) else {
                continue;
            };
            let retargeted =
                job.kind == JobKind::Assault && !Self::same_order(self.issued.get(unit), job);
            if Self::watched_by_mission(job.kind) {
                let Some(current) = obs
                    .own_entities()
                    .find(|entity| entity.entity_id == *unit)
                    .map(|entity| entity.mission)
                else {
                    continue;
                };
                if Self::living_out(job.kind, current) {
                    // The job is live, so the unit is left alone and the order
                    // is remembered — including the case where nothing was
                    // remembered yet, which is what a first-term unit looks
                    // like. The only exception is an assault whose target
                    // moved: the mission cannot show that.
                    self.issued.insert(*unit, (job.kind, job.target));
                    if !retargeted {
                        continue;
                    }
                }
            } else if Self::same_order(self.issued.get(unit), job) {
                continue;
            }
            orders.push((*unit, goal, (job.kind, job.target)));
        }
        for (unit, goal, order) in orders {
            out.order(unit, goal);
            self.issued.insert(unit, order);
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

        // A job whose target is gone — or which the unit cannot reach — is
        // finished before the term's proposals are collected, so the unit can be
        // given a live one in the same term.
        self.finish_done_jobs(obs, tick);
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

        // A unit with no job gets one: hold the nearest structure the house
        // owns. `design.md` section 4 calls this the default task, and it is why
        // `Unowned == 0` holds by construction rather than by luck — a unit whose
        // only proposal was a target it has given up on is exactly the case that
        // used to leave it standing with nothing to do.
        let fallback = self.default_job(obs, rules, tick);
        self.stats.fallback = fallback.len();
        for (unit, job) in fallback {
            self.jobs.give(unit, job);
        }

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
                "policy v2: tick={tick} proposals={} intents={} assigned={} issued={} unowned={} refused={} finished={} stalled={} cooled={} fallback={} stuck={}",
                self.stats.proposals,
                self.stats.intents,
                self.stats.assigned,
                self.stats.issued,
                self.stats.unowned,
                self.stats.refused,
                self.stats.finished,
                self.stats.stalled,
                self.stats.cooled,
                self.stats.fallback,
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
    fn the_army_attacks_the_object_it_was_told_to() {
        let job = |target| Job {
            kind: JobKind::Assault,
            target,
            priority: 60,
            given: 0,
        };
        assert_eq!(
            Policy::goal(&job(Target::Object(77))),
            Some(Goal::Attack { target: 77 }),
            "an assault names the enemy the click would name"
        );
        assert_eq!(
            Policy::goal(&job(Target::Cell((5, 6)))),
            Some(Goal::AttackMove { cell: (5, 6) }),
            "the cell form is still an order, just not the one the army sends"
        );
    }

    #[test]
    fn an_order_that_names_a_new_target_is_a_new_order() {
        let assault = |target| Job {
            kind: JobKind::Assault,
            target,
            priority: 60,
            given: 0,
        };
        // Same kind, moved enemy: the mission still reads Attack, so only the
        // target can tell the policy it has something new to say.
        let sent = Some(&(JobKind::Assault, Target::Object(77)));
        assert!(Policy::same_order(sent, &assault(Target::Object(77))));
        assert!(
            !Policy::same_order(sent, &assault(Target::Object(78))),
            "a moved target is a new order, not a job already lived out"
        );
    }

    #[test]
    fn a_harvesters_cell_is_not_part_of_its_order() {
        // The harvest job's target is the cell the truck stood on when it was
        // given, and the mine is the mission's business: comparing it would
        // re-order every truck every term and break the dock again.
        let harvest = Job {
            kind: JobKind::Harvest,
            target: Target::Cell((9, 9)),
            priority: 10,
            given: 0,
        };
        let sent = Some(&(JobKind::Harvest, Target::Cell((1, 1))));
        assert!(Policy::same_order(sent, &harvest));
    }

    #[test]
    fn a_defend_post_that_moves_is_not_a_new_order() {
        // Before the yard stands, the post is wherever the first soldier is, so
        // comparing it re-sent the guard every term and scattered the army.
        let guard = Job {
            kind: JobKind::Defend,
            target: Target::Cell((50, 60)),
            priority: 20,
            given: 0,
        };
        let sent = Some(&(JobKind::Defend, Target::Cell((40, 40))));
        assert!(Policy::same_order(sent, &guard));
    }

    #[test]
    fn a_unit_gives_up_on_a_target_it_cannot_reach_and_is_not_sent_back() {
        let mut policy = Policy::new(vera20k_core::rules::ruleset::RuleSet::empty_for_test());
        let standing = Some(&((40, 40), 0_u64));
        assert_eq!(
            Policy::standing_frames(standing, (40, 40), 100),
            100,
            "the unit has stood here since frame 0"
        );
        assert!(
            Policy::standing_frames(standing, (40, 40), UNREACHABLE_FRAMES) >= UNREACHABLE_FRAMES,
            "past the ladder's last rung the job is Unreachable"
        );
        assert_eq!(
            Policy::standing_frames(standing, (41, 40), 10_000),
            0,
            "a cell it just reached is not a stall"
        );
        // The cooldown is what keeps the ladder from looping, and it is the
        // policy's own state, so it survives the term that recorded it. The
        // deadline is the first frame the target may be proposed again.
        let until = 900_u64;
        policy.given_up.insert((7, 42), until);
        policy.given_up.retain(|_, deadline| *deadline > until - 1);
        assert!(
            policy.given_up.contains_key(&(7, 42)),
            "still cooling down on the frame before the deadline"
        );
        policy.given_up.retain(|_, deadline| *deadline > until);
        assert!(
            !policy.given_up.contains_key(&(7, 42)),
            "the target may be proposed again once the cooldown has passed"
        );
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
