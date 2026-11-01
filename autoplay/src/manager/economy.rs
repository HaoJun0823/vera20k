//! Economy: income, harvesters, refineries, the build order.
//!
//! Three decisions, all read off the profile rather than a type name: put the
//! maker down where it can stand, keep every harvester working, and raise the
//! next building the house needs — a plant while the house is short of power, a
//! refinery while it has none. The building it buys is the cheapest profile
//! that supplies the need.

use vera20k_core::rules::object_type::ObjectCategory;
use vera20k_core::sim::control::intent::Intent;
use vera20k_core::sim::control::placement::Placement;
use vera20k_core::sim::production::CanBuild;

use crate::job::{Proposal, Target};
use crate::manager::{Ctx, Demands, Manager};
use crate::profile::{Roles, UnitProfile};
use crate::registry::JobKind;

/// Power headroom under which the house buys another plant, in the same units
/// as power_balance.
const POWER_MARGIN: i32 = 20;

#[derive(Debug, Default)]
pub struct Economy;

impl Manager for Economy {
    fn propose(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        self.maker(ctx, out);
        self.harvesters(ctx, out);
        self.place(ctx, out);
        self.build(ctx, out);
    }
}

impl Economy {
    /// A maker that stands on ground that takes it is deployed before anything
    /// else is decided: a house with a yard has an economy, and a house with a
    /// maker on the ground has neither.
    fn maker(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        for unit in ctx.obs.own_entities() {
            let Some(ty) = ctx.obs.object_type(ctx.rules, unit.type_id) else {
                continue;
            };
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if !profile.roles.contains(Roles::DEPLOYER) || !unit.orderable {
                continue;
            }
            if ctx.obs.is_deployed(unit.entity_id) {
                continue;
            }
            // Whether the yard fits is the order gate's question, not a second
            // copy of it here: the deploy is refused at the door and the job
            // comes back to us, which is the one admission path every order
            // takes.
            if ty.deploys_into.is_none() {
                continue;
            }
            out.jobs.push(Proposal {
                kind: JobKind::Deploy,
                target: Target::None,
                priority: 95,
                actor: Some(unit.entity_id),
            });
        }
    }

    /// Every harvester that is not already harvesting gets one. The harvest
    /// mission finds its own ore, so the job names the unit's own cell.
    fn harvesters(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        for unit in ctx.obs.own_entities() {
            let Some(ty) = ctx.obs.object_type(ctx.rules, unit.type_id) else {
                continue;
            };
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if !profile.roles.contains(Roles::HARVESTER) || !unit.orderable {
                continue;
            }
            if ctx.obs.inside_transport(unit.entity_id).is_some() {
                continue;
            }
            out.jobs.push(Proposal {
                kind: JobKind::Harvest,
                target: Target::Cell(unit.cell),
                priority: 10,
                actor: Some(unit.entity_id),
            });
        }
    }

    /// A finished building takes the first cell the native site search admits.
    fn place(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        for ready in ctx.obs.ready_buildings() {
            let placement = Placement::new(ctx.obs, ctx.rules);
            let Some(cell) = placement.cell_for(ready.type_id, 0) else {
                continue;
            };
            out.intents.push(Intent::PlaceBuilding {
                type_id: ready.type_id,
                cell,
            });
        }
    }

    /// The next building: power while the house is short of it, a refinery
    /// while it has none.
    fn build(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        if ctx.obs.building_in_production().is_some() {
            return;
        }
        let mut refinery: Option<&UnitProfile> = None;
        let mut power: Option<&UnitProfile> = None;
        for ty in ctx.rules.all_objects() {
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if profile.category != ObjectCategory::Building {
                continue;
            }
            // The build button's own answer: tech level, prerequisites, owner
            // limits. A policy that queues a type the house would refuse has
            // built nothing and blocked its own factory.
            if ctx.obs.can_build(ctx.rules, ty, false) != CanBuild::Yes {
                continue;
            }
            if profile.roles.contains(Roles::REFINERY) {
                refinery = Self::cheaper(refinery, profile);
            } else if profile.power > 0 {
                power = Self::cheaper(power, profile);
            }
        }
        let has_refinery = ctx.obs.own_entities().any(|unit| {
            ctx.obs
                .object_type(ctx.rules, unit.type_id)
                .and_then(|ty| ctx.profiles.get(&ty.id))
                .is_some_and(|profile| profile.roles.contains(Roles::REFINERY))
        });
        // With no refinery the tree is still unopened, so the plant comes
        // first: a refinery whose prerequisites are unmet is not buildable, and
        // the only building this house can raise is the one that opens it.
        let want = if !has_refinery {
            power.or(refinery)
        } else if ctx.obs.power_balance() < POWER_MARGIN {
            power
        } else {
            None
        };
        let Some(want) = want else {
            return;
        };
        let Some(type_id) = ctx.obs.type_id(&want.id) else {
            return;
        };
        if ctx.obs.in_production(ctx.rules, type_id) {
            return;
        }
        out.intents.push(Intent::QueueProduction { type_id });
    }

    fn cheaper<'a>(
        current: Option<&'a UnitProfile>,
        candidate: &'a UnitProfile,
    ) -> Option<&'a UnitProfile> {
        match current {
            Some(best) if (best.cost, &best.id) <= (candidate.cost, &candidate.id) => Some(best),
            _ => Some(candidate),
        }
    }
}
