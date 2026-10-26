//! Sending the army out.
//!
//! Native carries the attack in teams: an `AITrigger` creates one and its
//! script assigns the missions. A brain-driven house never gets an `AITrigger`
//! — `HouseClass::Update`'s team block is gated on "no human controls it" —
//! so without this module its army sits on area guard for the whole match and
//! the house can only lose by attrition.
//!
//! This is the player's substitute, and deliberately two rules. The first is
//! *when*: an army large enough to be worth spending, a base that can replace
//! it, and nothing already at the yard. The second is *where*: one structure
//! for the whole army — the enemy economy structure nearest the first fighter
//! in entity-store order — because a fist takes a base, and units that walk
//! at their own nearest building arrive one at a time and die one at a time.
//!
//! One goal for the army is a measurement, not a preference. Three variants
//! were run end to end against the pinned `Death.mmx` match and all three
//! lost the push the current rule wins: aiming each unit at the structure its
//! own weapons answer best, aiming the fist from the army's centre instead of
//! that first fighter, and holding one target until it is destroyed instead
//! of re-reading the nearest one each frame (`super::RESIDUALS` has the
//! ticks). So the *target* stays one, and what differs per unit is the
//! **order**: a unit already inside its own weapon's reach of that structure
//! attacks it from where it stands, and one further out attack-moves toward
//! it. That keeps the fist together — the failure the three variants shared
//! was dispersion, not per-unit decisions — while stopping the army from
//! walking its short-ranged units point-blank into a base to shoot at all.
//!
//! While it is pushing it owns the whole army, so the area-guard step stands
//! down for the frame; a house that has not got an army yet keeps holding its
//! base, which is what a player does with five tanks and no plan.

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::control::Goal;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::{EnemyEntity, Observation};
use crate::sim::mission::{MissionId, MissionType};

use super::census::{Census, Unit};
use super::RetailBrain;

/// How large the army has to be before it is worth pushing with: the teams'
/// own number ([`super::squads::ARMY_TO_ATTACK`]), because "how big is big
/// enough" is one question with one owner.
///
/// Below it the opening force is the whole army, and an opening force that
/// walks into the enemy's pre-placed garrison is a house that has traded its
/// base for nothing — which is why the surplus goes harassing instead.

/// How many units one frame may re-task. The board's per-frame budget drops
/// everything past its own limit, so a larger number here would only spend the
/// frame on orders that never leave.
const ADVANCE_ORDERS_PER_FRAME: usize = 8;

/// How close an enemy has to come to the yard before the army stays home,
/// in cells. A raid inside this ring is what the army was bought for.
const THREAT_RADIUS: i32 = 18;

