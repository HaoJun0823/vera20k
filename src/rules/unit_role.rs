//! What a unit is *for*, read out of the rules rather than out of its name.
//!
//! A house that has to plan a squad needs to answer "what is missing" before
//! it answers "which type": too few things that can reach aircraft, too few
//! that can hurt a tank, too few that can deal with a wall of conscripts. The
//! rules already hold the answer to that question — every warhead carries
//! `Verses=` (the damage it does to each of the eleven armour classes) and
//! every projectile carries `AA=`/`AG=` (what it can engage at all) — so the
//! role of a type is a number derived from the same data the damage kernel
//! uses, not a name a player typed into a table.
//!
//! Nothing here is a threshold that decides membership. Every role is a
//! **weight**: a continuous score, scaled against the strongest answer the
//! same rule set has for that job, so a mod that rewrites the warheads, adds
//! an armour class or retunes damage reshapes the weights without anyone
//! revisiting a constant. A type carries several at once and the caller
//! decides what to do with them — `RoleWeights` is deliberately dumb
//! arithmetic, and the strategy layer owns the part that says "we are short of
//! anti-air".
//!
//! Scaling is per role, not per table: `1.0` means "the best answer this rule
//! set offers for that job", so a need for anti-air counts for as much as a
//! need for anti-tank even where only two types can reach the sky.
//!
//! This is a derived cache, not simulation state: it is built once from a
//! finished [`RuleSet`] and never changes.

use std::collections::BTreeMap;

use crate::rules::armor::ArmorId;
use crate::rules::capabilities::{Capabilities, Tag};
use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;

/// The jobs a unit can be good at. A type carries all of them at once, each
/// with its own weight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Hurts `Armor=none` and `flak`: soldiers, and the light vehicles that
    /// share their bracket.
    AntiInfantry,
    /// Hurts `plate`, `light`, `medium` and `heavy`: the armoured half of the
    /// field.
    AntiVehicle,
    /// Can engage aircraft at all, and hurts what it can reach.
    AntiAir,
    /// Hurts `wood`, `steel` and `concrete`: buildings, walls, bunkers.
    AntiStructure,
    /// Reaches further than the line can shoot back, and carries a payload
    /// there worth the reach.
    Artillery,
    /// Sees further and moves faster than the line it scouts for.
    Scout,
    /// Carries, repairs, captures or otherwise does not shoot.
    Support,
}

/// The roles, in the order the weight array holds them.
pub const ROLES: [Role; 7] = [
    Role::AntiInfantry,
    Role::AntiVehicle,
    Role::AntiAir,
    Role::AntiStructure,
    Role::Artillery,
    Role::Scout,
    Role::Support,
];

/// The weight at which a type counts as able to do a job rather than merely
/// carrying some of it.
///
/// `RoleWeights` are scaled against the best answer the rule set offers, so a
/// quarter of it is a type that really does the job; a tank's trace of anti-air
/// (it has none) stays out of the count. The retail brain counts its `able`
/// vector at this bar, and every caller that asks "is this job covered" has to
/// ask it the same way.
pub const ROLE_FLOOR: f64 = 0.25;

/// Seats at which a type carries rather than fights.
///
/// The night hawk (`Passengers=5`) and the hind (`Passengers=10`) are air
/// transports with a machine gun; the IFV's one seat is a fighting vehicle that
/// happens to carry someone. Five is where the retail brain draws the line
/// (`sim::strategy::retail::census`), and this is the owner of that reading so
/// a second caller cannot draw it somewhere else.
pub const TRANSPORT_SEATS: i32 = 5;

/// Whether a type belongs on an army's line.
///
/// Infantry, vehicles and aircraft that carry a weapon that fires, and are not
/// tools: a harvester, an engineer or spy (they capture, they do not shoot), a
/// transport (five seats or more, [`TRANSPORT_SEATS`]), a spawn child (a round
/// already in the air), or a yard maker (it deploys into the base's own
/// foundation).
///
/// Every term is a key the type declares — nothing here is a name or a shape
/// retail happens to have — so a mod's unit is admitted or refused by the same
/// reading. The retail brain and the autoplay policy both ask this question,
/// and this is the one place it is answered.
pub fn is_army(rules: &RuleSet, ty: &ObjectType) -> bool {
    matches!(
        ty.category,
        ObjectCategory::Infantry | ObjectCategory::Vehicle | ObjectCategory::Aircraft
    ) && Capabilities::of(rules, ty).has(Tag::Armed)
        && !ty.engineer
        && !ty.infiltrate
        && !ty.harvester
        && !ty.resource_gatherer
        && ty.passengers < TRANSPORT_SEATS
        && !ty.spawned
        && !ty
            .deploys_into
            .as_deref()
            .and_then(|name| rules.object(name))
            .is_some_and(|yard| yard.build_const_eligible)
}

