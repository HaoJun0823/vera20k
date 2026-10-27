//! The managed table: which objects run a routine, and its phase.
//!
//! **This state is not simulation state.** It never enters a snapshot, a
//! checksum or a replay: a replay reproduces it from the same command stream
//! and the same rules, because every routine is a pure function of
//! `(object, threats, phase)` and the phase is advanced only by stepping. What
//! the table decides reaches the world exclusively as commands. The header of
//! [`crate::sim::behavior::managed`] states the same invariant from the other
//! side; a test replays an identical sequence into two tables and compares them
//! step for step.
//!
//! Lifecycle is L2's: an entry is dropped when its object dies, leaves the map,
//! is captured, boards a transport or is mind-controlled, and when a team script
//! takes it over — a team's members already have an issuer, and two issuers
//! would overwrite each other's orders (`TeamClass::Recruit` clears the
//! member's target).

use std::collections::BTreeMap;

use crate::rules::ruleset::RuleSet;
use crate::sim::behavior::managed::{FallbackReason, MicroKind, MicroParams, MicroStep, Phase};
use crate::sim::behavior::view::{ObjectView, ThreatView};
use crate::sim::intern::InternedId;
use crate::sim::world::Simulation;

/// One attached object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Managed {
    pub kind: MicroKind,
    pub params: MicroParams,
    pub phase: Phase,
    /// Consecutive orders the funnel refused as not-ready. Past
    /// [`Self::MAX_REFUSALS`] the entry falls back to the brain.
    pub refusals: u32,
}

impl Managed {
    /// How many not-ready refusals in a row before the routine gives the object
    /// back. Three frames is shorter than any mission transition the funnel can
    /// see and long enough not to give up on a single awkward frame.
    pub const MAX_REFUSALS: u32 = 3;
}

/// Every managed object of one house.
#[derive(Debug, Default)]
pub struct MicroTable {
    entries: BTreeMap<u64, Managed>,
}

impl MicroTable {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn is_managed(&self, entity: u64) -> bool {
        self.entries.contains_key(&entity)
    }

    pub fn get(&self, entity: u64) -> Option<&Managed> {
        self.entries.get(&entity)
    }

    /// Attach `entity` to a routine. `false` when it is already managed, so a
    /// caller cannot silently reset a phase.
    pub fn attach(
        &mut self,
        entity: u64,
        kind: MicroKind,
        params: MicroParams,
        post: Option<(u16, u16)>,
    ) -> bool {
        if self.entries.contains_key(&entity) {
            return false;
        }
        self.entries.insert(
            entity,
            Managed {
                kind,
                params,
                phase: Phase {
                    post,
                    ..Phase::default()
                },
                refusals: 0,
            },
        );
        true
    }

    pub fn detach(&mut self, entity: u64) -> Option<Managed> {
        self.entries.remove(&entity)
    }

    /// Drop the entries whose object may no longer be driven by a routine.
    ///
    /// Called once per frame, before stepping. Returns the ids it dropped, so
    /// the caller can log them.
    pub fn retain_live(
        &mut self,
        sim: &Simulation,
        owner: InternedId,
    ) -> Vec<(u64, FallbackReason)> {
        let mut dropped = Vec::new();
        self.entries.retain(|&entity, _| {
            let Some(entry) = sim.entities().get(entity) else {
                dropped.push((entity, FallbackReason::Gone));
                return false;
            };
            let reason =
                if entry.owner() != owner {
                    Some(FallbackReason::Gone)
                } else if !entry.is_object_alive()
                    || entry.lifecycle.in_limbo()
                    || entry.health.current() <= 0
                {
                    Some(FallbackReason::Gone)
                } else if entry.passenger_role.inside_transport().is_some()
                    || entry.mind_control.controller().is_some()
                {
                    Some(FallbackReason::Gone)
                } else if sim.team_script_vm().team_for_member(entity).is_some_and(
                    |(team_id, _)| {
                        sim.team_script_vm()
                            .team(team_id)
                            .is_some_and(|team| team.owner() == owner)
                    },
                ) {
                    // A team's script owns its members.
                    Some(FallbackReason::NothingToDo)
                } else {
                    None
                };
            match reason {
                Some(reason) => {
                    dropped.push((entity, reason));
                    false
                }
                None => true,
            }
        });
        dropped
    }

    /// The ids to step this frame: `limit` of them, starting at `from` and
    /// wrapping, in ascending id order so the sequence is reproducible.
    pub fn rotation(&self, from: u64, limit: usize) -> Vec<u64> {
        if self.entries.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut ids: Vec<u64> = self
            .entries
            .range(from..)
            .map(|(&id, _)| id)
            .chain(self.entries.range(..from).map(|(&id, _)| id))
            .take(limit)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Step one attached object. `None` when it is still waiting out its timer
    /// and was not asked this frame.
    pub fn step_one(
        &mut self,
        entity: u64,
        sim: &Simulation,
        rules: &RuleSet,
        threats: &ThreatView<'_>,
    ) -> Option<MicroStep> {
        let Some(entry) = self.entries.get_mut(&entity) else {
            return None;
        };
        if entry.phase.wait > 0 {
            entry.phase.wait -= 1;
            return None;
        }
        let Some(me) = ObjectView::new(sim, rules, entity) else {
            return Some(MicroStep::Fallback(FallbackReason::Gone));
        };
        let kind = entry.kind;
        let params = entry.params;
        let step = kind.routine().step(&me, threats, &mut entry.phase, &params);
        if let MicroStep::Wait { frames } = step {
            entry.phase.wait = frames;
        }
        Some(step)
    }

    /// Record the outcome of issuing a routine's goal.
    ///
    /// `queued` keeps the entry; a refusal counts up and, past
    /// [`Managed::MAX_REFUSALS`], the entry is dropped so the brain takes the
    /// object back — the silent-stall defence.
    pub fn record_outcome(&mut self, entity: u64, queued: bool) -> Option<FallbackReason> {
        let entry = self.entries.get_mut(&entity)?;
        if queued {
            entry.refusals = 0;
            return None;
        }
        entry.refusals += 1;
        if entry.refusals < Managed::MAX_REFUSALS {
            return None;
        }
        self.entries.remove(&entity);
        Some(FallbackReason::RepeatedNotReady)
    }
}
