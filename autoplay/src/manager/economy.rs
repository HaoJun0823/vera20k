//! Economy: income, harvesters, refineries, the build order.
//!
//! Three decisions, all read off the profile rather than a type name: put the
//! maker down where it can stand, keep every harvester working, and raise the
//! next building the house needs. The need is a role — a refinery, a barracks,
//! a war factory, more power — and the building that satisfies it is the
//! cheapest buildable profile carrying that role.

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

/// How many harvesters a refinery is given.
///
/// One truck on a refinery is the ceiling on the house's income, and the
/// ceiling is what the wallet hits: measured, five of the six baseline cells
/// sat at twelve credits with the wallet frozen for the last 3600 frames,
/// which is also why no second refinery ever landed.
const HARVESTERS_PER_REFINERY: usize = 2;

/// What the wallet keeps in hand before a second truck on a refinery is
/// bought.
const EXTRA_HARVESTER_MARGIN: i32 = 1_500;

#[derive(Debug, Default)]
pub struct Economy;

impl Manager for Economy {
    fn propose(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        self.maker(ctx, out);
        self.harvesters(ctx, out);
        self.place(ctx, out);
        self.harvesters_for_refineries(ctx, out);
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
            if ctx.obs.is_deployed(unit.entity_id) || ty.deploys_into.is_none() {
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
            // The search answers a different cell per attempt; one attempt that
            // finds nothing is a base that stays blocked, because a ready
            // building holds its factory until it is placed.
            let mut cell = None;
            for attempt in 0..8 {
                if let Some(found) = placement.cell_for(ready.type_id, attempt) {
                    cell = Some(found);
                    break;
                }
            }
            let Some(cell) = cell else {
                if ctx.tick % 600 == 0 {
                    log::info!("economy: no site for a ready building at {}", ctx.tick);
                }
                continue;
            };
            out.intents.push(Intent::PlaceBuilding {
                type_id: ready.type_id,
                cell,
            });
        }
    }

