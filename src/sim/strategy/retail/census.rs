//! The house's own ledger: one pass over the entity store and the factories,
//! read by every decision that would otherwise ask "how many of mine" again.
//!
//! Before this module each decision re-walked `Observation::own_entities` for
//! its own question — the fighter count, the harvesters, the refineries, the
//! `[AI]` list counts, the price bands, the role coverage — six passes over the
//! whole store per frame, on a design that scales to twenty thousand objects.
//! One owner answers all of them once, and the answers are the same ones: the
//! counts are of the entities the projection yields, the lines are the
//! factories' own fields (`Observation::factory_line`), and the role coverage
//! is the same [`RoleWeights`] dot product the old `shortfall` computed.
//!
//! Rebuilt every frame, never cached across one: a ledger that lags the entity
//! store by a frame is a ledger a decision can be wrong with. The storage is
//! reused (the vectors keep their capacity), so a steady army costs no
//! allocation.
//!
//! Determinism: entity-storage order and `BTreeMap` order only. The role
//! arithmetic is `f64` and comparisons only, exactly as the weights it reads.

use std::collections::BTreeMap;

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::rules::unit_role::{ROLES, Role, RoleTable, role_against_armour};
use crate::sim::control::observation::{EnemyEntity, Observation};
use crate::sim::intern::InternedId;
use crate::sim::mission::MissionId;
use crate::sim::production::{ProductionCategory, category_for_object};

/// The five pools a house's assets fall in, in the order [`Census::counts`]
/// holds them. Naval units are `Vehicle` types carrying `Naval=`, so they are
/// their own pool here rather than a second reading of the same category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Slot {
    Building,
    Infantry,
    Vehicle,
    Aircraft,
    Ship,
}

pub(super) const SLOTS: [Slot; 5] = [
    Slot::Building,
    Slot::Infantry,
    Slot::Vehicle,
    Slot::Aircraft,
    Slot::Ship,
];

impl Slot {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Building => 0,
            Self::Infantry => 1,
            Self::Vehicle => 2,
            Self::Aircraft => 3,
            Self::Ship => 4,
        }
    }

    /// The pool a production category's finished product lands in. A defense
    /// is a building: it stands on base ground like any other.
    pub(super) fn of(category: ProductionCategory) -> Self {
        match category {
            ProductionCategory::Building | ProductionCategory::Defense => Self::Building,
            ProductionCategory::Infantry => Self::Infantry,
            ProductionCategory::Vehicle => Self::Vehicle,
            ProductionCategory::Aircraft => Self::Aircraft,
            ProductionCategory::Ship => Self::Ship,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Building => "building",
            Self::Infantry => "infantry",
            Self::Vehicle => "vehicle",
            Self::Aircraft => "aircraft",
            Self::Ship => "ship",
        }
    }
}

/// The production lines, in the order [`Census::lines`] holds them.
pub(super) const LINE_BUILDING: usize = 0;
pub(super) const LINE_DEFENSE: usize = 1;
pub(super) const LINE_INFANTRY: usize = 2;
pub(super) const LINE_VEHICLE: usize = 3;
pub(super) const LINE_AIRCRAFT: usize = 4;
pub(super) const LINE_SHIP: usize = 5;

fn line_index(category: ProductionCategory) -> usize {
    match category {
        ProductionCategory::Building => LINE_BUILDING,
        ProductionCategory::Defense => LINE_DEFENSE,
        ProductionCategory::Infantry => LINE_INFANTRY,
        ProductionCategory::Vehicle => LINE_VEHICLE,
        ProductionCategory::Aircraft => LINE_AIRCRAFT,
        ProductionCategory::Ship => LINE_SHIP,
    }
}

/// One line's occupancy this frame.
///
/// A house that has never started a build has no factory of that category in
/// its own slot yet — `HouseClass`'s slot is filled when production starts,
/// not when the barracks is placed — so "no line" and "an idle line" look the
/// same from here, and both mean the same thing to a caller: nothing is being
/// made, so a request is what creates the line. Whether the house can make a
/// type at all is `CanBuild`'s answer, not this struct's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) product: Option<InternedId>,
    pub(super) suspended: bool,
    pub(super) queued: usize,
    /// The finished product the factory still holds: a building waiting for a
    /// cell, or a unit waiting to be taken out.
    pub(super) ready: Option<InternedId>,
}

