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

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::ai_team_creation::TeamCreationPlan;
use crate::sim::ai_unit_choice::UnitChoiceKind;
use crate::sim::house_state::HouseDifficulty;
use crate::sim::intern::InternedId;
use crate::sim::mission::MissionId;
use crate::sim::production::ProductionCategory;
use crate::sim::production::{CanBuild, can_build};
use crate::sim::world::Simulation;
use crate::sim::world::TypeTechLevels;

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
                orderable: crate::sim::world::order_actor_admits_for(entity),
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

    /// Whether the house's team timer has expired, so `HouseClass::Update`'s
    /// team block (`sim::ai_team_creation`) would run for it now. The timer is
    /// the owner's state; the policy reads it and the host restarts it through
    /// the owner when a pass runs.
    pub(crate) fn team_creation_due(&self) -> bool {
        let Some(house) = self.sim.houses().get(&self.house) else {
            return false;
        };
        let frame = self.sim.session().binary_frame() as i32;
        house.team_creation().timer().expired(frame)
    }

    /// The TeamTypes the house's own selector would raise now
    /// (`sim::ai_team_creation::plan_team_creation`), decided with the caller's
    /// draws. The selector is the native one: a policy-driven house never runs
    /// it, because its gate is "no human controls this house".
    /// The TeamTypes the house's own selector would raise now for a house a
    /// brain plays (`sim::ai_team_creation::plan_team_creation_policy`),
    /// decided with the caller's draws. The selector is the native one.
    ///
    /// The gates are the policy-played ones: this house is the player's slot,
    /// so its own `RatioAITriggerTeam=` and AI-trigger activation are the ones
    /// the scenario leaves unset, and reading them would leave the selector
    /// dead and the army unbuilt.
    pub(crate) fn plan_team_creation(
        &self,
        rules: &RuleSet,
        draw: &mut impl FnMut(i32, i32) -> i32,
    ) -> TeamCreationPlan {
        crate::sim::ai_team_creation::plan_team_creation_policy(
            self.sim,
            rules,
            self.house,
            crate::sim::ai_team_creation::TeamGates::policy_played(),
            draw,
        )
    }

    /// The unit the house's teams need of `kind`, chosen the way
    /// `HouseClass::AI_Choose_Unit` chooses it (`sim::ai_unit_choice`), as a
    /// type. `None` when no forming team needs one of that class.
    ///
    /// The choice is the native decision over the house's live teams, drawn
    /// with the caller's RNG; the house's own choice slot is left alone, since
    /// `sim::ai_unit_choice` owns it.
    pub(crate) fn unit_choice(
        &self,
        rules: &RuleSet,
        kind: UnitChoiceKind,
        draw: &mut impl FnMut(i32, i32) -> i32,
    ) -> Option<InternedId> {
        let index = crate::sim::ai_unit_choice::unit_choice_decision(
            self.sim, rules, self.house, kind, draw,
        )?;
        let name = rules.type_array_ids(kind.category()).get(index as usize)?;
        self.type_id(name)
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

    /// The AI's own answer to "what unit does this house need": the harvester
    /// branch of `HouseClass::AI_Choose_Unit` (`ai_unit_choice`), read as a
    /// type. `None` when the house has enough gatherers or the rules name none.
    ///
    /// That branch is gated on no human controlling the house; auto play is a
    /// policy driving it in the player's place, so the gate reads false here.
    pub(crate) fn harvester_choice(&self, rules: &RuleSet) -> Option<InternedId> {
        let gates = crate::sim::ai_unit_choice::ChooserGates::policy_played(rules);
        let index =
            crate::sim::ai_unit_choice::harvester_choice(self.sim, rules, self.house, gates)?;
        let name = rules
            .type_array_ids(ObjectCategory::Vehicle)
            .get(usize::try_from(index).ok()?)?;
        self.type_id(name)
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

    /// The plant `AI_Choose_Building` would put in front of `ty`
    /// (`0x004FE6F6..0x004FE943`, [`crate::sim::ai_base_building::power_plant_to_splice`]):
    /// `ty`'s drain would take the house past its output, so the chooser
    /// splices the side's plant ahead of it. Read-only; the policy builds the
    /// plant first and leaves the node to the next decision.
    pub(crate) fn power_plant_to_splice(
        &self,
        rules: &RuleSet,
        ty: &ObjectType,
    ) -> Option<InternedId> {
        let index =
            crate::sim::ai_base_building::power_plant_to_splice(self.sim, rules, self.house, ty)?;
        let plant = rules.building_type_at(index)?;
        self.type_id(&plant.id)
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

    /// The scenario's per-type tech levels the native Recalc filters against.
    pub(crate) fn type_tech_levels(&self) -> &TypeTechLevels {
        self.sim.type_tech_levels()
    }

    /// The id the rules registered a type under, for a policy holding a name.
    pub fn type_id(&self, name: &str) -> Option<InternedId> {
        self.sim.interner().get(name)
    }

    /// `HouseClass::CanBuild` for this house, over the same inputs production
    /// uses. `count_in_production` decides whether a type already queued counts
    /// against the limit, which is what a policy wants when it is about to ask
    /// for another one.
    pub(crate) fn can_build(
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

    /// Whether the ground at a cell is walkable in the current path projection.
    /// Out of bounds answers false. This is not an occupancy query.
    pub fn is_ground_walkable(&self, rx: u16, ry: u16) -> bool {
        self.sim
            .path_grid()
            .is_some_and(|grid| grid.is_walkable(rx, ry))
    }
}