    /// The next building, in the order a house opens: a refinery, a barracks, a
    /// war factory, then power whenever the base is short of it.
    fn build(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        if let Some((type_id, ready)) = ctx.obs.building_in_production() {
            if ctx.tick % 1200 == 0 {
                log::info!(
                    "economy: building in production at {}: {:?} ready={ready} credits={}",
                    ctx.tick,
                    ctx.obs
                        .object_type(ctx.rules, type_id)
                        .map_or("?", |ty| ty.id.as_str()),
                    ctx.obs.credits()
                );
            }
            return;
        }
        let mut refinery: Option<&UnitProfile> = None;
        let mut infantry_factory: Option<&UnitProfile> = None;
        let mut vehicle_factory: Option<&UnitProfile> = None;
        let mut power: Option<&UnitProfile> = None;
        let mut defense: Option<&UnitProfile> = None;
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
            } else if profile.roles.contains(Roles::TRAINS_INFANTRY) {
                infantry_factory = Self::cheaper(infantry_factory, profile);
            } else if profile.roles.contains(Roles::BUILDS_VEHICLES) {
                vehicle_factory = Self::cheaper(vehicle_factory, profile);
            } else if profile.roles.contains(Roles::ARMED) && profile.cost <= 1200 {
                // A structure with a gun: the cheapest one the house can raise.
                defense = Self::cheaper(defense, profile);
            } else if profile.power > 0 {
                power = Self::cheaper(power, profile);
            }
        }

        let count = |role: Roles| {
            ctx.obs
                .own_entities()
                .filter(|unit| {
                    ctx.obs
                        .object_type(ctx.rules, unit.type_id)
                        .and_then(|ty| ctx.profiles.get(&ty.id))
                        .is_some_and(|profile| profile.roles.contains(role))
                })
                .count()
        };
        let has = |role: Roles| count(role) > 0;
        // A defences count is structures with a gun. ARMED alone counts every
        // soldier, so the base never bought one.
        let defenses = ctx
            .obs
            .own_entities()
            .filter(|unit| {
                ctx.obs
                    .object_type(ctx.rules, unit.type_id)
                    .and_then(|ty| ctx.profiles.get(&ty.id))
                    .is_some_and(|profile| {
                        profile.roles.contains(Roles::ARMED)
                            && profile.roles.contains(Roles::STRUCTURE)
                    })
            })
            .count();

        // Each need falls back to a plant: power is what the next building
        // usually waits on, and a plant is always cheaper than a stalled base.
        let want = if !has(Roles::REFINERY) {
            refinery.or(power)
        } else if !has(Roles::TRAINS_INFANTRY) {
            infantry_factory.or(power)
        } else if !has(Roles::BUILDS_VEHICLES) {
            vehicle_factory.or(power)
        } else if count(Roles::REFINERY) < 3
            && refinery.is_some_and(|want| Self::affordable(ctx, want))
        {
            refinery
        } else if defenses < 4 && defense.is_some_and(|want| Self::affordable(ctx, want)) {
            defense
        } else if ctx.obs.power_balance() < POWER_MARGIN {
            power
        } else {
            None
        };
        let Some(want) = want else {
            if ctx.tick % 1200 == 0 {
                log::info!(
                    "economy: nothing to build at {} credits={} refineries={} defenses={} in_production={:?}",
                    ctx.tick,
                    ctx.obs.credits(),
                    count(Roles::REFINERY),
                    defenses,
                    ctx.obs.building_in_production()
                );
            }
            return;
        };
        let Some(type_id) = ctx.obs.type_id(&want.id) else {
            return;
        };
        if ctx.obs.in_production(ctx.rules, type_id) {
            return;
        }
        // Starting a build the wallet cannot finish strands the factory: the
        // house pays as it builds, and a stall is a base that never grows.
        if !Self::affordable(ctx, want) {
            return;
        }
        out.intents.push(Intent::QueueProduction { type_id });
    }

    /// One harvester per refinery, replaced as they die. A refinery with no
    /// truck is the quietest way for a house to stop earning.
    fn harvesters_for_refineries(&self, ctx: &Ctx<'_>, out: &mut Demands) {
        let mut harvesters = 0;
        let mut refineries = 0;
        for unit in ctx.obs.own_entities() {
            if let Some(profile) = ctx
                .obs
                .object_type(ctx.rules, unit.type_id)
                .and_then(|ty| ctx.profiles.get(&ty.id))
            {
                harvesters += usize::from(profile.roles.contains(Roles::HARVESTER));
                refineries += usize::from(profile.roles.contains(Roles::REFINERY));
            }
        }
        let wanted = refineries * HARVESTERS_PER_REFINERY;
        if refineries == 0 || harvesters >= wanted {
            return;
        }
        let mut want: Option<&UnitProfile> = None;
        for ty in ctx.rules.all_objects() {
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if !profile.roles.contains(Roles::HARVESTER) {
                continue;
            }
            if ctx.obs.can_build(ctx.rules, ty, false) != CanBuild::Yes {
                continue;
            }
            want = Self::cheaper(want, profile);
        }
        let Some(want) = want else {
            return;
        };
        let Some(type_id) = ctx.obs.type_id(&want.id) else {
            return;
        };
        if ctx.obs.in_production(ctx.rules, type_id) {
            return;
        }
        // The first truck on a refinery is income and is bought as soon as the
        // wallet allows. The second is bought from the surplus: measured, the
        // two-truck rule raised the house's strength in four cells of six and
        // lowered it in two, and the two were the cells where the extra truck
        // took the money the army was about to spend.
        let margin = if harvesters >= refineries {
            EXTRA_HARVESTER_MARGIN
        } else {
            200
        };
        if ctx.obs.credits() < want.cost + margin {
            return;
        }
        out.intents.push(Intent::QueueProduction { type_id });
    }

    /// Whether the wallet can pay for this and still have change: the margin
    /// covers the next harvester or a repair.
    fn affordable(ctx: &Ctx<'_>, profile: &UnitProfile) -> bool {
        ctx.obs.credits() >= profile.cost + 200
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
