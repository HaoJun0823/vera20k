//! Getting a base started: deploy the yard maker, and buy another one when the
//! yard is gone.
//!
//! `UnitClass::Deploy` (`0x007393C0`) is the AI's own route to a base — the
//! house's MCV becomes a Construction Yard, and the Recalc that follows anchors
//! the plan on it. A brain-driven house receives no player click, so the brain
//! issues the same deploy the click would.
//!
//! A yard is also a structure like any other, so it can be destroyed, and every
//! structure in `[AI] BuildRefinery=`/`BuildWeapons=`/`BuildBarracks=` hangs off
//! it through `Prerequisite=` (`HouseClass::CanBuild` @ `0x004F7870`: a human
//! house needs the entry on the map). Losing it is not a lost base yet — the
//! factories still produce and the army still fights — but nothing new can be
//! started, so a player buys another MCV out of the war factory, which is the
//! only type that carries `DeploysInto=` back to the yard.
//!
//! Expanding beyond the first base is **not** driven: native's expansion is
//! `AI_Manage_Build_Queue`'s and the base-defense placement's, neither of which
//! is ported (see `mod.rs`).

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::prerequisite::Prerequisite;
use crate::rules::ruleset::RuleSet;
use crate::sim::control::Goal;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::Observation;
use crate::sim::control::placement::{PLACEMENT_ATTEMPTS, Placement};
use crate::sim::intern::InternedId;
use crate::sim::production::CanBuild;
use crate::sim::strategy::retail::RetailBrain;

/// Whether `ty` is a Construction Yard maker: its `DeploysInto=` is the
/// building the rules' `[AI] BuildConst=` list holds. `UnitType`'s
/// `DeploysInto=` is the reader's own field, so a side whose maker is called
/// something else answers here without a name of its own.
pub(super) fn makes_a_yard(rules: &RuleSet, ty: &ObjectType) -> bool {
    ty.deploys_into
        .as_deref()
        .and_then(|name| rules.object(name))
        .is_some_and(|yard| yard.build_const_eligible)
}

/// Whether a yard stands: `BuildConst=` eligible is the rules' own mark for
/// it.
pub(super) fn yard_stands(rules: &RuleSet, obs: &Observation<'_>) -> bool {
    obs.own_entities().any(|entity| {
        obs.object_type(rules, entity.type_id)
            .is_some_and(|ty| ty.build_const_eligible)
    })
}

/// Whether the house holds a mobile yard maker: an MCV waiting to deploy, or
/// one still driving out of its factory.
pub(super) fn owns_yard_maker(rules: &RuleSet, obs: &Observation<'_>) -> bool {
    obs.own_entities().any(|entity| {
        obs.object_type(rules, entity.type_id)
            .is_some_and(|ty| makes_a_yard(rules, ty))
    })
}

/// Deploy the house's Construction Yard maker while it has no yard.
///
/// A maker that just left the war factory stands where the yard cannot: the
/// deploy shifts its foundation one cell north-west
/// (`deploy_origin_from_unit_cell`), and the factory and its neighbours are
/// in the way. `UnitClass::Deploy` refuses that silently and drops the unit
/// back onto Guard, so the brain does what the player does — drives it to a
/// cell the yard fits on first and presses D there. Native's own answer to a
/// blocked cell is the same search, in `UnitClass::TryToDeploy`
/// (`0x00738D30`), which only a computer house's Hunt mission runs.
pub(super) fn deploy_yard_maker(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    // One line every ten seconds, because this branch is silent by design: a
    // report of "the maker never deploys" has to be able to say whether the
    // brain ever saw one, and whether a yard was standing over it.
    if obs.tick() % 600 == 0 {
        let maker = obs.own_entities().find(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| makes_a_yard(rules, ty))
        });
        log::info!(
            "auto play: yard={} maker={:?} credits={}",
            yard_stands(rules, obs),
            maker.map(|entity| (entity.entity_id, entity.cell)),
            obs.credits()
        );
    }
    // A yard already stands, so there is nothing to deploy.
    if yard_stands(rules, obs) {
        return;
    }
    let Some((mcv, yard)) = obs.own_entities().find_map(|entity| {
        let ty = obs.object_type(rules, entity.type_id)?;
        let yard = rules.object(ty.deploys_into.as_deref()?)?;
        makes_a_yard(rules, ty).then_some((entity, yard))
    }) else {
        return;
    };
    if obs.deploy_site_admits(rules, mcv.entity_id, yard) {
        out.order(mcv.entity_id, Goal::Deploy);
        return;
    }
    let Some(yard_id) = obs.type_id(&yard.id) else {
        return;
    };
    // The sweep's cells are the sidebar's own build area, so the maker ends up
    // on base ground the way a player would put it. `site_attempts` is what
    // moves past a cell that cannot be used, the same memory a waiting
    // structure uses.
    let attempt = brain.site_attempts.get(&yard_id).copied().unwrap_or(0);
    match Placement::new(obs, rules).cell_for(yard_id, attempt) {
        Some(cell) if cell != mcv.cell => out.order(mcv.entity_id, Goal::Move { cell }),
        _ => {
            let next = (attempt + 1) % PLACEMENT_ATTEMPTS;
            brain.site_attempts.insert(yard_id, next);
        }
    }
}

/// The maker the house may buy now, or `None` when it may buy none.
///
/// The type comes from the same `CanBuild` answer the sidebar reads, so a
/// side without the war factory that carries it — or one at its build limit —
/// simply has no way back, exactly as a player would.
pub(super) fn yard_maker<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
) -> Option<(&'r ObjectType, InternedId)> {
    super::build::class_types(rules, obs, ObjectCategory::Vehicle)
        .find(|(ty, _)| makes_a_yard(rules, ty))
}

/// A building the yard maker's own `Prerequisite=` list asks for and the
/// house does not hold yet, when the house may build it.
///
/// Retail's `[AMCV]` wants `GAWEAP,GADEPT`: the war factory is the army's
/// anyway, but the service depot is a structure nothing else asks for, and a
/// house whose depot was never built has no way back from a lost yard — the
/// maker it needs is `CanBuild` No for good. One is bought while the yard
/// still stands, so the recovery has something to stand on.
///
/// The list is the type's own: a side whose depot is called something else is
/// answered by `CanBuild`, which is where `RequiredHouses=` and the depot's
/// own prerequisites are read.
pub(super) fn yard_maker_support<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
) -> Option<(&'r ObjectType, InternedId)> {
    super::build::all_class_types(rules, obs, ObjectCategory::Vehicle)
        .filter(|(ty, _)| makes_a_yard(rules, ty))
        .find_map(|(maker, _)| missing_prerequisite(rules, obs, maker))
}

/// The first `Prerequisite=` building of `maker` the house lacks and may
/// build now.
fn missing_prerequisite<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    maker: &ObjectType,
) -> Option<(&'r ObjectType, InternedId)> {
    maker.prerequisite.iter().find_map(|entry| {
        let Prerequisite::Building(index) = *entry else {
            return None;
        };
        let want = rules.type_array_at(ObjectCategory::Building, index)?;
        let owned = obs.own_entities().any(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| ty.id == want.id)
        });
        if owned {
            return None;
        }
        let type_id = obs.type_id(&want.id)?;
        (obs.can_build(rules, want, true) != CanBuild::No).then_some((want, type_id))
    })
}