/// The role that answers a type wearing `armour`: the job a squad has to cover
/// to hurt it. `None` for the two `special` classes, which nothing in the
/// retail rules wears.
pub fn role_against_armour(armour: ArmorId) -> Option<Role> {
    let at = usize::from(armour.index());
    if INFANTRY_ARMOURS.contains(&at) {
        Some(Role::AntiInfantry)
    } else if VEHICLE_ARMOURS.contains(&at) {
        Some(Role::AntiVehicle)
    } else if STRUCTURE_ARMOURS.contains(&at) {
        Some(Role::AntiStructure)
    } else {
        None
    }
}

impl Role {
    /// Where this role sits in [`RoleWeights`].
    pub fn index(self) -> usize {
        match self {
            Self::AntiInfantry => 0,
            Self::AntiVehicle => 1,
            Self::AntiAir => 2,
            Self::AntiStructure => 3,
            Self::Artillery => 4,
            Self::Scout => 5,
            Self::Support => 6,
        }
    }

    /// Whether this role is a job rather than a way of shooting: scouting and
    /// support are not modulated by damage rate, because a scout is no better
    /// a scout for carrying a bigger gun.
    fn is_job_only(self) -> bool {
        matches!(self, Self::Scout | Self::Support)
    }
}

/// A type's weight in each role, every one in `0.0..=1.0` against the best
/// type the same rule set offers.
///
/// `0.0` means "this type does not do that job", not "this type is bad at it":
/// a tank that cannot point at the sky has no anti-air weight at all.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RoleWeights(pub [f64; ROLES.len()]);

impl RoleWeights {
    /// This role's weight.
    pub fn get(self, role: Role) -> f64 {
        self.0[role.index()]
    }

    /// How much these weights answer a need: the dot product, which is what
    /// "the right unit for the job we are short of" means once both sides are
    /// numbers.
    pub fn answer(self, need: Self) -> f64 {
        ROLES
            .iter()
            .map(|role| self.get(*role) * need.get(*role))
            .sum()
    }
}

/// What one type is: its role weights, and the few raw numbers the weights
/// were made from, kept so a caller can reason about them in the units the
/// rules use.
#[derive(Clone, Copy, Debug)]
pub struct UnitProfile {
    /// Weight in each role.
    pub weights: RoleWeights,
    /// Damage per second before any armour multiplier: `Damage=` × `Burst=`
    /// over `ROF=`, at the type's best weapon.
    pub dps: f64,
    /// The best weapon's `Range=`, in leptons (256 per cell).
    pub range_leptons: f64,
    /// `Speed=`.
    pub speed: i32,
    /// `Sight=`, in cells.
    pub sight: i32,
}

/// The role weights of every unit type a finished rule set holds, built once.
#[derive(Clone, Debug, Default)]
pub struct RoleTable {
    profiles: BTreeMap<String, UnitProfile>,
}