/// Send the army at the enemy's base when it is large enough. `false` when
/// the house is not pushing this frame, so the caller keeps its own posture.
pub(super) fn advance(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) -> bool {
    if !brain.census.units.iter().any(|unit| unit.traits.combat) {
        return false;
    }
    // An army the base cannot replace is not an army, it is the whole house
    // walking out of the gate; and a base with nothing to hold it is a base
    // that falls while the army is away. The third gate is the one that
    // decides matches: an enemy already at the yard is what the army is for,
    // and marching out with it in the base loses the production the army
    // came from.
    let fighters: Vec<&Unit> = brain
        .census
        .combat_units()
        .filter(|unit| !unit.hand)
        .collect();
    if fighters.len() < super::squads::ARMY_TO_ATTACK
        || !super::build::can_replace_units(rules, obs, &brain.census)
        || !super::build::base_defended(rules, obs, &brain.census)
        || base_under_threat(&brain.census)
    {
        return false;
    }
    // Structures, not units: a base is what a house loses with, and a
    // defending unit walks into the push on its own. Refineries first — a
    // house that loses its unloading bays loses the war it is paying for,
    // and the opponent's wallet is the one number that decides the late game.
    let structures: Vec<(&ObjectType, EnemyEntity)> = brain
        .census
        .enemies
        .iter()
        .filter_map(|enemy| {
            obs.object_type(rules, enemy.type_id)
                .filter(|ty| ty.category == ObjectCategory::Building)
                .map(|ty| (ty, *enemy))
        })
        .collect();
    let economy: Vec<EnemyEntity> = structures
        .iter()
        .filter(|(ty, _)| ty.resource_destination)
        .map(|(_, enemy)| *enemy)
        .collect();
    let targets: Vec<EnemyEntity> = if economy.is_empty() {
        structures.iter().map(|(_, enemy)| *enemy).collect()
    } else {
        economy
    };
    if targets.is_empty() {
        return false;
    }

    // One target for the whole army, not the nearest for each unit: units
    // pointed at their own nearest structure arrive one at a time and are
    // killed one at a time. A fist is what takes a base.
    let anchor = fighters.first().map_or((0, 0), |unit| unit.cell);
    let Some(target) = nearest_target(&targets, anchor) else {
        return false;
    };

    let tick = obs.tick();
    let mut issued = 0;
    for unit in &fighters {
        if issued >= ADVANCE_ORDERS_PER_FRAME {
            break;
        }
        // A unit already fighting or walking is doing what this would ask.
        if matches!(
            unit.mission,
            mission if mission == MissionId::from_known(MissionType::Attack)
                || mission == MissionId::from_known(MissionType::Move)
        ) {
            continue;
        }
        // The step that owns a specialist order already spent this unit's frame:
        // a spy walking into a building and a rifleman boarding a carrier both
        // outrank the push.
        if out.orders.iter().any(|order| order.actor == unit.entity_id) {
            continue;
        }
        // The micro layer has this unit fighting something in reach, and the
        // fight outranks the march. Its mission cannot say so yet — the order
        // was issued this frame — so the intent table is what is asked.
        if brain
            .tracked
            .stands(tick, unit.entity_id, &[super::tracked::TrackedKind::Engage])
        {
            continue;
        }
        // Inside its own reach the unit shoots the structure from where it
        // stands; outside it, it walks. The goal is still the one target — this
        // is the same fist, ordered by what each unit can actually do.
        let reach = obs
            .object_type(rules, unit.type_id)
            .map_or(0, |ty| weapon_range_cells(rules, ty));
        let (kind, goal) = if reach > 0 && cell_distance(unit.cell, target.cell) <= reach {
            (
                super::tracked::TrackedKind::Attack,
                Goal::Attack {
                    target: target.entity_id,
                },
            )
        } else {
            (
                super::tracked::TrackedKind::AttackMove,
                Goal::AttackMove { cell: target.cell },
            )
        };
        // L4: it has been sent at this target, the target has not walked away,
        // and the order has not expired — so it is already doing this.
        if !brain.tracked.admits(
            tick,
            unit.entity_id,
            kind,
            Some(target.entity_id),
            Some(target.cell),
        ) {
            continue;
        }
        brain.tracked.record(
            tick,
            unit.entity_id,
            kind,
            Some(target.entity_id),
            Some(target.cell),
        );
        out.order(unit.entity_id, goal);
        issued += 1;
    }
    true
}

/// The reach of a type's primary weapon in cells, rounded up; zero when it has
/// none.
pub(super) fn weapon_range_cells(rules: &RuleSet, ty: &ObjectType) -> i32 {
    let Some(name) = ty.primary() else {
        return 0;
    };
    let Some(weapon) = rules.weapon(name) else {
        return 0;
    };
    let per_cell = crate::util::lepton::LEPTONS_PER_CELL_I32;
    let reach = (weapon.range_leptons - weapon.minimum_range_leptons.max(0)).max(0);
    reach.div_euclid(per_cell) + i32::from(reach.rem_euclid(per_cell) != 0)
}

/// Chebyshev distance in cells, the metric the game's own adjacency uses.
fn cell_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    let dx = i32::from(a.0).abs_diff(i32::from(b.0));
    let dy = i32::from(a.1).abs_diff(i32::from(b.1));
    i32::try_from(dx.max(dy)).unwrap_or(i32::MAX)
}

