//! The house's team block, played by the brain.
//!
//! `HouseClass::Update`'s team block runs for a house no human controls; the
//! house a brain drives is normally the local player's, so its own pass never
//! runs and it would own no teams — which leaves `AI_Choose_Unit`'s combat
//! branches with nothing to answer and the army unbuilt.
//!
//! So when the house's team timer has expired, this module asks the house's own
//! selector (`sim::ai_team_creation::plan_team_creation`) what it would raise and
//! returns it as a state intent. The control layer applies it through that
//! module: the same teams, the same eviction of the oldest base-defense team,
//! the same `Autocreate=` marks, the same timer restart — with this brain's draws
//! instead of the Scenario RNG's.
//!
//! The timer is the owner's state, not the brain's: the brain reads it and the
//! owner restarts it when the pass runs, so a brain replaced mid-match cannot
//! drift the schedule.

use crate::rules::ruleset::RuleSet;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::strategy::retail::RetailBrain;

/// The teams the house's selector would raise now, if the timer is due.
pub(super) fn raise(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<Intent> {
    if !obs.team_creation_due() {
        return None;
    }
    let plan = obs.plan_team_creation(rules, &mut |low, high| brain.draw(low, high));
    if plan.picked.is_empty() {
        // No team was raised, so the combat branches of `AI_Choose_Unit` have
        // nothing to answer and the army stops being built. Say so: a headless
        // run has nobody watching the base.
        log::info!("retail-ai: the team selector raised nothing; the army has no demand");
    } else {
        log::info!(
            "retail-ai: raising {} team(s): {:?}",
            plan.picked.len(),
            plan.picked
                .iter()
                .map(|id| obs.name_of(*id).unwrap_or("?"))
                .collect::<Vec<_>>()
        );
    }
    // The pass ran, so its timer restarts even when it raised nothing:
    // `apply_team_creation` owns that write, and leaving the timer expired
    // would ask the selector again on the next frame with a fresh roll — more
    // teams than the house's `TeamDelays=` schedule allows.
    Some(Intent::RaiseTeams(plan))
}