impl RoleTable {
    /// Derive the table from a finished rule set. Infantry, vehicles and
    /// aircraft are assessed; buildings are targets, not squad members, and
    /// the strategy layer reads their armour directly.
    pub fn build(rules: &RuleSet) -> Self {
        let mut raw: BTreeMap<String, RawProfile> = BTreeMap::new();
        for category in [
            ObjectCategory::Infantry,
            ObjectCategory::Vehicle,
            ObjectCategory::Aircraft,
        ] {
            for index in 0..rules.type_array_ids(category).len() {
                let Ok(index) = i32::try_from(index) else {
                    continue;
                };
                let Some(ty) = rules.type_array_at(category, index) else {
                    continue;
                };
                raw.insert(ty.id.to_ascii_uppercase(), assess(rules, ty));
            }
        }
        // How hard the best type hits, on the log scale: a weapon that fires
        // every four frames must not be allowed to make every other type look
        // like it has no role at all.
        let best_punch = raw
            .values()
            .map(|profile| profile.dps.ln_1p())
            .fold(0.0, f64::max);

        // Half of a weight is the job, half is how hard the type hits at it.
        let mut scored: BTreeMap<String, RoleWeights> = raw
            .iter()
            .map(|(id, raw)| {
                let punch = if best_punch > 0.0 {
                    raw.dps.ln_1p() / best_punch
                } else {
                    0.0
                };
                let mut weights = RoleWeights::default();
                for role in ROLES {
                    let at = role.index();
                    weights.0[at] = if role.is_job_only() {
                        raw.aim[at]
                    } else {
                        raw.aim[at] * (PUNCH_FLOOR + (1.0 - PUNCH_FLOOR) * punch)
                    };
                }
                (id.clone(), weights)
            })
            .collect();

        // Each role is scaled against the best answer this rule set has for
        // it, so a need in one role is worth as much as a need in another:
        // "we are short of anti-air" has to be answerable in a rule set where
        // only two types can reach the sky, and no amount of good anti-tank
        // options may drown it out.
        let mut best = [0.0f64; ROLES.len()];
        for weights in scored.values() {
            for at in 0..ROLES.len() {
                best[at] = best[at].max(weights.0[at]);
            }
        }

        let mut profiles = BTreeMap::new();
        for (id, raw) in raw {
            let mut weights = scored.remove(&id).unwrap_or_default();
            for (at, weight) in weights.0.iter_mut().enumerate() {
                *weight = if best[at] > 0.0 {
                    *weight / best[at]
                } else {
                    0.0
                };
            }
            profiles.insert(
                id,
                UnitProfile {
                    weights,
                    dps: raw.dps,
                    range_leptons: raw.range_leptons,
                    speed: raw.speed,
                    sight: raw.sight,
                },
            );
        }
        Self { profiles }
    }

    /// A type's profile, when the table holds one.
    pub fn profile(&self, type_id: &str) -> Option<&UnitProfile> {
        self.profiles.get(&type_id.to_ascii_uppercase())
    }

    /// A type's weights, or a zeroed set: a type the table does not hold has
    /// no job it is good at, and a caller that adds one to a squad should see
    /// that as zero rather than as a panic.
    pub fn weights(&self, type_id: &str) -> RoleWeights {
        self.profile(type_id)
            .map_or_else(RoleWeights::default, |profile| profile.weights)
    }
}

/// What one type is, before it is put on the table's scale: how well aimed
/// each role is, and how hard the type hits.
struct RawProfile {
    /// Per role, how much of the damage gets through: the warhead's `Verses=`
    /// against that role's armour classes, with reach and trajectory folded
    /// into the artillery figure. `0.0` means the type cannot do the job.
    aim: [f64; ROLES.len()],
    /// Damage per second before any armour multiplier, at the best weapon.
    dps: f64,
    /// The best weapon's `Range=`, in leptons.
    range_leptons: f64,
    /// `Speed=`.
    speed: i32,
    /// `Sight=`, in cells.
    sight: i32,
}

/// The armour classes each role answers, as indices into `Verses=`
/// (`0=none, 1=flak, 2=plate, 3=light, 4=medium, 5=heavy, 6=wood, 7=steel,
/// 8=concrete`).
///
/// These are the brackets retail actually fills, counted off `rulesmd.ini`:
/// infantry are `none` (49 types), `flak` (7) and `plate` (8); vehicles are
/// `light` (46), `medium` (7) and `heavy` (24); buildings are `wood` (157),
/// `steel` (140) and `concrete` (99). The retail warheads agree — `AP` reads
/// 25%,25%,15% across the first three and 75%,100%,100% across the next
/// three — so `plate` is an infantry armour, not a vehicle one.
const INFANTRY_ARMOURS: &[usize] = &[0, 1, 2];
const VEHICLE_ARMOURS: &[usize] = &[3, 4, 5];
const STRUCTURE_ARMOURS: &[usize] = &[6, 7, 8];