impl Line {
    /// The factory is working: something is on the line, in the queue, or
    /// finished and not yet taken.
    pub(super) fn busy(&self) -> bool {
        self.product.is_some() || self.ready.is_some() || self.queued > 0
    }
}

/// What a type is, to the decisions that count it. Resolved once per unit
/// while the ledger is built, so no decision has to re-open the rules.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Traits {
    /// A unit that fights: infantry, vehicle or aircraft with a primary weapon,
    /// and neither a harvester, an engineer nor a yard maker
    /// ([`super::orders::is_combat_unit`]).
    pub(super) combat: bool,
    /// `ResourceGatherer=`: the house's income.
    pub(super) harvester: bool,
    /// `ResourceDestination=`: an unloading bay.
    pub(super) refinery: bool,
    /// `BuildConst=` eligible: a Construction Yard.
    pub(super) yard: bool,
    /// `DeploysInto=` a yard: an MCV.
    pub(super) yard_maker: bool,
    pub(super) engineer: bool,
    pub(super) infiltrator: bool,
    pub(super) occupier: bool,
    /// `DeployFire=`: a soldier the D key turns into a position
    /// (`[E1]` and `[GGI]` carry `Deployer=yes` with it).
    pub(super) deployer: bool,
    /// `NotHuman=yes` with `Natural=yes`, the retail attack dog's own pair: a
    /// scanner with teeth, and the only type the rules mark as neither human
    /// nor mechanical.
    pub(super) scout: bool,
    /// It has a weapon in some slot, primary or not.
    ///
    /// `Primary=` alone was the test, which is a fact about retail's key
    /// layout and not about the type: a unit a mod gives only a `Secondary=` is
    /// armed, and the brain refused to treat it as army. What slot the weapon
    /// sits in is [`Self::deploy_slot`]'s business.
    pub(super) armed: bool,
    /// It takes a building by walking into it: `Engineer=` or `Infiltrate=`.
    pub(super) captures: bool,
    /// It carries others: `Passengers=` at or above a transport's seats.
    pub(super) carries: bool,
    /// The weapon slot it fires while deployed, for a `DeployFire=` infantry:
    /// `DeployFireWeapon=`, slot 1 (the secondary) by default. `None` when the
    /// type has no such pose or the slot is empty.
    pub(super) deploy_slot: Option<usize>,
}

/// Seats at which a type is a transport rather than a fighting unit.
const TRANSPORT_SEATS: i32 = 5;

impl Traits {
    /// What a type can do, read from the keys the type itself declares.
    ///
    /// One owner for the question every decision asks — "is this thing army, a
    /// tool, a carrier, a position" — and the answer is *the type's own keys*,
    /// not its name and not the shape of retail's `Primary=`. The brain used to
    /// answer it in five places with five hand-rolled tests over one or two
    /// keys, which is correct for `[E1]` and `[ADOG]` and wrong for anything a
    /// mod adds: an engineer with no `Primary=` was not army and not a tool, a
    /// unit with only a `Secondary=` was not army at all, and a type whose
    /// custom ability lives in a key nobody had looked at yet was invisible.
    ///
    /// Every field below is a key. A new key that describes what a type can do
    /// belongs here first, and the decisions read it from here.
    pub(super) fn of(rules: &RuleSet, ty: &ObjectType) -> Self {
        let armed = (0..8).any(|slot| ty.weapon_at(slot).is_some());
        let captures = ty.engineer || ty.infiltrate;
        let carries = ty.passengers >= TRANSPORT_SEATS;
        // `DeployFireWeapon=` names the slot, defaulting to the secondary: the
        // G.I. stands up with `M60` (slot 0) and digs in with `Para` (slot 1).
        let deploy_slot = ty
            .deploy_fire
            .then(|| usize::try_from(ty.deploy_fire_weapon.max(0)).ok())
            .flatten()
            .filter(|slot| ty.weapon_at(*slot).is_some());
        Self {
            combat: matches!(
                ty.category,
                ObjectCategory::Infantry | ObjectCategory::Vehicle | ObjectCategory::Aircraft
            ) && armed
                && !captures
                && !ty.engineer
                && !ty.harvester
                && !ty.resource_gatherer
                && !carries
                && !ty
                    .deploys_into
                    .as_deref()
                    .and_then(|name| rules.object(name))
                    .is_some_and(|yard| yard.build_const_eligible),
            harvester: ty.resource_gatherer,
            refinery: ty.resource_destination,
            yard: ty.build_const_eligible,
            yard_maker: super::expand::makes_a_yard(rules, ty),
            engineer: ty.engineer,
            infiltrator: ty.infiltrate,
            occupier: ty.occupier,
            deployer: ty.deploy_fire,
            scout: ty.not_human && ty.natural,
            armed,
            captures,
            carries,
            deploy_slot,
        }
    }
}

