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

/// Deploy the house's Construction Yard maker while it has no yard.
pub(super) fn deploy_yard_maker(rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
    // A yard already stands, so there is nothing to deploy. `BuildConst=`
    // eligible is the rules' own mark for it.
    if obs.own_entities().any(|entity| {
        obs.object_type(rules, entity.type_id)
            .is_some_and(|ty| ty.build_const_eligible)
    }) {
        return;
    }
    let Some(mcv) = obs.own_entities().find(|entity| {
        obs.object_type(rules, entity.type_id)
            .and_then(|ty| ty.deploys_into.as_deref())
            .and_then(|name| rules.object(name))
            .is_some_and(|yard| yard.build_const_eligible)
    }) else {
        return;
    };
    out.order(mcv.entity_id, Goal::Deploy);
}