/// A cell, to put a `Range=` in leptons next to a `Sight=` in cells.
const LEPTONS_PER_CELL: f64 = 256.0;
/// How much of a weight is the job itself rather than how hard the type hits:
/// a type with no damage rate at all keeps this much of the aim it has, so a
/// mod that retunes `Damage=` moves strength, not role.
const PUNCH_FLOOR: f64 = 0.5;
/// The range a line unit shoots at, in cells: past it a weapon outranges the
/// things that shoot back, which is what makes a piece artillery rather than
/// merely long-ranged.
const LINE_RANGE_CELLS: f64 = 6.0;
/// `Speed=` is a percentage and `Sight=` is a count of cells; these are the
/// scales that put them on one axis, not thresholds a type has to clear.
const SCOUT_SPEED_SCALE: f64 = 100.0;
const SCOUT_SIGHT_SCALE: f64 = 10.0;

/// Weigh one type against every role.
fn assess(rules: &RuleSet, ty: &ObjectType) -> RawProfile {
    let mut aim = [0.0f64; ROLES.len()];
    let mut dps: f64 = 0.0;
    let mut range_leptons: f64 = 0.0;

    for name in weapon_names(ty) {
        let Some(weapon) = rules.weapon(&name) else {
            continue;
        };
        // `NeverUse=yes` is never selected: `TechnoClass::What_Weapon_Should_I_Use
        // @ 0x006F33BF` returns the primary as soon as the secondary carries the
        // flag, so a type's worth is not in that weapon. Retail's one such
        // weapon is the `VirtualScanner` every dog and engineer carries as
        // `Secondary=`, a range-5 `InvisibleAll`/`SA` entry that exists to widen
        // the guard scan. Scored as a weapon it handed the attack dog 0.24 of
        // anti-vehicle and 0.30 of anti-air weight off a warhead it can never
        // fire, which is enough for a squad's shortfall to call itself answered.
        // `capabilities.rs` already skips the flag when it decides `Armed`.
        if weapon.never_use {
            continue;
        }
        let shots = weapon.damage.max(0) as f64 * weapon.burst.max(1) as f64;
        let rate = if weapon.rof > 0 {
            60.0 / f64::from(weapon.rof)
        } else {
            1.0
        };
        let weapon_dps = shots * rate;
        let Some(projectile) = weapon
            .projectile
            .as_deref()
            .and_then(|name| rules.projectile(name))
        else {
            continue;
        };
        let verses = weapon
            .warhead
            .as_deref()
            .and_then(|name| rules.warhead(name))
            .map_or([1.0; 11], |warhead| warhead.verses_f64);

        // How much of the damage each bracket lets through.
        let versus = |armours: &[usize]| -> f64 {
            armours.iter().map(|&at| verses[at]).sum::<f64>() / armours.len() as f64
        };
        // The hardest this weapon hits anything, which is what it would carry
        // out of the line's reach.
        let mut payload = 0.0f64;

        // What it can engage at all is a fact, not a degree: a weapon that
        // cannot reach aircraft contributes no anti-air aim however hard it
        // hits.
        if projectile.ag {
            for (role, armours) in [
                (Role::AntiInfantry, INFANTRY_ARMOURS),
                (Role::AntiVehicle, VEHICLE_ARMOURS),
                (Role::AntiStructure, STRUCTURE_ARMOURS),
            ] {
                let mean = versus(armours);
                let at = role.index();
                aim[at] = aim[at].max(mean);
                payload = payload.max(mean);
            }
        }
        if projectile.aa {
            aim[Role::AntiAir.index()] = aim[Role::AntiAir.index()].max(versus(VEHICLE_ARMOURS));
        }

        // Artillery is reach plus the means to use it: a weapon that outranges
        // the line — and, if it lobs (`Arcing=`), one that does not need the
        // line of sight either — carrying its best payload there. A weapon the
        // line can shoot back at is not artillery whatever it is aimed at.
        let cells = f64::from(weapon.range_leptons) / LEPTONS_PER_CELL;
        if projectile.ag && cells > LINE_RANGE_CELLS {
            let lob = if projectile.arcing { 1.5 } else { 1.0 };
            let at = Role::Artillery.index();
            aim[at] = aim[at].max(payload * (cells / LINE_RANGE_CELLS) * lob);
        }

        dps = dps.max(weapon_dps);
        range_leptons = range_leptons.max(f64::from(weapon.range_leptons));
    }

    // Seeing and moving is its own job, and it is the whole job of a type
    // that carries nothing to shoot with.
    let mobility =
        (f64::from(ty.speed) / SCOUT_SPEED_SCALE + f64::from(ty.sight) / SCOUT_SIGHT_SCALE) / 2.0;
    let unarmed = if dps <= 0.0 { 1.5 } else { 1.0 };
    aim[Role::Scout.index()] = mobility.max(0.0) * unarmed;
    aim[Role::Support.index()] =
        f64::from(ty.passengers > 0) + f64::from(ty.engineer) + f64::from(dps <= 0.0);

    RawProfile {
        aim,
        dps,
        range_leptons,
        speed: ty.speed,
        sight: ty.sight,
    }
}