/// One object of the house, as the decisions see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Unit {
    pub(super) entity_id: u64,
    pub(super) type_id: InternedId,
    pub(super) cell: (u16, u16),
    /// Health as permille of the type's full strength.
    pub(super) health_permille: u16,
    /// What it is worth in a fight, in hit points: [`fight_points`] of its own
    /// type at the health it has left, and zero for anything that does not
    /// fight. Resolved while the ledger is built, because the ledger is the
    /// only pass that has the type open.
    pub(super) points: i32,
    pub(super) mission: MissionId,
    pub(super) orderable: bool,
    /// A team's script or one of the player's control groups owns it
    /// ([`Observation::owned_by_hand`]).
    pub(super) hand: bool,
    pub(super) slot: Slot,
    /// The price band of its type: cheap, the line unit, the heavy end.
    pub(super) tier: usize,
    pub(super) traits: Traits,
}

/// The house's assets for one frame.
#[derive(Debug, Default)]
pub(super) struct Census {
    /// Building, infantry, vehicle, aircraft, ship.
    pub(super) counts: [usize; 5],
    /// Every own type's count, for the questions a name-parsed rules list asks.
    pub(super) by_type: BTreeMap<InternedId, usize>,
    pub(super) lines: [Line; 6],
    pub(super) units: Vec<Unit>,
    pub(super) enemies: Vec<EnemyEntity>,
    /// Cells of the house's own buildings: the yard, the plants, the
    /// factories, the defenses.
    pub(super) home_cells: Vec<(u16, u16)>,
    /// The base's anchor: the yard when one stands, otherwise whatever building
    /// the house still has.
    pub(super) base_cell: Option<(u16, u16)>,
    pub(super) harvesters: usize,
    pub(super) refineries: usize,
    pub(super) yards: usize,
    pub(super) yard_makers: usize,
    /// Units that fight, harvesters and engineers aside.
    pub(super) fighters: usize,
    /// Fighting units per price band, per pool.
    pub(super) tiers: [[usize; 3]; 5],
    /// Finished buildings waiting for a cell, in factory order.
    pub(super) ready: Vec<InternedId>,
    /// The jobs the enemy's own types ask for, counted.
    pub(super) threat: [u32; ROLES.len()],
    /// How many of this house's fighting units can actually do each job: their
    /// weight in it clears [`ROLE_FLOOR`].
    pub(super) able: [u32; ROLES.len()],
}

/// The weight at which a type counts as able to do a job rather than merely
/// carrying some of it. `RoleWeights` are scaled against the best answer the
/// rule set offers, so a quarter of it is a type that really does the job; a
/// tank's trace of anti-air (it has none) stays out of the count.
const ROLE_FLOOR: f64 = 0.25;

/// What one object is worth in a fight, in hit points.
///
/// `Strength=` is the rules' own measure of what a type survives, and it is the
/// only number both sides can be scored in: a Grizzly's 300 against a
/// conscript's 100 is close to how many of the latter the former is worth.
/// Health scales it, because a damaged tank is not a tank.
///
/// A type that cannot shoot back is worth nothing here whatever it survives —
/// a refinery is ground the army stands on, not a threat it has to outfight —
/// which is also what keeps the enemy's own economy out of the comparison.
pub(super) fn fight_points(ty: &ObjectType, health_permille: u16) -> i32 {
    if ty.primary().is_none() {
        return 0;
    }
    let full = ty.strength.max(0);
    let scaled = i64::from(full) * i64::from(health_permille.min(1_000)) / 1_000;
    i32::try_from(scaled).unwrap_or(i32::MAX)
}

