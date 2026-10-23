//! The house's spending budget: what the yard may eat, and what it must leave.
//!
//! The native build plan is credit-blind on purpose and stays that way:
//! `AI_RecalcBuildOptions` orders the nodes, `HouseClass::CanBuild` never looks
//! at the wallet, and both choosers spend whatever is in it. A retail house
//! lives with that because its map pays for it — and on the maps a play test
//! uses, ours did not: the yard's node costs several times a unit, the two
//! queues draw on one wallet, and the house sat at 0 credits from tick 12000
//! with the twelve units it started with. A team whose task force misses one
//! tank never fills, and a team that never fills never attacks, so the house
//! lost without ever fighting.
//!
//! This module is the budget, and it is the brain's own policy rather than a
//! port: no native body budgets for a house, and a human proxy has to. Two
//! rules, both read from the house's own state:
//!
//! - **The army keeps its money.** While a forming team still lacks a type the
//!   house could build, the reserve is that type's `Cost=` in `rulesmd.ini`
//!   (`Cost_Of` through the house's own cost multipliers), and the money below
//!   it belongs to the factories: the yard starts nothing and puts a running
//!   build on hold.
//! - **Income and power are never held.** A refinery or a plant is what makes
//!   the next unit affordable at all, so a node of either kind runs even while
//!   the reserve is unmet. Starving it is how a slow economy becomes a stopped
//!   one.
//!
//! Nothing here invents resources: the reserve only decides *when* the yard
//! spends, so the house still competes for its own ore.
//!
//! [`Observation::army_reserve`]: crate::sim::control::observation::Observation::army_reserve

use crate::rules::ruleset::RuleSet;
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::production::ProductionCategory;

/// What the yard may do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Yard {
    /// Start the next open node, or resume the build it holds.
    Run,
    /// Hold the running build; nothing new starts.
    Hold,
}

/// Whether the yard may spend this frame, given what the factories still need.
pub(crate) fn yard(rules: &RuleSet, obs: &Observation<'_>, reserve: i32) -> Yard {
    if reserve <= 0 || obs.credits() >= reserve {
        return Yard::Run;
    }
    if obs
        .building_in_production()
        .and_then(|(type_id, _)| obs.object_type(rules, type_id))
        .is_some_and(|ty| ty.refinery || ty.power > 0)
    {
        return Yard::Run;
    }
    Yard::Hold
}

/// Put the yard's running build on hold.
///
/// Idempotent: a build that is already suspended is left alone, so this can be
/// asked for every frame the reserve is unmet.
pub(super) fn hold(obs: &Observation<'_>, out: &mut Outbox) {
    if obs
        .building_in_production()
        .is_some_and(|(_, suspended)| !suspended)
    {
        out.intent(Intent::SuspendProduction {
            category: ProductionCategory::Building,
        });
    }
}

/// Take the yard's build off hold.
///
/// `Begin_Production` resumes the product the factory already holds, so passing
/// the held type is what resumes it — anything else would queue a second copy.
pub(super) fn resume(obs: &Observation<'_>, out: &mut Outbox) {
    if let Some((type_id, true)) = obs.building_in_production() {
        out.intent(Intent::QueueProduction { type_id });
    }
}
