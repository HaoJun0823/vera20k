//! Layer 1: what the house should build, place, repair or deploy this frame.
//!
//! This is the first slice that gives the `Production` and `Ability` subsystems real
//! work. It reads the engine's own authoritative views rather than keeping a second
//! copy of anything: `queue_view_for_owner` says what is already queued,
//! `ready_buildings_for_owner` says what is waiting for placement and
//! `placement_preview_for_owner_with_overlays` judges a candidate cell. Type ids come
//! from `RuleSet::all_objects()` - no unit or building name is hard-coded.
//!
//! One strategic order at most per frame, and only on a paced interval. A paced
//! decision is what keeps a production queue from being flooded (the wishlist's P05).
//! A building that is already complete and waiting for placement is the exception - it
//! goes out immediately, because until it is placed the player sees nothing happen.

use std::collections::BTreeSet;

use vera20k_core::rules::object_type::{ObjectCategory, ObjectType};
use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::production::{
    can_repair_building, credits_for_owner, placement_preview_for_owner_with_overlays,
    power_balance_for_owner, queue_view_for_owner, ready_buildings_for_owner,
};
use vera20k_core::sim::world::Simulation;

use crate::availability::availability_of;
use crate::control::{ControlState, Subsystem};
use crate::world::Snapshot;

/// Frames between strategic decisions. Fifteen frames per second, so this is about
/// three seconds - long enough for a queued item to make progress, short enough that
/// a fresh match does not sit idle for half a minute.
pub const SUPPORT_INTERVAL: u64 = 45;

/// Ability commands are slower still: repair and deploy are toggles, so re-sending
/// them every production tick would switch them straight back off.
pub const ABILITY_INTERVAL: u64 = SUPPORT_INTERVAL * 4;

/// Below this a "building" is a wall or a decoration. Queuing one produces a
/// command the factory machinery refuses, and even when it succeeds it is not a
/// base the player can see growing.
pub const MIN_STRUCTURE_COST: i32 = 100;

/// One non-combat order, already shaped as a command.
pub struct StrategicOrder {
    pub command: Command,
    pub subsystem: Subsystem,
    /// An owned object used only as the admission anchor: `Arbiter` reads the
    /// envelope's owner house from an entity, and production events act on that
    /// house. It is not a task subject.
    pub anchor: u64,
    /// Why this order was chosen, for the match report.
    pub reason: &'static str,
}

/// An owned object to anchor an admission on. Buildings are preferred because a
/// construction yard or factory is the natural author of a production event.
fn anchor_of(snapshot: &Snapshot) -> Option<u64> {
    let own: Vec<_> = snapshot
        .own()
        .filter(|object| object.in_playfield)
        .collect();
    own.iter()
        .find(|object| object.is_building())
        .or_else(|| own.first())
        .map(|object| object.id)
}

/// Find a legal cell for a completed building, near the house's existing buildings.
///
/// Rings outward from the average building position and lets the engine's own
/// placement preview decide. Returning `None` is honest: a placement we cannot
/// prove legal is worse than no placement at all.
fn placement_cell(
    sim: &Simulation,
    rules: &RuleSet,
    owner: &str,
    type_id: InternedId,
    snapshot: &Snapshot,
) -> Option<(u16, u16)> {
    let type_str = sim.interner().resolve(type_id);
    let buildings: Vec<_> = snapshot
        .own()
        .filter(|object| object.is_building() && object.in_playfield)
        .collect();
    if buildings.is_empty() {
        return None;
    }
    let sum_x: i32 = buildings.iter().map(|b| i32::from(b.cell.0)).sum();
    let sum_y: i32 = buildings.iter().map(|b| i32::from(b.cell.1)).sum();
    let cx = sum_x / buildings.len() as i32;
    let cy = sum_y / buildings.len() as i32;
    let max_x = i32::from(sim.session().map_width().saturating_sub(1));
    let max_y = i32::from(sim.session().map_height().saturating_sub(1));
    const DIRECTIONS: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    for radius in 2..=12_i32 {
        for &(dx, dy) in &DIRECTIONS {
            let rx = (cx + dx * radius).clamp(0, max_x) as u16;
            let ry = (cy + dy * radius).clamp(0, max_y) as u16;
            let preview = placement_preview_for_owner_with_overlays(
                sim, rules, owner, type_str, rx, ry, None,
            );
            if preview.is_some_and(|p| p.valid && p.in_build_area && p.foundation_visible) {
                return Some((rx, ry));
            }
        }
    }
    None
}

/// A buildable type from the rules, with its interned id ready for a command.
fn candidates<'a>(
    sim: &Simulation,
    rules: &'a RuleSet,
    credits: i32,
    queued: &BTreeSet<InternedId>,
    owned_types: &BTreeSet<&str>,
    category: ObjectCategory,
) -> Vec<(&'a ObjectType, InternedId)> {
    rules
        .all_objects()
        .filter(|obj| obj.category == category && obj.cost > 0 && obj.cost <= credits)
        .filter(|obj| !owned_types.contains(obj.id.as_str()))
        .filter_map(|obj| {
            let id = sim.interner().get(&obj.id)?;
            (!queued.contains(&id)).then_some((obj, id))
        })
        .collect()
}