/// The price band a unit falls in: cheap, the line unit, or the heavy end. The
/// cuts are retail's own steps — a soldier around 200, a line vehicle around
/// 900, the heavy and long-range types above that — read as the type's own
/// `Cost=` through the house's cost factors.
pub(super) fn price_tier(cost: i32) -> usize {
    if cost <= CHEAP_UNIT_CREDITS {
        0
    } else if cost <= MID_UNIT_CREDITS {
        1
    } else {
        2
    }
}

/// The price cuts between the bands a unit can fall in.
pub(super) const CHEAP_UNIT_CREDITS: i32 = 500;
pub(super) const MID_UNIT_CREDITS: i32 = 1_000;

/// The job that answers `ty`: shooting at an aircraft is its own job, and for
/// everything else it is the armour the type wears.
pub(super) fn counter(ty: &ObjectType) -> Option<Role> {
    if ty.category == ObjectCategory::Aircraft {
        return Some(Role::AntiAir);
    }
    role_against_armour(ty.armor_id())
}

impl Census {
    /// What the lines have been promised and have not paid out yet: the
    /// product on each line, plus the copies queued behind it.
    ///
    /// A build starts owing its `Cost_Of` and pays it down as it steps, so a
    /// queue costs the wallet nothing today and a budget that reads the
    /// balance alone spends the same credits twice. The queue holds
    /// `QueueEntry` types the projection does not name, so a queued copy is
    /// priced at the product it is queued behind — the answer is a floor, and
    /// a floor is what a budget wants.
    pub(super) fn committed(&self, rules: &RuleSet, obs: &Observation<'_>) -> i32 {
        let mut total = 0i32;
        for line in &self.lines {
            let Some(type_id) = line.product else {
                continue;
            };
            let Some(ty) = obs.object_type(rules, type_id) else {
                continue;
            };
            let copies = i32::try_from(line.queued).unwrap_or(i32::MAX).saturating_add(1);
            total = total.saturating_add(obs.cost_of(rules, ty).saturating_mul(copies));
        }
        total
    }

