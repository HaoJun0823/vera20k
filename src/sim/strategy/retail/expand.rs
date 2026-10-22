//! Getting a base started: deploy the yard maker.
//!
//! `UnitClass::Deploy` (`0x007393C0`) is the AI's own route to a base — the
//! house's MCV becomes a Construction Yard, and the Recalc that follows anchors
//! the plan on it. A brain-driven house receives no player click, so the brain
//! issues the same deploy the click would.
//!
//! Expanding beyond the first base is **not** driven: native's expansion is
//! `AI_Manage_Build_Queue`'s and the base-defense placement's, neither of which
//! is ported (see `mod.rs`).

use crate::rules::ruleset::RuleSet;
use crate::sim::control::Goal;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::Observation;
use crate::sim::strategy::retail::RetailBrain;

/// Deploy the house's Construction Yard maker while it has no yard.
pub(super) fn deploy_yard_maker(
    brain: &RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    if brain.anchor(rules, obs).is_some() {
        return;
    }
    let Some(mcv) = obs.own_entities().find(|entity| {
        obs.object_type(rules, entity.type_id)
            .and_then(|ty| crate::sim::mcv_deploy::deploys_into_of(rules, ty))
            .is_some_and(|yard| yard.build_const_eligible)
    }) else {
        return;
    };
    out.order(mcv.entity_id, Goal::Deploy);
}
