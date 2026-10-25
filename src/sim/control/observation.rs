//! L2's read-only projection, scoped to one house: what that house's player
//! sees.
//!
//! The decision layer reads the game only through this type, so its whole
//! information set is one documented surface with one owner. It is a borrow, not
//! a snapshot: nothing is copied or retained, and a decision that needs a value
//! across frames holds the value, not the view.
//!
//! Each field reads the state's authoritative owner, never a presentation
//! cache. Specifically:
//!
//! - `credits` reads `HouseState::economy`, the single wallet balance, not the
//!   sidebar's smoothed `displayed_credits` (an app-layer presentation value).
//! - `power_*` reads the retained `PowerState` totals, which is what native
//!   queries, not `effective_totals()` (that one applies the trainer's full
//!   power override under `debug-tools`).
//! - `own_entities` reads `GameEntity` identity and health directly. Its
//!   `entity_id` is the entity's stable id, the same id space
//!   `Command::Move { entity_id }` and the other order variants name, so an
//!   agent can hold one across frames.
//! - `is_ground_walkable` reads `Simulation::path_grid`, the current canonical
//!   navigation projection. It is derived from terrain and blocking objects,
//!   and it says nothing about whether a cell is currently occupied: use it
//!   for reachability, not for occupancy.
//!
//! Everything here is integer-valued and iterates in storage order, so a
//! decision built on it stays deterministic.

use crate::map::entities::EntityCategory;
use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::house_state::HouseDifficulty;
use crate::sim::intern::InternedId;
use crate::sim::mission::MissionId;
use crate::sim::production::ProductionCategory;
use crate::sim::production::{CanBuild, can_build};
use crate::sim::world::Simulation;

/// What one house's agent may look at, and nothing else.
pub struct Observation<'a> {
    sim: &'a Simulation,
    house: InternedId,
}

/// One object the observed house owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnEntity {
    /// The stable id an order command names.
    pub entity_id: u64,
    pub type_id: InternedId,
    pub cell: (u16, u16),
    pub health: i32,
    /// `effective()`: the mission the object is on, or is queued onto, now.
    pub mission: MissionId,
    /// Whether an order may be issued to it now: alive, above zero strength,
    /// out of limbo. The same predicate the order gate applies
    /// (`Simulation::order_actor_admits`), so a caller can skip a doomed
    /// attempt without restating the test.
    pub orderable: bool,
}

/// A finished building of the observed house that is waiting for a placement
/// cell — the product `Command::PlaceReadyBuilding` places. A house can hold
/// one per factory category, so `ready_buildings` yields them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadyBuilding {
    pub category: ProductionCategory,
    pub type_id: InternedId,
    /// The completed object, once one exists: the factory's pending product
    /// names its type before the object is built.
    pub entity_id: Option<u64>,
}

/// An enemy object a policy may aim an order at: the id `Command::Attack`
/// names as its target, and where it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnemyEntity {
    pub entity_id: u64,
    pub type_id: InternedId,
    pub cell: (u16, u16),
}

/// A building that takes infantry and has room for more: the garrison click's
/// target. `room` is the carrier-side count, not a rules number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Billet {
    pub building_id: u64,
    pub cell: (u16, u16),
    pub room: u32,
}

impl<'a> Observation<'a> {
    pub fn new(sim: &'a Simulation, house: InternedId) -> Self {
        Self { sim, house }
    }

    /// The house this view is scoped to; the owner every issued command takes.
    pub fn house(&self) -> InternedId {
        self.house
    }