    /// Rebuild the ledger from this frame's projection. Called once, at the
    /// top of the brain's `think`.
    pub(super) fn refresh(&mut self, roles: &RoleTable, rules: &RuleSet, obs: &Observation<'_>) {
        self.counts = [0; 5];
        self.by_type.clear();
        self.lines = [Line::default(); 6];
        self.units.clear();
        self.enemies.clear();
        self.home_cells.clear();
        self.ready.clear();
        self.harvesters = 0;
        self.refineries = 0;
        self.yards = 0;
        self.yard_makers = 0;
        self.fighters = 0;
        self.tiers = [[0; 3]; 5];
        self.threat = [0; ROLES.len()];
        self.able = [0; ROLES.len()];

        for entity in obs.own_entities() {
            let Some(ty) = obs.object_type(rules, entity.type_id) else {
                continue;
            };
            let slot = Slot::of(category_for_object(ty));
            let max_health = ty.strength.max(0);
            let health_permille = if max_health > 0 {
                let scaled = i64::from(entity.health.max(0)) * 1000 / i64::from(max_health);
                u16::try_from(scaled.clamp(0, 1000)).unwrap_or(1000)
            } else {
                0
            };
            let traits = Traits::of(rules, ty);
            let combat = traits.combat;
            let tier = if combat {
                price_tier(obs.cost_of(rules, ty))
            } else {
                0
            };

            self.counts[slot.index()] += 1;
            *self.by_type.entry(entity.type_id).or_insert(0) += 1;
            if traits.harvester {
                self.harvesters += 1;
            }
            if traits.refinery {
                self.refineries += 1;
            }
            if traits.yard {
                self.yards += 1;
                self.base_cell = Some(entity.cell);
            }
            if traits.yard_maker {
                self.yard_makers += 1;
            }
            if ty.category == ObjectCategory::Building {
                self.home_cells.push(entity.cell);
                self.base_cell.get_or_insert(entity.cell);
            }
            let points = if combat {
                fight_points(ty, health_permille)
            } else {
                0
            };
            if combat {
                self.fighters += 1;
                self.tiers[slot.index()][tier] += 1;
                let weights = roles.weights(&ty.id);
                for role in ROLES {
                    if weights.get(role) >= ROLE_FLOOR {
                        self.able[role.index()] += 1;
                    }
                }
            }

            self.units.push(Unit {
                entity_id: entity.entity_id,
                type_id: entity.type_id,
                cell: entity.cell,
                health_permille,
                points,
                mission: entity.mission,
                orderable: entity.orderable,
                hand: obs.owned_by_hand(entity.entity_id),
                slot,
                tier,
                traits,
            });
        }

        for category in [
            ProductionCategory::Building,
            ProductionCategory::Defense,
            ProductionCategory::Infantry,
            ProductionCategory::Vehicle,
            ProductionCategory::Aircraft,
            ProductionCategory::Ship,
        ] {
            let index = line_index(category);
            if let Some(line) = obs.factory_line(category) {
                self.lines[index] = Line {
                    product: line.product,
                    suspended: line.suspended,
                    queued: line.queued,
                    ready: obs.finished_product(category),
                };
            }
        }
        for building in obs.ready_buildings() {
            self.ready.push(building.type_id);
        }

        for enemy in obs.enemy_entities() {
            let Some(ty) = obs.object_type(rules, enemy.type_id) else {
                continue;
            };
            // A building is ground the army stands on, not a threat it has to
            // be composed against, and a spawn child is a round already in the
            // air (`[V3ROCKET] Spawned=yes`).
            if ty.category != ObjectCategory::Building
                && !ty.spawned
                && let Some(role) = counter(ty)
            {
                self.threat[role.index()] += 1;
            }
            self.enemies.push(enemy);
        }
    }

    pub(super) fn type_count(&self, type_id: InternedId) -> usize {
        self.by_type.get(&type_id).copied().unwrap_or(0)
    }

    /// The count of the types `names` lists, from the rules' own `[AI]` lists.
    pub(super) fn count_named(&self, obs: &Observation<'_>, names: &[String]) -> usize {
        names
            .iter()
            .filter_map(|name| obs.type_id(name))
            .map(|type_id| self.type_count(type_id))
            .sum()
    }

    pub(super) fn slot_count(&self, slot: Slot) -> usize {
        self.counts[slot.index()]
    }

    pub(super) fn line(&self, category: ProductionCategory) -> &Line {
        &self.lines[line_index(category)]
    }