/// Every weapon the type fires before it is promoted, duplicates dropped.
///
/// What a type *is* is its ordinary weapon, not its veteran one: retail gives
/// elite warheads flat `Verses=` — the Grizzly's `GRIZAPE` is 100% against
/// everything — so letting them in would make every elite-capable type look
/// like it has no speciality at all. Elite weapons are only a fallback for a
/// type whose ordinary list is empty.
fn weapon_names(ty: &ObjectType) -> Vec<String> {
    let mut names = dedup(&ty.weapon_list);
    if names.is_empty() {
        names = dedup(&ty.elite_weapon_list);
    }
    names
}

fn dedup(list: &[Option<String>]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in list.iter().flatten() {
        if !names.iter().any(|held| held == name) {
            names.push(name.clone());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Option<RoleTable> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        Some(RoleTable::build(&rules))
    }

    /// The retail data behind these numbers, so the assertions say what the
    /// table is for rather than restating it: `FV` is the IFV, the one Allied
    /// unit that points at aircraft; `MTNK` is the Grizzly, which does not.
    #[test]
    fn a_unit_that_cannot_reach_aircraft_has_no_anti_air_weight() {
        let Some(table) = table() else {
            return;
        };
        assert!(
            table.weights("FV").get(Role::AntiAir) > 0.0,
            "the IFV's second weapon is an AA missile"
        );
        assert_eq!(
            table.weights("MTNK").get(Role::AntiAir),
            0.0,
            "a Grizzly has no weapon that can engage aircraft at all"
        );
    }

    /// `NeverUse=yes` is a scan range, not a weapon. The attack dog carries the
    /// same `VirtualScanner` every engineer does, an `InvisibleAll`/`SA` entry
    /// that reaches five cells and is never selected to fire, and scored as a
    /// weapon it made the dog look like a counter to armour and to aircraft.
    #[test]
    fn a_never_use_weapon_earns_no_weight() {
        let Some(table) = table() else {
            return;
        };
        let dog = table.weights("ADOG");
        assert_eq!(
            dog.get(Role::AntiVehicle),
            0.0,
            "the dog's teeth do nothing to armour and its scanner never fires: {dog:?}"
        );
        assert_eq!(
            dog.get(Role::AntiAir),
            0.0,
            "and it cannot engage aircraft: {dog:?}"
        );
        assert!(
            dog.get(Role::AntiInfantry) > 0.0,
            "what it is for is infantry: {dog:?}"
        );
    }

    /// What a squad is made of, read off the keys rather than off names: a
    /// soldier and a tank are army, a truck, an engineer, a big transport and
    /// a yard maker are not.
    #[test]
    fn army_is_a_key_and_not_a_name() {
        let Some(rules) = crate::rules::retail_ini_fixture::retail_unlayered_rules() else {
            return;
        };
        let army = |id: &str| rules.object(id).is_some_and(|ty| is_army(&rules, ty));
        assert!(army("E1"), "a rifleman fights");
        assert!(army("MTNK"), "a tank fights");
        assert!(!army("HARV"), "a harvester is the house's income");
        assert!(!army("ENGINEER"), "an engineer captures, it does not shoot");
        assert!(
            !army("SHAD"),
            "a five-seat night hawk carries the army, it is not the army"
        );
        // A seat or two is a fighting vehicle that happens to carry someone.
        assert!(
            army("FV"),
            "the IFV's one seat does not make it a transport"
        );
    }

    #[test]
    fn the_table_covers_the_retail_types() {
        let Some(table) = table() else {
            return;
        };
        for name in ["E1", "MTNK", "FV", "SREF", "JUMPJET", "CMIN", "AMCV"] {
            assert!(
                table.profile(name).is_some(),
                "the table holds the retail types a squad is made of: {name} is missing"
            );
        }
    }

    /// The point of weights rather than labels: what a type is good at is
    /// relative, and the table has to order the two of them the way the
    /// warheads do.
    #[test]
    fn a_tank_outranks_a_soldier_against_armour() {
        let Some(table) = table() else {
            return;
        };
        assert!(
            table.weights("MTNK").get(Role::AntiVehicle)
                > table.weights("E1").get(Role::AntiVehicle),
            "a tank's warhead bites armour harder than a rifle does"
        );
        assert!(
            table.weights("E1").get(Role::AntiInfantry)
                > table.weights("CMIN").get(Role::AntiInfantry),
            "and a soldier outranks a harvester at everything but ore"
        );
    }

    /// The weights have to separate the jobs enough to plan with: what a type
    /// is *for* is written in its warhead, and retail `AP` bites armour and
    /// bounces off soldiers while `SA` does the opposite.
    #[test]
    fn a_types_own_weights_order_what_its_warhead_is_for() {
        let Some(table) = table() else {
            return;
        };
        let mtnk = table.weights("MTNK");
        assert!(
            mtnk.get(Role::AntiVehicle) > mtnk.get(Role::AntiInfantry),
            "a Grizzly's AP round is an anti-armour round: {mtnk:?}"
        );
        let e1 = table.weights("E1");
        assert!(
            e1.get(Role::AntiInfantry) > e1.get(Role::AntiVehicle),
            "and a rifle is the other way round: {e1:?}"
        );
        assert!(
            table.weights("SREF").get(Role::Artillery) > table.weights("MTNK").get(Role::Artillery),
            "a Prism tank reaches past the line and a Grizzly does not"
        );
    }

    /// The brackets are retail's, read off what the warheads do rather than
    /// off what the armour names sound like: `plate` is an infantry armour
    /// (`AP` is 15% against it and 75–100% against the vehicle armours), and
    /// the two `special` classes answer no role because nothing retail wears
    /// them.
    #[test]
    fn the_armour_brackets_follow_the_warheads() {
        for name in ["none", "flak", "plate"] {
            assert_eq!(
                role_against_armour(crate::rules::armor::armor_index(name)),
                Some(Role::AntiInfantry),
                "{name} is an infantry armour"
            );
        }
        for name in ["light", "medium", "heavy"] {
            assert_eq!(
                role_against_armour(crate::rules::armor::armor_index(name)),
                Some(Role::AntiVehicle),
                "{name} is a vehicle armour"
            );
        }
        for name in ["wood", "steel", "concrete"] {
            assert_eq!(
                role_against_armour(crate::rules::armor::armor_index(name)),
                Some(Role::AntiStructure),
                "{name} is a building armour"
            );
        }
        assert_eq!(
            role_against_armour(crate::rules::armor::armor_index("special_1")),
            None,
            "and the special classes answer no role"
        );
    }

    /// A need is answered by the sum of what a type brings, so the same table
    /// ranks a type differently depending on what the squad is short of.
    #[test]
    fn the_same_type_answers_two_different_needs_differently() {
        let Some(table) = table() else {
            return;
        };
        let mut anti_air = RoleWeights::default();
        anti_air.0[Role::AntiAir.index()] = 1.0;
        let mut anti_vehicle = RoleWeights::default();
        anti_vehicle.0[Role::AntiVehicle.index()] = 1.0;

        let fv = table.weights("FV");
        let mtnk = table.weights("MTNK");
        assert!(
            fv.answer(anti_air) > mtnk.answer(anti_air),
            "against an air threat the IFV is the answer and the Grizzly is not"
        );
        assert!(
            mtnk.answer(anti_vehicle) > mtnk.answer(anti_air),
            "and the Grizzly's own weights answer a ground threat, not an air one: {mtnk:?}"
        );
    }
}
