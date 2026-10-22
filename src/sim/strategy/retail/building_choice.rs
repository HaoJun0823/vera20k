//! What the yard builds next, and where the finished product goes.
//!
//! `AI_Choose_Building` (`0x004FE3E0`) takes the first node
//! `BaseClass::Find_Node` (`0x0042EB50`, `wanted = -1`) reports open and
//! answers at once while a choice waits (`0x004FE3E9..0x004FE416`). Two
//! wrinkles are native and kept:
//!
//! - a node whose drain would take the house past its power output gets the
//!   side's plant spliced in front of it (`0x004FE6F6..0x004FE943`). Natively
//!   that plant becomes a node at the same index and the rest shift up; here it
//!   is one build that fills no node, and the node stays open for the decision
//!   that follows;
//! - a node whose building stands is satisfied (`0x0042E780`), which is what
//!   makes the plan advance after a placement rather than asking again.
//!
//! Native parks its choice on an unbuildable node and lets
//! `AI_Manage_Build_Queue` (`0x004FDD10`) recover by selling, splicing or
//! replanning. That recovery is not ported, so a node that stays unbuildable
//! past [`GIVE_UP_FRAMES`] is stepped over instead: a plan that stalls forever
//! leaves a play test with a base that never moves, which is worse than one
//! built slightly out of the retail order. That is a recorded residual.

use crate::rules::ruleset::RuleSet;
use crate::sim::base_plan::pack_base_plan_cell;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::control::placement::{PLACEMENT_ATTEMPTS, Placement};
use crate::sim::intern::InternedId;
use crate::sim::production::CanBuild;
use crate::sim::strategy::retail::{GIVE_UP_FRAMES, RetailBrain};

/// One plan node being built or placed: the yard serializes, so a house has at
/// most one of these at a time.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Build {
    /// The plan index this product fills.
    pub(super) node: usize,
    pub(super) type_id: InternedId,
    /// This product is a plant `AI_Choose_Building` spliced in front of the
    /// node (`0x004FE893..0x004FE943`), not the node's own type: it fills no
    /// node, and the node stays open for the decision that follows.
    pub(super) spliced: bool,
    /// The cell the last placement asked for. `Some` once one was asked, which
    /// is how the product's disappearance reads as "it stands" rather than
    /// "the player cancelled it".
    pub(super) placement: Option<(u16, u16)>,
    /// How many admitted cells were already asked for. A product still waiting
    /// next frame means the last cell was refused, so the sweep moves on.
    pub(super) attempt: u32,
    /// Frames the finished product has waited for a cell no admitted one
    /// refused. Nought until one is refused.
    pub(super) waited: u32,
}

