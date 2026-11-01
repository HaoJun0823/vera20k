//! Unit profiles: what a unit can do, derived from the rules.
//!
//! Every fact the policy knows about a type is computed here, from the same
//! readers the simulation uses. No type name appears in the policy, which is
//! what lets a mod's unit be used without a code change.
//!
//! The profile is integer-valued: the policy never compares floats, and a
//! damage figure is kept in permille so two builds rank targets identically on
//! every platform.

use std::collections::BTreeMap;

use vera20k_core::rules::object_type::{ObjectCategory, ObjectType};
use vera20k_core::rules::ruleset::RuleSet;

/// The armor registry size the verses table is indexed by, in the order
/// `Armor=` names resolve to (`rules::armor::armor_index`).
pub const ARMOR_SLOTS: usize = 11;

/// Frames in one second of the simulation, for turning a rate-of-fire into a
/// per-second figure.
const FRAMES_PER_SECOND: i64 = 15;

/// What a unit is for, independent of its name. A type may carry several.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Roles(pub u16);

impl Roles {
    pub const HARVESTER: Self = Self(1 << 0);
    pub const ENGINEER: Self = Self(1 << 1);
    pub const AGENT: Self = Self(1 << 2);
    pub const SABOTEUR: Self = Self(1 << 3);
    pub const OCCUPIER: Self = Self(1 << 4);
    pub const TRANSPORT: Self = Self(1 << 5);
    pub const DEPLOYER: Self = Self(1 << 6);
    pub const REFINERY: Self = Self(1 << 7);
    pub const ARMED: Self = Self(1 << 8);
    pub const STRUCTURE: Self = Self(1 << 9);
    /// A structure whose factory produces infantry (`Factory=InfantryType`).
    pub const TRAINS_INFANTRY: Self = Self(1 << 10);
    /// A structure whose factory produces vehicles (`Factory=UnitType`).
    pub const BUILDS_VEHICLES: Self = Self(1 << 11);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The roles as names, for the profile dump and the logs.
    pub fn names(self) -> Vec<&'static str> {
        const ALL: &[(&str, Roles)] = &[
            ("harvester", Roles::HARVESTER),
            ("engineer", Roles::ENGINEER),
            ("agent", Roles::AGENT),
            ("saboteur", Roles::SABOTEUR),
            ("occupier", Roles::OCCUPIER),
            ("transport", Roles::TRANSPORT),
            ("deployer", Roles::DEPLOYER),
            ("refinery", Roles::REFINERY),
            ("armed", Roles::ARMED),
            ("structure", Roles::STRUCTURE),
            ("trains-infantry", Roles::TRAINS_INFANTRY),
            ("builds-vehicles", Roles::BUILDS_VEHICLES),
        ];
        ALL.iter()
            .filter(|(_, role)| self.contains(*role))
            .map(|(name, _)| *name)
            .collect()
    }
}

/// One weapon, reduced to what a decision compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponProfile {
    pub id: String,
    pub damage: i32,
    pub rof: i32,
    pub burst: i32,
    /// Reach in cells, from the stored lepton range with the simulation's own
    /// conversion (a cell is 256 leptons).
    pub range_cells: i32,
    /// Damage per second against each armor class, in permille of one hit
    /// point. Index follows `Armor=` order.
    pub dps_permille: [i64; ARMOR_SLOTS],
    /// Splash radius in cells from the warhead.
    pub splash_cells: i32,
}

impl WeaponProfile {
    /// Whether this weapon does anything to that armor class.
    pub fn can_hurt(&self, armor: usize) -> bool {
        self.dps_permille.get(armor).is_some_and(|dps| *dps > 0)
    }

    /// The best damage this weapon does to anything, for ranking.
    pub fn best_dps(&self) -> i64 {
        self.dps_permille.iter().copied().max().unwrap_or(0)
    }
}

/// One object type, as the policy understands it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitProfile {
    pub id: String,
    pub category: ObjectCategory,
    pub cost: i32,
    pub strength: i32,
    /// The type's own armor, as an index into the verses table.
    pub armor: usize,
    pub speed: i32,
    pub sight: i32,
    pub roles: Roles,
    pub weapons: Vec<WeaponProfile>,
    pub passengers: i32,
    pub ammo: i32,
    pub reload: i32,
    /// Power output for a structure (negative is drain).
    pub power: i32,
    /// `BuildLimit=`: a hero or a one-off.
    pub build_limit: i32,
}

