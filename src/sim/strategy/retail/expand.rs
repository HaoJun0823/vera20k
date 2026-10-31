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
//! Beyond that first yard, a house that is earning more than it can spend
//! builds another: it buys a maker out of the war factory, walks it away from
//! the base it has, and deploys it there. That is a second `BuildConst=`
//! structure, and the placement sweep, the build list and the walls all read
//! it as one — there is no second ledger for it.

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

use super::advance::cell_distance;
use super::census::Census;
use super::plan::Budget;

/// The free wallet a second base is bought out of: the maker's own price and
/// the first structure that follows it out of the new yard.
pub(super) const EXPAND_MIN_CREDITS: i32 = 4_000;

/// How far from the base it already has the second one is built, in cells: near
/// enough to walk to and to defend, far enough that the two bases do not become
/// one crowded block — which is the shape the placement sweep is built to
/// avoid.
const EXPAND_MIN_CELLS: i32 = 10;

/// How far out that search looks, in cells.
const EXPAND_SCAN_CELLS: i32 = 24;

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
/// it, and the ledger already counted them.
pub(super) fn yard_stands(census: &Census) -> bool {
    census.yards > 0
}

/// Whether the house holds a mobile yard maker: an MCV waiting to deploy, or
/// one still driving out of its factory.
pub(super) fn owns_yard_maker(census: &Census) -> bool {
    census.yard_makers > 0
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
    // One line every forty seconds (600 frames at 15 a second), because this
    // branch is silent by design: a report of "the maker never deploys" has to
    // be able to say whether the brain ever saw one, and whether a yard was
    // standing over it.
    if obs.tick() % 600 == 0 {
        let maker = brain
            .census
            .units
            .iter()
            .find(|unit| unit.traits.yard_maker);
        log::info!(
            "auto play: yard={} maker={:?} credits={}",
            yard_stands(&brain.census),
            maker.map(|unit| (unit.entity_id, unit.cell)),
            obs.credits()
        );
    }
    let Some((entity_id, cell, yard)) = brain
        .census
        .units
        .iter()
        .filter(|unit| unit.traits.yard_maker)
        // A maker the player has grouped is theirs to deploy, and one a team
        // script has recruited is the script's.
        .filter(|unit| !unit.hand)
        .find_map(|unit| {
            let ty = obs.object_type(rules, unit.type_id)?;
            let yard = rules.object(ty.deploys_into.as_deref()?)?;
            makes_a_yard(rules, ty).then_some((unit.entity_id, unit.cell, yard))
        })
    else {
        return;
    };
    // A yard already stands, so this maker is not the way back from a loss: it
    // is the second base. It walks out and deploys where the house has ground
    // of its own.
    if yard_stands(&brain.census) {
        deploy_second_yard(brain, rules, obs, out, entity_id, cell, yard);
        return;
    }
    if obs.deploy_site_admits(rules, entity_id, yard) {
        super::orders::issue(brain, out, rules, obs, entity_id, Goal::Deploy);
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
        Some(target) if target != cell => {
            super::orders::issue(brain, out, rules, obs, entity_id, Goal::Move { cell: target });
        }
        _ => {
            let next = (attempt + 1) % PLACEMENT_ATTEMPTS;
            brain.site_attempts.insert(yard_id, next);
        }
    }
}

/// Whether the house should be buying a yard maker it does not need yet: one
/// for a second base.
///
/// Native's expansion is `AI_Manage_Build_Queue`'s, which is not ported; the
/// gates here are what any house has to satisfy before a maker is worth a war
/// factory cycle — a standing yard (otherwise it is the way back from a loss,
/// which [`super::composition`] already answers), the trucks to fill the bays
/// it has, and a wallet that can still build once the maker is paid for. The
/// type itself comes from the same `CanBuild` answer the sidebar reads
/// ([`Self::yard_maker`]), so a house that cannot build one simply does not.
pub(super) fn wants_a_maker(brain: &RetailBrain, budget: &Budget) -> bool {
    if !yard_stands(&brain.census) || owns_yard_maker(&brain.census) {
        return false;
    }
    // One expansion at a time. A second yard is a second set of `BaseNormal=`
    // anchors for the placement sweep and a second perimeter for the walls, and
    // two of those half-started are worse than one finished.
    if brain.census.yards > 1 {
        return false;
    }
    // Income first: a bay with no truck at it is the dearest structure in the
    // base, and a maker is dearer than a bay.
    if !brain.census.harvesters_served() {
        return false;
    }
    budget.free >= EXPAND_MIN_CREDITS
}

/// Drive a maker the house bought for a second base out, and deploy it there.
///
/// A second yard beside the first is not an expansion, it is the same base with
/// more of it — the sweep's own job is to keep one base from becoming a block,
/// so the second one is started far enough away to be its own.
fn deploy_second_yard(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
    entity_id: u64,
    cell: (u16, u16),
    yard: &ObjectType,
) {
    let Some(home) = brain.census.base_cell else {
        return;
    };
    // Far enough out, and on ground the yard fits: then it is the player's own
    // D press. Whether the deploy is admitted is the simulation's answer once
    // the maker is standing there, not something this pass can know from a
    // distance — the deploy shifts the foundation off the unit's own cell.
    if cell_distance(cell, home) >= EXPAND_MIN_CELLS
        && obs.deploy_site_admits(rules, entity_id, yard)
    {
        super::orders::issue(brain, out, rules, obs, entity_id, Goal::Deploy);
        return;
    }
    if let Some(target) = expansion_cell(brain, rules, obs, home, yard) {
        super::orders::issue(brain, out, rules, obs, entity_id, Goal::Move { cell: target });
    }
}