    /// The world behind the projection.
    ///
    /// Crate-internal, and only for another L2 helper that needs the whole
    /// world rather than a projection of it — today the native placement
    /// search (`sim::ai_base_site`), which takes `&Simulation`. A decision
    /// layer must not use it: it would bypass this surface, and
    /// `architecture_guards.rs` fails the tests if the strategy module does.
    pub(crate) fn simulation(&self) -> &'a Simulation {
        self.sim
    }

    /// The tick commands issued now are stamped with.
    pub fn tick(&self) -> u64 {
        self.sim.session().tick()
    }

    /// The house's wallet: `HouseState::economy`, the authoritative balance.
    pub fn credits(&self) -> i32 {
        self.sim
            .houses()
            .get(&self.house)
            .map_or(0, |house| house.economy().credits())
    }

    /// Power produced by the house's live, powered buildings.
    pub fn power_output(&self) -> i32 {
        self.power().map_or(0, |(output, _)| output)
    }

    /// Power the house's buildings consume.
    pub fn power_drain(&self) -> i32 {
        self.power().map_or(0, |(_, drain)| drain)
    }

    /// Output minus drain: negative means a brownout.
    pub fn power_balance(&self) -> i32 {
        let (output, drain) = self.power().unwrap_or((0, 0));
        output.wrapping_sub(drain)
    }

    fn power(&self) -> Option<(i32, i32)> {
        let state = self.sim.power_states().get(&self.house)?;
        Some((state.total_output(), state.total_drain()))
    }

    /// The house's own objects, in entity-storage order.
    ///
    /// The store also holds objects that are in limbo, riding in a transport
    /// or awaiting deletion, so "mine" does not imply "orderable": the host's
    /// gate applies `order_actor_admits` before a command is queued.
    pub fn own_entities(&self) -> impl Iterator<Item = OwnEntity> + '_ {
        let house = self.house;
        self.sim
            .entities()
            .values()
            .filter(move |entity| entity.owner() == house)
            .map(|entity| OwnEntity {
                entity_id: entity.stable_id(),
                type_id: entity.type_ref(),
                cell: (entity.position.rx(), entity.position.ry()),
                health: entity.health.current(),
                mission: entity.mission.effective(),
                orderable: self.sim.order_actor_admits(entity.stable_id()),
            })
    }

    /// Full strength of a type, from the frozen rules an object was built with.
    pub fn max_health(&self, rules: &RuleSet, type_id: InternedId) -> Option<i32> {
        self.sim.object_type(type_id, rules).map(|ty| ty.strength)
    }

    /// Finished buildings waiting to be placed on the map.
    pub fn ready_buildings(&self) -> impl Iterator<Item = ReadyBuilding> + '_ {
        let house = self.house;
        [ProductionCategory::Building, ProductionCategory::Defense]
            .into_iter()
            .filter_map(move |category| {
                let pending = self
                    .sim
                    .production()
                    .factories()
                    .view(house, category)?
                    .complete_object()?;
                Some(ReadyBuilding {
                    category,
                    type_id: pending.type_id,
                    entity_id: pending.entity_id,
                })
            })
    }

    /// Whether the factory that makes this type still holds it: the object
    /// being built, or a copy queued behind it. `false` once a building's
    /// product waits for placement, which [`Self::ready_buildings`] reports.
    pub fn in_production(&self, rules: &RuleSet, type_id: InternedId) -> bool {
        let Some(ty) = self.sim.object_type(type_id, rules) else {
            return false;
        };
        let category = crate::sim::production::category_for_object(ty);
        let Some(factory) = self.sim.production().factories().view(self.house, category) else {
            return false;
        };
        factory
            .object
            .is_some_and(|object| object.type_id == type_id)
            || factory.queue.iter().any(|entry| entry.type_id == type_id)
    }

    /// How far the factory has got on this type, in production steps — the
    /// progress bar the sidebar draws. `None` when the factory does not hold
    /// this type.
    pub fn production_progress(&self, rules: &RuleSet, type_id: InternedId) -> Option<u16> {
        let ty = self.sim.object_type(type_id, rules)?;
        let category = crate::sim::production::category_for_object(ty);
        let factory = self.sim.production().factories().view(self.house, category)?;
        let object = factory.object?;
        (object.type_id == type_id).then_some(factory.progress)
    }

    /// The type the building factory holds right now, and whether its build is
    /// on hold (`FactoryClass`'s suspension).
    ///
    /// A budget that puts the yard on hold needs the type to resume it:
    /// `HouseClass::Begin_Production` resumes a suspended product and would
    /// queue a second copy of anything else.
    pub fn building_in_production(&self) -> Option<(InternedId, bool)> {
        self.factory_product(ProductionCategory::Building)
    }

    /// The type the house's factory of `category` holds right now, and whether
    /// its build is on hold — `None` when the factory is idle or the house has
    /// none of that category.
    pub fn factory_product(&self, category: ProductionCategory) -> Option<(InternedId, bool)> {
        let factory = self
            .sim
            .production()
            .factories()
            .view(self.house, category)?;
        factory
            .object
            .map(|object| (object.type_id, factory.suspended))
    }

    /// Whether the house's factory of `category` holds anything of its own: a
    /// product being built, or a copy queued behind it.
    pub(crate) fn factory_busy(&self, category: ProductionCategory) -> bool {
        let Some(factory) = self.sim.production().factories().view(self.house, category) else {
            return false;
        };
        factory.object.is_some() || !factory.queue.is_empty()
    }

    /// `find_factory`'s answer for this house: a live factory of the house can
    /// make `ty` now. `CanBuild` alone does not imply one (`Owner=` matching is
    /// the factory's own).
    pub(crate) fn has_factory(&self, rules: &RuleSet, ty: &ObjectType) -> bool {
        crate::sim::production::find_factory(self.sim, rules, self.house, ty, true, true, true)
            .is_some()
    }

    /// Enemy objects an order may name as its target, in storage order.
    ///
    /// Natively the computer AI is omniscient, and its teams pick targets from
    /// the whole map; a policy reusing its decisions inherits that. Fog honesty
    /// is a recorded open question, not a decision this accessor makes.
    ///
    /// Enmity is the House ally graph's complement (`sim::house_diplomacy`,
    /// `HouseClass+0x5788`): an ally is never a target, and neither is a
    /// `MultiplayPassive` house — the neutral and civilian houses that own the
    /// map's tech buildings, garrisonable houses and parked cars, which the
    /// computer AI leaves alone and turns on only when they are made enemies.
    pub(crate) fn enemy_entities(&self) -> impl Iterator<Item = EnemyEntity> + '_ {
        let house = self.house;
        let houses = self.sim.houses();
        let allied = crate::map::houses::ally_test(
            self.sim.house_alliances.as_map(),
            self.sim.interner().resolve(house),
        );
        self.sim.entities().values().filter_map(move |entity| {
            let owner = entity.owner();
            if owner == house || entity.lifecycle.in_limbo() || entity.health.current() <= 0 {
                return None;
            }
            let target = houses.get(&owner)?;
            if target.multiplay_passive() || allied(self.sim.interner().resolve(owner)) {
                return None;
            }
            Some(EnemyEntity {
                entity_id: entity.stable_id(),
                type_id: entity.type_ref(),
                cell: (entity.position.rx(), entity.position.ry()),
            })
        })
    }

    /// Buildings any engineer may take: alive, not this house's, and
    /// `Capturable=`. Neutral tech buildings and civilian structures qualify,
    /// which is exactly the difference between capturing one and shooting it.
    pub(crate) fn capturable_buildings<'r>(
        &'r self,
        rules: &'r RuleSet,
    ) -> impl Iterator<Item = EnemyEntity> + 'r {
        let house = self.house;
        self.sim.entities().values().filter_map(move |entity| {
            let owner = entity.owner();
            if owner == house || entity.lifecycle.in_limbo() || entity.health.current() <= 0 {
                return None;
            }
            let ty = self.sim.object_type(entity.type_ref(), rules)?;
            if !ty.capturable || ty.category != ObjectCategory::Building {
                return None;
            }
            Some(EnemyEntity {
                entity_id: entity.stable_id(),
                type_id: entity.type_ref(),
                cell: (entity.position.rx(), entity.position.ry()),
            })
        })
    }

    /// Buildings a spy may walk into: alive, not this house's, and `Spyable=`.
    ///
    /// `InfantryClass::What_Action_OnObject`'s infiltration arm
    /// (`0x0051EE3B..0x0051EEEF`) turns a spy's Attack into Capture only over a
    /// `Spyable=` building — `Capturable=` is the flag every other infiltrator
    /// asks for — so the engineer's list is the wrong one for a spy, and a spy
    /// sent at a merely-`Capturable=` building walks into nothing.
    ///
    /// Enmity is [`Self::enemy_entities`]' own: an ally's building and a
    /// `MultiplayPassive` house's are both left out, because there is no
    /// treasury behind either to steal from.
    pub(crate) fn spyable_buildings<'r>(
        &'r self,
        rules: &'r RuleSet,
    ) -> impl Iterator<Item = EnemyEntity> + 'r {
        let house = self.house;
        let houses = self.sim.houses();
        let allied = crate::map::houses::ally_test(
            self.sim.house_alliances.as_map(),
            self.sim.interner().resolve(house),
        );
        self.sim.entities().values().filter_map(move |entity| {
            let owner = entity.owner();
            if owner == house || entity.lifecycle.in_limbo() || entity.health.current() <= 0 {
                return None;
            }
            let target = houses.get(&owner)?;
            if target.multiplay_passive() || allied(self.sim.interner().resolve(owner)) {
                return None;
            }
            let ty = self.sim.object_type(entity.type_ref(), rules)?;
            if !ty.spyable || ty.category != ObjectCategory::Building {
                return None;
            }
            Some(EnemyEntity {
                entity_id: entity.stable_id(),
                type_id: entity.type_ref(),
                cell: (entity.position.rx(), entity.position.ry()),
            })
        })
    }

    /// Buildings that take infantry and have room for more, whoever owns them.
    ///
    /// Whether a given infantryman may walk into one is not this accessor's
    /// question: `BuildingClass::CanBeOccupiedBy` owns it, and the policy asks
    /// it per candidate. Any owner is listed because that owner also refuses
    /// the ones a house may not enter.
    pub(crate) fn occupiable_buildings<'r>(
        &'r self,
        rules: &'r RuleSet,
    ) -> impl Iterator<Item = Billet> + 'r {
        self.sim.entities().values().filter_map(move |entity| {
            if entity.category() != EntityCategory::Structure
                || entity.lifecycle.in_limbo()
                || entity.health.current() <= 0
            {
                return None;
            }
            let ty = self.sim.object_type(entity.type_ref(), rules)?;
            if !ty.can_be_occupied {
                return None;
            }
            let cargo = entity.passenger_role.cargo()?;
            let room = cargo.capacity().saturating_sub(cargo.count());
            if room == 0 {
                return None;
            }
            Some(Billet {
                building_id: entity.stable_id(),
                cell: (entity.position.rx(), entity.position.ry()),
                room,
            })
        })
    }

    /// `BridgeRepairHut=` buildings whose bridge is actually down.
    ///
    /// Walking an engineer into one is the only thing that rebuilds a bridge:
    /// the hut has no timer, no `Capturable=` and `Immune=yes`, so it never
    /// appears in [`Self::capturable_buildings`] and no other command reaches
    /// it. The scan of the damaged span is `bridge_hut_can_repair`'s own;
    /// repeating it here would be a second answer to the same question.
    pub(crate) fn bridge_repair_huts<'r>(
        &'r self,
        rules: &'r RuleSet,
    ) -> impl Iterator<Item = Billet> + 'r {
        self.sim.entities().values().filter_map(move |entity| {
            if entity.lifecycle.in_limbo() || entity.health.current() <= 0 {
                return None;
            }
            let ty = self.sim.object_type(entity.type_ref(), rules)?;
            if !ty.bridge_repair_hut {
                return None;
            }
            // `[CABHUT]` is 1x1 (`artmd.ini:4143`), so its cell is the centre
            // the native scan is keyed by.
            let cell = (entity.position.rx(), entity.position.ry());
            if !crate::sim::world::bridge_orchestrator::bridge_hut_can_repair(self.sim, cell) {
                return None;
            }
            Some(Billet {
                building_id: entity.stable_id(),
                cell,
                room: 1,
            })
        })
    }

    /// Whether `infantry_id` may garrison `building_id`:
    /// `BuildingClass::CanBeOccupiedBy`'s answer for this pair.
    pub(crate) fn can_garrison(&self, rules: &RuleSet, infantry_id: u64, building_id: u64) -> bool {
        crate::sim::passenger::building_can_be_occupied_by(self.sim, rules, building_id, infantry_id)
    }

    /// The rules' name of a type, for messages: a policy that has to say what
    /// it is waiting on names the type the way the rules do.
    pub(crate) fn name_of(&self, type_id: InternedId) -> Option<&'a str> {
        self.sim.interner().try_resolve(type_id)
    }

    /// The rules' type for `type_id`: a policy reads the type's own fields
    /// (foundation, `Adjacent=`, `DeploysInto=`, `BuildConst=` membership).
    pub(crate) fn object_type<'r>(
        &self,
        rules: &'r RuleSet,
        type_id: InternedId,
    ) -> Option<&'r ObjectType> {
        self.sim.object_type(type_id, rules)
    }

    /// What the house's own choice has a factory of `category` make next:
    /// `HouseClass::Suggest_New_Object @ 0x004FBD80`, read for a policy driving
    /// a human house. `HouseClass::Update` fills that choice every eight frames
    /// from what the house's forming teams still need (`TeamClass::
    /// Get_Needed_Types`), which is where an engineer, a spy or a transport
    /// comes from — needs no combat ranking states. The native factory reads
    /// the slot itself (`BuildingClass::Factory_AI`), and a human house's
    /// factory never runs that body, so the policy is the reader here.
    ///
    /// Unchecked, as the native reader is: the caller still owns `CanBuild`
    /// and the wallet.
    pub(crate) fn chosen_product<'r>(
        &self,
        rules: &'r RuleSet,
        category: ObjectCategory,
    ) -> Option<(&'r ObjectType, InternedId)> {
        let ty =
            crate::sim::ai_base_building::suggested_product(self.sim, rules, self.house, category)?;
        let type_id = self.type_id(&ty.id)?;
        Some((ty, type_id))
    }

    /// `BaseClass @ 0x0042E780` for a node of the policy's own plan: the
    /// node's building stands at its cell, so `Find_Node` no longer reports it
    /// open. A wall node needs the overlay registry and so reads unsatisfied
    /// here, which is right for a policy that does not drive walls.
    pub(crate) fn base_plan_node_satisfied(
        &self,
        rules: &RuleSet,
        node: &crate::sim::base_plan::BasePlanNode,
    ) -> bool {
        crate::sim::ai_base_building::node_satisfied(self.sim, rules, self.house, node, None)
    }

    /// Whether the unit `entity_id` could deploy into `yard` where it stands:
    /// the placement test `UnitClass::Deploy` applies (`0x00739422..
    /// 0x00739565`), answered by the receiver's own owner without touching
    /// the unit.
    ///
    /// A brain that presses D on a blocked cell gets nothing back — the
    /// receiver plays `CannotDeployHere`, clears the deploy latch and queues
    /// Guard (`0x007394E0..0x0073957A`) — so the decision "drive somewhere
    /// else first" needs this answer before the order, not after it.
    pub fn deploy_site_admits(&self, rules: &RuleSet, entity_id: u64, yard: &ObjectType) -> bool {
        self.sim.deploy_site_admits(entity_id, rules, yard)
    }

    /// Whether `HouseClass::Place_Production` would accept `ty` with its
    /// top-left cell here: the sidebar's build-area adjacency and the
    /// building's own `CanPlaceAt` clear-to-build, the two predicates that
    /// Unlimbo asks again after the click. Read-only; walls need the overlay
    /// registry and so are not admitted here.
    pub(crate) fn placement_admits(
        &self,
        rules: &RuleSet,
        ty: &ObjectType,
        rx: u16,
        ry: u16,
    ) -> bool {
        crate::sim::production::is_within_build_area(
            self.sim,
            rules,
            self.sim.interner().resolve(self.house),
            ty,
            rx,
            ry,
        ) && crate::sim::build_site::can_place_building_at(
            self.sim,
            rules,
            None,
            ty,
            (rx as i16, ry as i16),
            Some(self.house),
            None,
            None,
        )
    }

    /// Whether a team of this house has recruited `entity_id`.
    ///
    /// A team's script owns its members: `team_script_vm`'s pass has no house
    /// gate, so a policy-driven house's teams run like any other's — recruit
    /// the house's units and drive them. The policy does not order over a
    /// member, or two issuers would give the same unit its next step.
    pub(crate) fn team_has_member(&self, entity_id: u64) -> bool {
        self.sim
            .team_script_vm
            .team_for_member(entity_id)
            .is_some_and(|(team_id, _)| {
                self.sim
                    .team_script_vm
                    .team(team_id)
                    .is_some_and(|team| team.owner() == self.house)
            })
    }

    /// The house's country, spelled the way the rules name it. `None` when the
    /// house has none or the id is unknown.
    pub fn country(&self) -> Option<String> {
        let id = self.sim.houses().get(&self.house)?.country()?;
        Some(self.sim.interner().resolve(id).to_owned())
    }

    /// The house's side index, as the rules carry it.
    pub fn side_index(&self) -> Option<i32> {
        self.sim
            .houses()
            .get(&self.house)
            .map(|house| house.side_index())
    }

    pub fn difficulty(&self) -> Option<HouseDifficulty> {
        self.sim
            .houses()
            .get(&self.house)
            .map(|house| house.difficulty())
    }

    pub fn tech_level(&self) -> Option<i32> {
        self.sim
            .houses()
            .get(&self.house)
            .map(|house| house.tech_level())
    }

    /// `[GameOptions] SuperWeapons=`, which the native Recalc reads.
    pub fn super_weapons(&self) -> bool {
        self.sim.session().game_options().super_weapons
    }

    /// The id the rules registered a type under, for a policy holding a name.
    pub fn type_id(&self, name: &str) -> Option<InternedId> {
        self.sim.interner().get(name)
    }

    /// `HouseClass::CanBuild` for this house, over the same inputs production
    /// uses. `count_in_production` decides whether a type already queued counts
    /// against the limit, which is what a policy wants when it is about to ask
    /// for another one.
    ///
    /// This is the production path's own test, read through the projection: a
    /// decision asks the same question the sidebar's build button answers, so
    /// it cannot talk itself into a build the house would refuse, and it needs
    /// no second copy of the rules it checks (tech level, prerequisites,
    /// owner limits, in-production counts).
    pub fn can_build(
        &self,
        rules: &RuleSet,
        object: &ObjectType,
        count_in_production: bool,
    ) -> CanBuild {
        can_build(
            self.sim,
            rules,
            self.house,
            object,
            false,
            count_in_production,
        )
    }

    /// The finished product `category`'s factory still holds: the cameo a
    /// player's sidebar shows as done.
    ///
    /// A factory does not hand its object over on its own. The house has to
    /// take it — the click on the done cameo, `Place_Production` — and until
    /// it does the object is built, paid for and counted, the category starts
    /// nothing else, and the house's money buys nothing new there. A decision
    /// layer that queues builds and never takes them stalls itself.
    pub fn finished_product(&self, category: ProductionCategory) -> Option<InternedId> {
        self.sim
            .production()
            .factories()
            .view(self.house, category)?
            .complete_object()
            .map(|object| object.type_id)
    }

    /// What the house pays for one `object` now: `HouseClass::Cost_Of`, the
    /// price production charges when it starts the build.
    ///
    /// Read from the simulation rather than from the rules' `Cost=`: the price
    /// carries the house's country cost multiplier, which the rules alone do
    /// not. A decision that budgets credits against this cannot misprice a
    /// build for the house it is playing.
    pub fn cost_of(&self, rules: &RuleSet, object: &ObjectType) -> i32 {
        self.sim.cost_of(self.house, object, rules)
    }

    /// Whether the ground at a cell is walkable in the current path projection.
    /// Out of bounds answers false. This is not an occupancy query.
    pub fn is_ground_walkable(&self, rx: u16, ry: u16) -> bool {
        self.sim
            .path_grid()
            .is_some_and(|grid| grid.is_walkable(rx, ry))
    }
}