impl UnitProfile {
    /// Derive one type's profile. Pure: the same rules always give the same
    /// profile.
    pub fn of(rules: &RuleSet, ty: &ObjectType) -> Self {
        let mut roles = Roles::default();
        if ty.harvester || ty.resource_gatherer {
            roles = roles.with(Roles::HARVESTER);
        }
        if ty.engineer {
            roles = roles.with(Roles::ENGINEER);
        }
        if ty.agent || ty.infiltrate {
            roles = roles.with(Roles::AGENT);
        }
        if ty.c4 {
            roles = roles.with(Roles::SABOTEUR);
        }
        if ty.occupier {
            roles = roles.with(Roles::OCCUPIER);
        }
        if ty.passengers > 0 {
            roles = roles.with(Roles::TRANSPORT);
        }
        // A maker carries `DeploysInto=`; a deployment-form unit carries
        // `UndeploysInto=` or `Deployer=`. All three mean "this unit becomes
        // something else where it stands".
        if ty.deployer || ty.deploys_into.is_some() || ty.undeploys_into.is_some() {
            roles = roles.with(Roles::DEPLOYER);
        }
        if ty.refinery {
            roles = roles.with(Roles::REFINERY);
        }
        if ty.category == ObjectCategory::Building {
            roles = roles.with(Roles::STRUCTURE);
        }
        match ty.factory_type() {
            Some(vera20k_core::rules::object_type::FactoryType::InfantryType) => {
                roles = roles.with(Roles::TRAINS_INFANTRY);
            }
            Some(vera20k_core::rules::object_type::FactoryType::UnitType) => {
                roles = roles.with(Roles::BUILDS_VEHICLES);
            }
            _ => {}
        }

        let mut weapons = Vec::new();
        for index in 0..ty.weapon_record_count() {
            let Some(name) = ty.weapon_at(index) else {
                continue;
            };
            if name.is_empty() || name.eq_ignore_ascii_case("none") {
                continue;
            }
            let Some(weapon) = rules.weapon(name) else {
                continue;
            };
            let mut dps_permille = [0_i64; ARMOR_SLOTS];
            let verses = rules
                .warhead(weapon.warhead.as_deref().unwrap_or(""))
                .map(|warhead| warhead.verses_f64)
                .unwrap_or([1.0; ARMOR_SLOTS]);
            let rof = weapon.rof.max(1) as i64;
            let burst = weapon.burst.max(1) as i64;
            for (slot, multiplier) in verses.iter().enumerate().take(ARMOR_SLOTS) {
                let per_mille = (multiplier * 1000.0).round() as i64;
                dps_permille[slot] =
                    i64::from(weapon.damage) * burst * FRAMES_PER_SECOND * per_mille / rof / 1000;
            }
            let splash_cells = rules
                .warhead(weapon.warhead.as_deref().unwrap_or(""))
                .map(|warhead| warhead.cell_spread_f64.round() as i32)
                .unwrap_or(0);
            weapons.push(WeaponProfile {
                id: name.to_owned(),
                damage: weapon.damage,
                rof: weapon.rof,
                burst: weapon.burst,
                range_cells: weapon.range_leptons / 256,
                dps_permille,
                splash_cells,
            });
        }
        if weapons.iter().any(|weapon| weapon.best_dps() > 0) {
            roles = roles.with(Roles::ARMED);
        }

        Self {
            id: ty.id.clone(),
            category: ty.category,
            cost: ty.cost,
            strength: ty.strength,
            armor: vera20k_core::rules::armor::armor_index(&ty.armor).index(),
            speed: ty.speed,
            sight: ty.sight,
            roles,
            weapons,
            passengers: ty.passengers,
            ammo: ty.ammo,
            reload: ty.reload,
            power: ty.power,
            build_limit: ty.build_limit,
        }
    }

    /// The weapon the policy would use against that armor class: the one with
    /// the highest damage per second that can hurt it.
    pub fn best_weapon_against(&self, armor: usize) -> Option<&WeaponProfile> {
        self.weapons
            .iter()
            .filter(|weapon| weapon.can_hurt(armor))
            .max_by_key(|weapon| weapon.dps_permille[armor])
    }

    /// Whether this unit can hurt an object with that armor at all.
    pub fn can_hurt(&self, armor: usize) -> bool {
        self.best_weapon_against(armor).is_some()
    }
}

/// Every type the rules describe, by id.
#[derive(Debug, Clone, Default)]
pub struct Profiles {
    by_id: BTreeMap<String, UnitProfile>,
}

