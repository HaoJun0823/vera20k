//! What the house's factories build: a harvester for income, then the combat
//! units its forming teams still need.
//!
//! Both answers are `HouseClass::AI_Choose_Unit`'s (`0x004FEA60`), asked through
//! [`Observation`] with this brain's own draws — the chooser itself stays the
//! native port (`sim::ai_unit_choice`), and the brain adds only the scheduling:
//! one request at a time, on a factory that has nothing in hand, for something
//! the house may build and can pay for.
//!
//! The harvester branch goes first: a refinery with no harvester earns nothing,
//! so a base that only builds buildings runs out of credits part-way through its
//! plan and stops. That branch is gated on no human controlling the house, which
//! is exactly the gate a brain-driven house leaves false, so it is asked with
//! `ChooserGates::policy_played` rather than the house's own byte.

use crate::rules::ruleset::RuleSet;
use crate::sim::ai_unit_choice::UnitChoiceKind;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::production::{CanBuild, category_for_object};
use crate::sim::strategy::retail::RetailBrain;

/// The harvester the house lacks, on its war factory.
pub(super) fn harvester(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<Intent> {
    if brain.anchor(rules, obs).is_none() {
        return None;
    }
    let type_id = obs.harvester_choice(rules)?;
    let object = obs.object_type(rules, type_id)?;
    if obs.can_build(rules, object, true) != CanBuild::Yes {
        return None;
    }
    if obs.in_production(rules, type_id) {
        return None;
    }
    Some(Intent::QueueProduction { type_id })
}

/// A combat unit for the house's army, on a factory that has nothing in hand.
///
/// The type is the native chooser's own answer: its Unit, Infantry and Aircraft
/// branches read what the house's forming teams still need
/// (`TeamClass::Get_Needed_Types`), so a unit is requested because a team is
/// short of it, not because it is next in a list.
pub(super) fn combat_unit(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<Intent> {
    if brain.anchor(rules, obs).is_none() {
        return None;
    }
    for kind in [
        UnitChoiceKind::Unit,
        UnitChoiceKind::Infantry,
        UnitChoiceKind::Aircraft,
    ] {
        let Some(type_id) = obs.unit_choice(rules, kind, &mut |low, high| brain.draw(low, high))
        else {
            continue;
        };
        let Some(object) = obs.object_type(rules, type_id) else {
            continue;
        };
        // One request at a time on the factory that would make it. The
        // chooser's own candidate filter already keeps what the house may build
        // and can pay for; this is the brain's scheduling, not a second
        // eligibility test.
        if obs.factory_busy(category_for_object(object)) {
            continue;
        }
        if obs.can_build(rules, object, true) != CanBuild::Yes {
            continue;
        }
        if i64::from(object.cost) > i64::from(obs.credits()) {
            continue;
        }
        if !obs.has_factory(rules, object) {
            continue;
        }
        if obs.in_production(rules, type_id) {
            continue;
        }
        return Some(Intent::QueueProduction { type_id });
    }
    None
}