/// Step the product the yard has finished: place it, and once it has left the
/// factory record the node it filled.
pub(super) fn place(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<Intent> {
    let mut build = brain.build?;
    if obs
        .ready_buildings()
        .any(|ready| ready.type_id == build.type_id)
    {
        // A product still waiting here was asked for on an earlier frame and
        // did not stand: that cell was refused, so the sweep moves on instead
        // of asking for it forever.
        let refused = build.placement.is_some();
        let attempt = if refused {
            (build.attempt + 1) % PLACEMENT_ATTEMPTS
        } else {
            build.attempt
        };
        let placement = Placement::new(obs, rules);
        let Some(cell) = placement
            .cell_for(build.type_id, attempt)
            .or_else(|| placement.cell_for(build.type_id, 0))
        else {
            // No cell this map admits. Retrying forever would leave the yard
            // holding the product and the plan stopped, so past the give-up
            // window the product is abandoned and the node with it.
            build.waited += 1;
            if build.waited <= GIVE_UP_FRAMES {
                brain.build = Some(build);
                if build.waited == 1 {
                    log::info!(
                        "retail-ai: no placement cell is admitted for {}; waiting",
                        obs.name_of(build.type_id).unwrap_or("a building")
                    );
                }
                return None;
            }
            log::info!(
                "retail-ai: giving up on {} after {GIVE_UP_FRAMES} frames with no cell",
                obs.name_of(build.type_id).unwrap_or("a building")
            );
            brain.skipped.push(build.node);
            brain.build = None;
            return Some(Intent::CancelProduction {
                type_id: build.type_id,
                all: true,
            });
        };
        build.placement = Some(cell);
        build.attempt = attempt;
        brain.build = Some(build);
        if refused && attempt == 1 {
            log::info!(
                "retail-ai: placement of {} was refused; sweeping the next cell",
                obs.name_of(build.type_id).unwrap_or("a building")
            );
        }
        return Some(Intent::PlaceBuilding {
            type_id: build.type_id,
            cell,
        });
    }

    // Nothing waits for placement. A request that was made and whose product is
    // gone placed it; a refused placement leaves it waiting, so this is the
    // confirmation. Without a request the product was cancelled.
    let cell = build.placement?;
    if build.spliced {
        // The plant raised the house's output; the node it was spliced in front
        // of stays open and the next decision asks for it again.
        brain.build = None;
        brain.stall_logged = None;
        return None;
    }
    let index = obs.object_type(rules, build.type_id)?.base_plan_type_index;
    let packed = pack_base_plan_cell(i32::from(cell.0), i32::from(cell.1));
    brain.plan.assign_site(build.node, packed);
    brain.plan.fill_successful_building(index, packed, false);
    brain.build = None;
    brain.stall_logged = None;
    None
}

/// Queue the first open node's type, once the house may build it and nothing of
/// ours is already in flight.
pub(super) fn queue(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<Intent> {
    // Without a yard no Building factory exists; the deploy step is what
    // changes that, so nothing else is worth asking for yet.
    if brain.anchor(rules, obs).is_none() {
        return None;
    }
    if let Some(build) = brain.build {
        // Still being built, or already waiting for its placement.
        if obs.in_production(rules, build.type_id)
            || obs
                .ready_buildings()
                .any(|ready| ready.type_id == build.type_id)
        {
            return None;
        }
        // Cancelled by the player, or refused by the factory: pick again.
        brain.build = None;
    }
    let (node, type_id, spliced) = choose(brain, rules, obs)?;
    brain.build = Some(Build {
        node,
        type_id,
        spliced,
        placement: None,
        attempt: 0,
        waited: 0,
    });
    Some(Intent::QueueProduction { type_id })
}

/// The first open node of the plan, the type the yard builds for it now, and
/// whether that type is a plant spliced in front of the node.
fn choose(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<(usize, InternedId, bool)> {
    loop {
        let mut chosen = None;
        let mut blocked = None;
        for (index, node) in brain.plan.nodes().iter().enumerate() {
            // A node whose building stands is satisfied (`0x0042E780`); a
            // negative one is a control node (walls, base defenses, perimeter)
            // this brain does not drive.
            if node.type_or_control < 0
                || brain.skipped.contains(&index)
                || obs.base_plan_node_satisfied(rules, node)
            {
                continue;
            }
            // A node naming a type the loaded rules never registered has no
            // producer at all, so it cannot be worked: step over it.
            let Some((object, type_id)) = brain.node_type(rules, obs, node.type_or_control) else {
                continue;
            };
            // `0x004FE6F6..0x004FE943`: a node whose drain would take the house
            // past its output gets the side's plant in front of it.
            if let Some(plant) = obs.power_plant_to_splice(rules, object) {
                let buildable = obs.object_type(rules, plant).is_some_and(|plant_type| {
                    obs.can_build(rules, plant_type, true) == CanBuild::Yes
                });
                if buildable {
                    chosen = Some((index, plant, true));
                } else {
                    blocked = Some((index, plant));
                }
                break;
            }
            if obs.can_build(rules, object, true) == CanBuild::Yes {
                chosen = Some((index, type_id, false));
            } else {
                blocked = Some((index, type_id));
            }
            break;
        }
        let Some((index, type_id)) = blocked else {
            brain.blocked = None;
            brain.stall_logged = None;
            return chosen;
        };
        let waited = match brain.blocked {
            Some((blocked, waited)) if blocked == index => waited + 1,
            _ => 1,
        };
        brain.blocked = Some((index, waited));
        if waited <= GIVE_UP_FRAMES {
            if brain.stall_logged != Some(index) {
                brain.stall_logged = Some(index);
                log::info!(
                    "retail-ai: plan node {index} waits on {}: the house may not build it yet",
                    obs.name_of(type_id).unwrap_or("a building")
                );
            }
            return None;
        }
        log::info!(
            "retail-ai: giving up on plan node {index} ({}) after {GIVE_UP_FRAMES} frames: \
             the house never may build it",
            obs.name_of(type_id).unwrap_or("a building")
        );
        brain.skipped.push(index);
        brain.blocked = None;
        brain.stall_logged = None;
    }
}