/// The nearest cell a second yard may be deployed on: away from the house's own
/// base, on ground a unit can stand on, and a cell a yard of this type fits on.
///
/// The rings are walked outwards from the base and the first admissible cell in
/// the fixed ring order wins, so the walk is the shortest one that gets the
/// maker clear of what the house has already built. Ore is not a factor yet:
/// the rules' overlay is not reachable from [`Observation`], and guessing at a
/// field from the truck's path is not evidence.
fn expansion_cell(
    brain: &RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    home: (u16, u16),
    yard: &ObjectType,
) -> Option<(u16, u16)> {
    let (width, height) = obs.map_extent();
    // No map, no expansion: the headless harness and the fixtures run without
    // one, and a walk into cell (0, 0) is worse than staying put.
    if width == 0 || height == 0 {
        return None;
    }
    let home_cells = &brain.census.home_cells;
    for ring in EXPAND_MIN_CELLS..=EXPAND_SCAN_CELLS {
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                if dx.abs().max(dy.abs()) != ring {
                    continue;
                }
                let (Some(rx), Some(ry)) = (
                    u16::try_from(i32::from(home.0) + dx).ok(),
                    u16::try_from(i32::from(home.1) + dy).ok(),
                ) else {
                    continue;
                };
                if rx >= width || ry >= height || !obs.is_ground_walkable(rx, ry) {
                    continue;
                }
                if !obs.placement_admits(rules, yard, rx, ry) {
                    continue;
                }
                // Not inside a base the house already has: that is where the
                // first yard is, and the point of the walk is to leave it.
                if home_cells
                    .iter()
                    .any(|cell| cell_distance(*cell, (rx, ry)) < EXPAND_MIN_CELLS)
                {
                    continue;
                }
                return Some((rx, ry));
            }
        }
    }
    None
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
    census: &Census,
) -> Option<(&'r ObjectType, InternedId)> {
    super::build::all_class_types(rules, obs, ObjectCategory::Vehicle)
        .filter(|(ty, _)| makes_a_yard(rules, ty))
        .find_map(|(maker, _)| missing_prerequisite(rules, obs, census, maker))
}

/// The first `Prerequisite=` building of `maker` the house lacks and may
/// build now.
fn missing_prerequisite<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    maker: &ObjectType,
) -> Option<(&'r ObjectType, InternedId)> {
    maker.prerequisite.iter().find_map(|entry| {
        let Prerequisite::Building(index) = *entry else {
            return None;
        };
        let want = rules.type_array_at(ObjectCategory::Building, index)?;
        let type_id = obs.type_id(&want.id)?;
        if census.type_count(type_id) > 0 {
            return None;
        }
        (obs.can_build(rules, want, true) != CanBuild::No).then_some((want, type_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::arena_fixture;
    use crate::sim::world::Simulation;
    use crate::sim::strategy::retail::RetailBrain;

    /// The flat arena with the computer `Russians` and the human `Americans`.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet, InternedId)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        let owner = sim.interner.intern("Americans");
        Some((sim, rules, owner))
    }

    /// One maker, bought while the yard stands, and only then.
    ///
    /// The gate used to be "no yard and no maker", which is the way back from a
    /// loss and nothing else: a house that had never lost its base never bought
    /// a second one, and the whole map's ore past its own field was somebody
    /// else's.
    #[test]
    fn a_yard_maker_is_bought_for_a_second_base_once_the_income_holds() {
        let Some((mut sim, rules, owner)) = arena(0xA17) else {
            return;
        };
        let _ = sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules);
        let _ = sim.spawn_object("GAREFN", "Americans", 18, 16, 255, &rules);
        let _ = sim.spawn_object("CMIN", "Americans", 20, 16, 255, &rules);
        let _ = sim.spawn_object("CMIN", "Americans", 20, 18, 255, &rules);
        sim.houses
            .get_mut(&owner)
            .expect("the house")
            .economy_mut()
            .set_credits_for_test(10_000);

        let mut brain = RetailBrain::new(0xA17);
        {
            let obs = Observation::new(&sim, owner);
            brain.rebuild_census(&rules, &obs);
            let budget = super::super::plan::budget(&rules, &obs, &brain.census, None);
            assert!(
                wants_a_maker(&brain, &budget),
                "a house with a yard, its trucks and a full wallet wants a second one"
            );
        }

        // A second yard is enough: the sweep and the walls have one more base
        // to keep tidy than they can while the first is still filling in.
        let _ = sim.spawn_object("GACNST", "Americans", 24, 16, 255, &rules);
        {
            let obs = Observation::new(&sim, owner);
            brain.census = Default::default();
            brain.rebuild_census(&rules, &obs);
            let budget = super::super::plan::budget(&rules, &obs, &brain.census, None);
            assert!(!wants_a_maker(&brain, &budget), "one expansion at a time");
        }
    }
}
