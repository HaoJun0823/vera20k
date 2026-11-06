//! One reading of the world, taken once and then inert.
//!
//! The plan's principle C is that production, combat, expansion, capture and defence
//! must share one world state, and that no two managers may hold contradicting pictures
//! of who owns what or what threatens whom. The review turned that into a rule: never
//! keep a second copy of something the engine already owns.
//!
//! This module is how that rule is kept. A [`Snapshot`] is built by reading the
//! simulation and is then a plain value - no reference back into the engine, no
//! interior mutability, nothing that can drift. Decisions made from a snapshot are
//! decisions about a world that existed at a known frame, which is also what makes a
//! decision explainable afterwards: "at frame 400 the enemy held three buildings" is a
//! claim that can be checked, while "the enemy is weak" cannot.
//!
//! What it does not do: filter by fog, infer intent, or predict. A snapshot says what is,
//! including things the player could not see - see [`Snapshot::is_visible`] for the
//! distinction, which exists so a caller cannot confuse the two by accident.

use std::collections::BTreeMap;

use vera20k_core::map::entities::EntityCategory;
use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

/// One object, as it stood when the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    /// Stable identity, valid for the life of the object.
    pub id: u64,
    /// Who holds it now. Changes during a match by capture or mind control, which is why
    /// it is read per snapshot rather than remembered per object.
    pub owner: InternedId,
    /// The rule type, upper-cased, so a report reads `MTNK` rather than a handle.
    pub type_name: String,
    /// Infantry, vehicle, aircraft, building, or whatever else the engine says.
    pub category: EntityCategory,
    /// Cell position.
    pub cell: (u16, u16),
    /// Current hit points.
    pub health: i32,
    /// Full hit points for its type, which is what makes `health` meaningful.
    pub strength: i32,
    /// Whether it stands inside the playfield, which a projectile does not.
    pub in_playfield: bool,
    /// Whether the local player can see it right now.
    ///
    /// The engine knows this and a planner must not ignore it: the plan's own query
    /// interface distinguishes "true state" from "visible state", and an automated player
    /// that acts on what it could not see is cheating rather than playing.
    pub visible: bool,
}

impl Object {
    /// Health as a share of full strength, or `None` for a type with no strength.
    pub fn health_fraction(&self) -> Option<f64> {
        (self.strength > 0).then(|| f64::from(self.health) / f64::from(self.strength))
    }

    /// Whether this is a structure, which is what a house has to lose to be defeated.
    pub fn is_building(&self) -> bool {
        // The engine calls it `Structure`: a building is the thing a house loses
        // to be defeated, and `EntityCategory` names it by its map role.
        self.category == EntityCategory::Structure
    }
}

/// The world at one frame, as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The frame this was taken at. Every claim in the snapshot is about this frame.
    pub frame: u64,
    /// The house the automated player speaks for.
    pub house: InternedId,
    /// Every object, keyed by id so two snapshots can be compared without ordering noise.
    pub objects: BTreeMap<u64, Object>,
}

impl Snapshot {
    /// Read the world.
    ///
    /// `house` is the player whose view this is: it decides ownership queries and
    /// visibility. Reading is a straight pass over the entity store with no filtering -
    /// deciding what to ignore is a planner's business, and a snapshot that quietly
    /// dropped things would make "there are no enemy buildings" indistinguishable from
    /// "I did not look".
    pub fn take(sim: &Simulation, rules: &RuleSet, house: InternedId) -> Self {
        // One viewer plane for the whole pass, not one lookup per object. The fog is
        // keyed by house, and a house with no plane has revealed nothing - which is a
        // real state (a house added after the map loaded) and not an error.
        let viewer = sim.fog().by_owner().get(&house);
        let mut objects = BTreeMap::new();
        for entity in sim.entities().values() {
            let id = entity.stable_id();
            let type_name = sim
                .interner()
                .resolve(entity.type_ref())
                .to_ascii_uppercase();
            // `strength` is what makes `health` mean something, and only the rule set
            // knows it. A simulation does not own the rules it runs under, so the caller
            // supplies them - the same rules it built the simulation with.
            let strength = sim
                .object_type(entity.type_ref(), rules)
                .map_or(0, |ty| ty.strength);
            let cell = (entity.position.rx(), entity.position.ry());
            let visible = viewer.is_some_and(|plane| plane.is_revealed(cell.0, cell.1));
            objects.insert(
                id,
                Object {
                    id,
                    owner: entity.owner(),
                    type_name,
                    category: entity.category(),
                    cell,
                    health: entity.health.current(),
                    strength,
                    in_playfield: entity.in_playfield(),
                    visible,
                },
            );
        }
        Self {
            frame: sim.session().tick(),
            house,
            objects,
        }
    }

    /// Every object this house owns.
    pub fn own(&self) -> impl Iterator<Item = &Object> {
        let house = self.house;
        self.objects
            .values()
            .filter(move |object| object.owner == house)
    }

    /// Every object somebody else owns.
    pub fn enemy(&self) -> impl Iterator<Item = &Object> {
        let house = self.house;
        self.objects
            .values()
            .filter(move |object| object.owner != house)
    }

    /// Own objects the house can currently see.
    ///
    /// Own things are visible by definition in this engine - the fog is about the enemy -
    /// but the method exists so a reader never has to know that.
    pub fn own_visible(&self) -> impl Iterator<Item = &Object> {
        self.own().filter(|object| object.visible)
    }

    /// Enemy objects the house can currently see.
    ///
    /// The only honest input for target selection. `enemy()` is everything the engine
    /// knows, which includes what the player has never looked at; a planner that aims at
    /// that is not playing the game.
    pub fn enemy_visible(&self) -> impl Iterator<Item = &Object> {
        self.enemy().filter(|object| object.visible)
    }

    /// Own buildings, which keep the house alive.
    pub fn own_buildings(&self) -> impl Iterator<Item = &Object> {
        self.own().filter(|object| object.is_building())
    }

    /// Enemy buildings this house can see.
    pub fn enemy_buildings_visible(&self) -> impl Iterator<Item = &Object> {
        self.enemy_visible().filter(|object| object.is_building())
    }

    /// Whether the house can see `cell`, as far as the objects in this snapshot say.
    ///
    /// A snapshot is a value and must not reach back into the engine, so it can only
    /// answer for cells something stands on: `None` means nothing was there to tell me,
    /// which is not the same as "hidden". A caller that needs fog for an empty cell has to
    /// ask the simulation, and the name says so rather than pretending.
    pub fn visibility_at(&self, cell: (u16, u16)) -> Option<bool> {
        self.objects
            .values()
            .find(|object| object.cell == cell)
            .map(|object| object.visible)
    }

    /// How many objects of each type this house owns, most numerous first.
    ///
    /// The same shape the v2 ledger used, because a composition reading that changes shape
    /// between reports cannot be compared with an older one.
    pub fn own_composition(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for object in self.own() {
            *counts.entry(object.type_name.as_str()).or_insert(0) += 1;
        }
        let mut rows: Vec<(String, usize)> = counts
            .into_iter()
            .map(|(name, count)| (name.to_string(), count))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }
}