/// Whether an enemy is close enough to what this house still holds that the
/// army belongs at home.
///
/// Measured from the yard when one stands, and otherwise from whatever
/// building the house still has: a house whose yard is gone but whose factory
/// still stands has plenty left to lose, and an army that walks out while the
/// enemy is standing on the factory loses that too.
///
/// A house with nothing standing is the other way round. It used to count as
/// under threat and held the army home for the rest of the match, which is
/// the residual the module header records: once the yard is gone the army is
/// the only thing the house has, and an army guarding rubble loses slowly
/// instead of taking the base that would let it rebuild. Nothing standing
/// means nothing to defend, so the push runs.
fn base_under_threat(census: &Census) -> bool {
    let Some(base) = census.base_cell else {
        return false;
    };
    census.enemies.iter().any(|enemy| {
        let dx = i32::from(enemy.cell.0) - i32::from(base.0);
        let dy = i32::from(enemy.cell.1) - i32::from(base.1);
        dx * dx + dy * dy <= THREAT_RADIUS * THREAT_RADIUS
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::unit_role::RoleTable;
    use crate::sim::world::Simulation;

    /// A retail arena: two houses on 32x32 ground, needed to spawn real object
    /// types. The rules come from the repository's own `ini/` snapshot, so
    /// these run without a retail install.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    fn ledger(rules: &RuleSet, sim: &Simulation, owner: crate::sim::intern::InternedId) -> Census {
        let mut census = Census::default();
        census.refresh(&RoleTable::build(rules), rules, &Observation::new(sim, owner));
        census
    }

    /// The whole reason the rule was narrowed: a house whose yard is gone and
    /// whose base is rubble used to read as under threat for the rest of the
    /// match, and an army guarding rubble loses slowly instead of taking the
    /// base that would let the house rebuild.
    #[test]
    fn a_house_with_nothing_left_to_defend_is_not_under_threat() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        assert!(
            !base_under_threat(&ledger(&rules, &sim, owner)),
            "nothing standing means nothing to defend"
        );
    }

    /// The same rule with something still standing, measured from the yard: an
    /// enemy inside the ring holds the army at home.
    #[test]
    fn an_enemy_at_the_yard_still_holds_the_army_home() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 16, 16, 255, &rules)
            .expect("an Allied Construction Yard");
        assert!(
            !base_under_threat(&ledger(&rules, &sim, owner)),
            "an empty field around the yard is no threat"
        );

        sim.spawn_object("MTNK", "Russians", 20, 16, 255, &rules)
            .expect("a Soviet tank");
        assert!(
            base_under_threat(&ledger(&rules, &sim, owner)),
            "four cells from the yard is inside {THREAT_RADIUS}"
        );
    }

    /// A unit inside its own weapon's reach of the push's target attacks it
    /// from where it stands instead of walking into it: the same fist, ordered
    /// by what each unit can actually do.
    #[test]
    fn a_unit_in_reach_attacks_the_fist_target_from_where_it_stands() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("GACNST", "Americans", 8, 8, 255, &rules)
            .expect("a yard");
        sim.spawn_object("GAPILE", "Americans", 14, 8, 255, &rules)
            .expect("a barracks");
        sim.spawn_object("GAWEAP", "Americans", 18, 8, 255, &rules)
            .expect("a war factory");
        sim.spawn_object("GAPILL", "Americans", 4, 8, 255, &rules)
            .expect("a pillbox so the base counts as defended");
        // The fist, close enough to the enemy refinery to shoot it.
        let mut fighters = Vec::new();
        for cell in [(24u16, 24u16), (25, 24), (26, 24), (24, 25), (25, 25)] {
            fighters.push(
                sim.spawn_object("MTNK", "Americans", cell.0, cell.1, 255, &rules)
                    .expect("a Grizzly"),
            );
        }
        for cell in [(24u16, 26u16), (25, 26), (26, 26), (24, 27), (25, 27)] {
            fighters.push(
                sim.spawn_object("MTNK", "Americans", cell.0, cell.1, 255, &rules)
                    .expect("a Grizzly"),
            );
        }
        sim.spawn_object("NAREFN", "Russians", 28, 26, 255, &rules)
            .expect("an enemy refinery");

        let mut brain = RetailBrain::new(0xA17);
        brain.census = ledger(&rules, &sim, owner);
        let obs = Observation::new(&sim, owner);
        let mut out = Outbox::default();
        assert!(
            advance(&mut brain, &rules, &obs, &mut out),
            "ten fighters and a defended base is a push"
        );
        assert!(
            out.orders
                .iter()
                .any(|order| order.actor == fighters[0] && matches!(order.goal, Goal::Attack { .. })),
            "the nearest fighter is inside its reach and shoots: {:?}",
            out.orders
        );
    }
}

/// The enemy structure nearest `from`, ties going to the earlier one in
/// storage order. Integer cells, so the choice is deterministic.
fn nearest_target(targets: &[EnemyEntity], from: (u16, u16)) -> Option<EnemyEntity> {
    let mut best: Option<(i32, EnemyEntity)> = None;
    for target in targets {
        let dx = i32::from(target.cell.0) - i32::from(from.0);
        let dy = i32::from(target.cell.1) - i32::from(from.1);
        let distance = dx * dx + dy * dy;
        if best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, *target));
        }
    }
    best.map(|(_, target)| target)
}