/// Decide one strategic order for this frame, or `None` when nothing is due.
pub fn plan(
    sim: &Simulation,
    snapshot: &Snapshot,
    control: &ControlState,
    rules: &RuleSet,
) -> Option<StrategicOrder> {
    if !control.is_global_enabled() {
        return None;
    }
    let anchor = anchor_of(snapshot)?;
    let owner = sim.interner().resolve(control.house()).to_string();
    let tick = sim.session().tick();

    // A completed building waiting for placement is the most visible thing we can
    // do, and it must not wait for the pacing interval.
    if control.is_subsystem_enabled(Subsystem::Production)
        && let Some(ready) = ready_buildings_for_owner(sim, rules, &owner).first()
        && let Some(cell) = placement_cell(sim, rules, &owner, ready.type_id, snapshot)
    {
        return Some(StrategicOrder {
            command: Command::PlaceReadyBuilding {
                type_id: ready.type_id,
                rx: cell.0,
                ry: cell.1,
            },
            subsystem: Subsystem::Production,
            anchor,
            reason: "place-ready-building",
        });
    }

    // A house with no construction yard cannot build anything: `find_factory`
    // refuses every QueueProduction. Deploy the MCV first - this is the step that
    // makes "build a base" real rather than a command the engine silently drops.
    if control.is_subsystem_enabled(Subsystem::Production) && tick.is_multiple_of(30) {
        let has_yard = snapshot.own().any(|o| {
            o.is_building()
                && rules
                    .object(&o.type_name)
                    .is_some_and(|ty| ty.construction_yard)
        });
        if !has_yard
            && let Some(mcv) = snapshot.own().filter(|o| o.in_playfield).find(|o| {
                rules
                    .object(&o.type_name)
                    .is_some_and(|ty| ty.deploys_into.is_some())
            })
        {
            return Some(StrategicOrder {
                command: Command::DeployMcv { entity_id: mcv.id },
                subsystem: Subsystem::Ability,
                anchor: mcv.id,
                reason: "deploy-mcv",
            });
        }
    }

    if !tick.is_multiple_of(SUPPORT_INTERVAL) {
        return None;
    }

    // Abilities are the rarer decision (once every ABILITY_INTERVAL frames) and go
    // first on those frames: repair and deploy are toggles, so a repeat too soon
    // would undo the previous order. They cannot starve production because the
    // other production ticks still belong to the queue.
    if control.is_subsystem_enabled(Subsystem::Ability) && tick.is_multiple_of(ABILITY_INTERVAL) {
        for object in snapshot.own().filter(|o| o.is_building() && o.in_playfield) {
            if can_repair_building(sim, rules, object.id) {
                return Some(StrategicOrder {
                    command: Command::ToggleRepair {
                        entity_id: object.id,
                    },
                    subsystem: Subsystem::Ability,
                    anchor: object.id,
                    reason: "repair-building",
                });
            }
        }
        for object in snapshot
            .own()
            .filter(|o| !o.is_building() && o.in_playfield)
        {
            let can_deploy = rules
                .object(&object.type_name)
                .is_some_and(|ty| ty.deploy_fire);
            let undeployed = availability_of(sim, object.id).is_some_and(|a| !a.deployed);
            if can_deploy && undeployed {
                return Some(StrategicOrder {
                    command: Command::ToggleInfantryDeploy {
                        entity_id: object.id,
                    },
                    subsystem: Subsystem::Ability,
                    anchor: object.id,
                    reason: "deploy-infantry",
                });
            }
        }
    }

    let credits = credits_for_owner(sim, &owner);
    let (power_out, power_drain) = power_balance_for_owner(sim, rules, &owner);
    let low_power = power_out <= power_drain;
    let queued: BTreeSet<InternedId> = queue_view_for_owner(sim, rules, &owner)
        .into_iter()
        .map(|item| item.type_id)
        .collect();
    let owned_types: BTreeSet<&str> = snapshot
        .own()
        .map(|object| object.type_name.as_str())
        .collect();

    // Production comes first: it is what the player actually sees growing.
    if control.is_subsystem_enabled(Subsystem::Production) && queued.len() < 2 {
        // A structure the house does not have yet. When short of power a power plant
        // wins; otherwise a factory; then the cheapest remaining option.
        let structures: Vec<_> = candidates(
            sim,
            rules,
            credits,
            &queued,
            &owned_types,
            ObjectCategory::Building,
        )
        .into_iter()
        .filter(|(obj, _)| obj.cost >= MIN_STRUCTURE_COST)
        .collect();
        let building = structures.iter().min_by_key(|(obj, _)| {
            // A factory (barracks / war factory / refinery / yard) changes what the
            // house can build next, so it comes before a power plant; then cheapest.
            let priority = if obj.has_factory() {
                0
            } else if low_power && obj.power > 0 {
                1
            } else {
                2
            };
            (priority, obj.cost)
        });
        if let Some((_, type_id)) = building {
            return Some(StrategicOrder {
                command: Command::QueueProduction { type_id: *type_id },
                subsystem: Subsystem::Production,
                anchor,
                reason: "build-missing-structure",
            });
        }

        // A fighting unit, preferring a type the house does not own yet so the army
        // is not one single kind (P16). Cheapest first among equally new types.
        let mut units = Vec::new();
        for category in [
            ObjectCategory::Infantry,
            ObjectCategory::Vehicle,
            ObjectCategory::Aircraft,
        ] {
            units.extend(candidates(
                sim,
                rules,
                credits,
                &queued,
                &owned_types,
                category,
            ));
        }
        if let Some((_, type_id)) = units.iter().min_by_key(|(obj, _)| obj.cost) {
            return Some(StrategicOrder {
                command: Command::QueueProduction { type_id: *type_id },
                subsystem: Subsystem::Production,
                anchor,
                reason: "produce-unit",
            });
        }
    }

    None
}