    pub(super) fn combat_units(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|unit| unit.traits.combat)
    }

    /// What the house has to fight with: one number, in hit points, so that a
    /// decision that asks "is this enough" has something to compare against
    /// rather than a count of things that are not alike.
    pub(super) fn army_points(&self) -> i32 {
        self.units
            .iter()
            .fold(0i32, |total, unit| total.saturating_add(unit.points))
    }

    /// What the enemy has within `radius` cells of `cell`, in hit points: the
    /// force that will meet an army walking there.
    ///
    /// Types are opened here because this is the one pass that holds them; the
    /// answer is what the house can see, and an enemy it has never seen is not
    /// one it can weigh — the honest reading, and the reason the comparison
    /// below is a floor and not a promise.
    pub(super) fn enemy_points(
        &self,
        rules: &RuleSet,
        obs: &Observation<'_>,
        cell: (u16, u16),
        radius: i32,
    ) -> i32 {
        let reach = radius.saturating_mul(radius);
        self.enemies
            .iter()
            .filter(|enemy| {
                let dx = i32::from(enemy.cell.0) - i32::from(cell.0);
                let dy = i32::from(enemy.cell.1) - i32::from(cell.1);
                dx * dx + dy * dy <= reach
            })
            .filter_map(|enemy| obs.object_type(rules, enemy.type_id))
            .fold(0i32, |total, ty| total.saturating_add(fight_points(ty, 1_000)))
    }

    /// The job gaps: what the enemy's field asks for, less what this house can
    /// already answer, floored at zero. `None` when nothing is short.
    ///
    /// Counted in units rather than summed weights: a type either can do a job
    /// or it cannot, so one IFV no longer "covers" a tank the way a weight sum
    /// let it, which is what left the army one type deep.
    pub(super) fn shortfall(&self) -> Option<[i32; ROLES.len()]> {
        let mut need = [0i32; ROLES.len()];
        let mut short = false;
        for role in ROLES {
            let gap = i32::try_from(self.threat[role.index()]).unwrap_or(i32::MAX)
                .saturating_sub(i32::try_from(self.able[role.index()]).unwrap_or(i32::MAX));
            if gap > 0 {
                need[role.index()] = gap;
                short = true;
            }
        }
        short.then_some(need)
    }

    /// One line of the ledger, for the log the client already writes: a report
    /// of "the AI builds nothing" has to be answerable from the log alone.
    pub(super) fn describe(&self) -> String {
        let slots: Vec<String> = SLOTS
            .iter()
            .map(|slot| format!("{}={}", slot.label(), self.slot_count(*slot)))
            .collect();
        format!(
            "{} harv={} ref={} yard={} fighters={} threat=[{}] able=[{}]",
            slots.join(" "),
            self.harvesters,
            self.refineries,
            self.yards,
            self.fighters,
            self.threat
                .iter()
                .map(|count| count.to_string())
                .collect::<Vec<_>>()
                .join(","),
            self.able
                .iter()
                .map(|count| count.to_string())
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::world::Simulation;

    /// The retail rival arena, needed to field real object types. The rules
    /// come from the repository's own `ini/` snapshot, so the ledger's counts
    /// are regression tested without a retail install.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    fn ledger(rules: &RuleSet, sim: &Simulation, owner: InternedId) -> Census {
        let mut census = Census::default();
        census.refresh(&RoleTable::build(rules), rules, &Observation::new(sim, owner));
        census
    }

    /// The ledger is what a decision asks instead of six walks over the entity
    /// store, so it has to answer all of them: the five pools, the key types,
    /// and each line's occupancy.
    #[test]
    fn the_ledger_counts_the_house_by_pool_type_and_line() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("a yard");
        sim.spawn_object("GAPOWR", "Americans", 20, 16, 255, &rules)
            .expect("a power plant");
        for cell in [(10u16, 10u16), (12, 10), (14, 10)] {
            sim.spawn_object("E1", "Americans", cell.0, cell.1, 255, &rules)
                .expect("a G.I.");
        }
        sim.spawn_object("MTNK", "Americans", 10, 14, 255, &rules)
            .expect("a Grizzly");
        sim.spawn_object("CMIN", "Americans", 14, 14, 255, &rules)
            .expect("a Chrono Miner");

        let census = ledger(&rules, &sim, owner);
        assert_eq!(
            census.slot_count(Slot::Building),
            2,
            "the yard and the plant"
        );
        assert_eq!(census.slot_count(Slot::Infantry), 3, "three G.I.s");
        assert_eq!(
            census.slot_count(Slot::Vehicle),
            2,
            "a Grizzly and a Chrono Miner"
        );
        assert_eq!(census.yards, 1, "one yard");
        assert_eq!(census.harvesters, 1, "one harvester");
        assert_eq!(census.fighters, 4, "three G.I.s and the Grizzly");
        assert!(
            census.lines.iter().all(|line| !line.busy()),
            "no factory is making anything"
        );
    }

    /// What a type is comes from the keys the type declares, and every decision
    /// reads it from here. The point is the mod case: a unit that says
    /// `Engineer=yes` is a tool and not army, one that says `Passengers=` is a
    /// carrier and not army, and one whose weapon sits only in `Secondary=` is
    /// armed — none of which the hand-rolled tests that used to live in five
    /// decisions could tell.
    #[test]
    fn capabilities_are_read_from_the_types_own_keys() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let obs = Observation::new(&sim, owner);
        let of = |name: &str| {
            let ty = obs
                .type_id(name)
                .and_then(|type_id| obs.object_type(&rules, type_id))
                .unwrap_or_else(|| panic!("{name} is in the rules"));
            (ty, Traits::of(&rules, ty))
        };

        let (_, engineer) = of("ENGINEER");
        assert!(engineer.captures && !engineer.combat, "a tool, not army");
        let (_, gi) = of("E1");
        assert!(gi.combat && gi.armed, "a rifleman is army");
        assert_eq!(gi.deploy_slot, Some(1), "and digs in with its second slot");
        let (_, dog) = of("ADOG");
        assert!(dog.scout && dog.armed, "the dog scans, and bites");
        let (_, transport) = of("SHAD");
        assert!(transport.carries && !transport.combat, "a carrier");
        let (_, tank) = of("MTNK");
        assert!(tank.combat && !tank.captures, "a tank");
    }

    /// A fight is weighed in what the other side can shoot back with, so a
    /// building with no gun is worth nothing in it — the enemy's refinery is
    /// ground the army walks over, not a defender it has to outfight.
    #[test]
    fn a_type_that_cannot_shoot_back_is_worth_nothing_in_a_fight() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let obs = Observation::new(&sim, owner);
        let by_name = |name: &str| {
            obs.type_id(name)
                .and_then(|type_id| obs.object_type(&rules, type_id))
                .unwrap_or_else(|| panic!("{name} is in the rules"))
        };
        assert_eq!(
            fight_points(by_name("NAREFN"), 1_000),
            0,
            "a refinery has no gun"
        );
        let tank = fight_points(by_name("MTNK"), 1_000);
        assert!(tank > 0, "a Grizzly does: {tank}");
        assert_eq!(
            fight_points(by_name("MTNK"), 500),
            tank / 2,
            "half health is half the tank"
        );
    }

    /// What the army is short of is read off the jobs the enemy's own types
    /// ask for: a tank by something that bites armour, a conscript by
    /// something that bites soldiers, and neither by reach.
    #[test]
    fn the_jobs_the_army_is_short_of_are_the_enemys_own() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [("HTNK", (6u16, 6u16)), ("E2", (8, 8))] {
            sim.spawn_object(name, "Russians", cell.0, cell.1, 64, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        let census = ledger(&rules, &sim, owner);
        let need = census.shortfall().expect("an army of nothing is short of it all");

        assert!(need[Role::AntiVehicle.index()] > 0, "a Rhino tank: {need:?}");
        assert!(need[Role::AntiInfantry.index()] > 0, "a conscript: {need:?}");
        assert_eq!(need[Role::AntiAir.index()], 0, "nothing in the air");
        assert_eq!(need[Role::AntiStructure.index()], 0, "no buildings");
        assert_eq!(need[Role::Artillery.index()], 0, "nothing asks for reach");
    }

    /// A round already in the air is not an aircraft. `[V3ROCKET]` sits in the
    /// aircraft table with `Spawned=yes`, and reading it as air asked the army
    /// for anti-air: the measured match answered two rockets with ten IFVs
    /// while the bunkers and Tesla coils that took the base went unanswered.
    #[test]
    fn a_fired_rocket_does_not_ask_the_army_for_anti_air() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        for (name, cell) in [("V3ROCKET", (6u16, 6u16)), ("E2", (8, 8))] {
            sim.spawn_object(name, "Russians", cell.0, cell.1, 64, &rules)
                .unwrap_or_else(|| panic!("{name}"));
        }
        let census = ledger(&rules, &sim, owner);
        let need = census.shortfall().expect("an army of nothing is short of it all");
        assert_eq!(census.threat[Role::AntiAir.index()], 0, "the rocket is not air");
        assert!(need[Role::AntiInfantry.index()] > 0, "the conscript still counts");
    }

    /// A building is ground the army stands on, not a threat to compose
    /// against, so a base with nothing but structures asks for nothing. A
    /// refinery is not a fair sample: it comes with a harvester of its own,
    /// and a harvester is a vehicle.
    #[test]
    fn a_base_of_buildings_asks_the_army_for_nothing() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("NAPOWR", "Russians", 8, 8, 255, &rules)
            .expect("an enemy power plant");
        let census = ledger(&rules, &sim, owner);
        assert!(census.threat.iter().all(|count| *count == 0));
        assert!(census.shortfall().is_none());
    }
}