impl Profiles {
    pub fn build(rules: &RuleSet) -> Self {
        let mut by_id = BTreeMap::new();
        for ty in rules.all_objects() {
            by_id.insert(ty.id.clone(), UnitProfile::of(rules, ty));
        }
        Self { by_id }
    }

    pub fn get(&self, id: &str) -> Option<&UnitProfile> {
        self.by_id.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &UnitProfile> {
        self.by_id.values()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The whole table as JSON, for review. Hand-written so the crate needs no
    /// serialization dependency for a diagnostic.
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\n");
        for (index, profile) in self.by_id.values().enumerate() {
            if index > 0 {
                out.push_str(",\n");
            }
            out.push_str(&profile.to_json());
        }
        out.push_str("\n}\n");
        out
    }
}

impl UnitProfile {
    fn to_json(&self) -> String {
        let roles: Vec<String> = self
            .roles
            .names()
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect();
        let weapons: Vec<String> = self
            .weapons
            .iter()
            .map(|weapon| {
                let dps: Vec<String> = weapon
                    .dps_permille
                    .iter()
                    .map(|value| value.to_string())
                    .collect();
                format!(
                    "{{\"id\":\"{}\",\"damage\":{},\"rof\":{},\"burst\":{},\"range_cells\":{},\"splash_cells\":{},\"dps_permille\":[{}]}}",
                    weapon.id,
                    weapon.damage,
                    weapon.rof,
                    weapon.burst,
                    weapon.range_cells,
                    weapon.splash_cells,
                    dps.join(",")
                )
            })
            .collect();
        format!(
            "  \"{}\": {{\"category\":\"{}\",\"cost\":{},\"strength\":{},\"armor\":{},\"speed\":{},\"sight\":{},\"passengers\":{},\"ammo\":{},\"reload\":{},\"power\":{},\"build_limit\":{},\"roles\":[{}],\"weapons\":[{}]}}",
            self.id,
            category_name(self.category),
            self.cost,
            self.strength,
            self.armor,
            self.speed,
            self.sight,
            self.passengers,
            self.ammo,
            self.reload,
            self.power,
            self.build_limit,
            roles.join(","),
            weapons.join(",")
        )
    }
}

fn category_name(category: ObjectCategory) -> &'static str {
    match category {
        ObjectCategory::Infantry => "infantry",
        ObjectCategory::Vehicle => "vehicle",
        ObjectCategory::Aircraft => "aircraft",
        ObjectCategory::Building => "building",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retail() -> Option<RuleSet> {
        Some(vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules)
    }

    #[test]
    fn every_type_gets_a_profile() {
        let Some(rules) = retail() else {
            return;
        };
        let profiles = Profiles::build(&rules);
        assert!(profiles.len() > 100, "the rules describe few types");
        for profile in profiles.iter() {
            assert!(!profile.id.is_empty());
        }
    }

    #[test]
    fn a_tank_is_armed_and_its_weapon_hurts_armor() {
        let Some(rules) = retail() else {
            return;
        };
        let profiles = Profiles::build(&rules);
        let Some(grizzly) = profiles.get("MTNK") else {
            return;
        };
        assert!(grizzly.roles.contains(Roles::ARMED), "a tank is armed");
        assert!(
            grizzly.can_hurt(vera20k_core::rules::armor::armor_index("heavy").index()),
            "a tank's gun hurts heavy armor"
        );
        assert!(grizzly.cost > 0);
    }

    #[test]
    fn a_harvester_is_not_a_fighter_and_a_transport_carries() {
        let Some(rules) = retail() else {
            return;
        };
        let profiles = Profiles::build(&rules);
        if let Some(miner) = profiles.get("HARV") {
            assert!(miner.roles.contains(Roles::HARVESTER));
        }
        if let Some(ifv) = profiles.get("FV") {
            assert!(ifv.roles.contains(Roles::TRANSPORT), "an IFV carries one");
            assert!(ifv.passengers > 0);
        }
    }

    #[test]
    fn an_armor_class_the_warhead_cannot_touch_is_not_hurt() {
        let Some(rules) = retail() else {
            return;
        };
        let profiles = Profiles::build(&rules);
        let Some(grizzly) = profiles.get("MTNK") else {
            return;
        };
        // The verses table is the authority: every slot either has damage or
        // does not, and the predicate must agree with it.
        for armor in 0..ARMOR_SLOTS {
            let expected = grizzly
                .weapons
                .iter()
                .any(|weapon| weapon.dps_permille[armor] > 0);
            assert_eq!(grizzly.can_hurt(armor), expected);
        }
    }
}
