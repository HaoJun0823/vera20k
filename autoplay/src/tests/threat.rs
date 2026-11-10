//! Threat assessment: fight, break off, or hold.
//!
//! Every scene starts from a real `Snapshot::take` and then edits only the fields the
//! case is about, so the rule-derived facts come from the production reader rather than
//! a hand-built second copy of the rules. Distances and ranges are integers.

use vera20k_core::map::entities::EntityCategory;
use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::threat::{Verdict, assess};
use crate::world::Snapshot;

const BOUNDS: (u16, u16) = (32, 32);

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0xBEEF);
    vera20k_core::sim::arena_fixture::flat_ground(&mut sim, &rules);
    sim.intern_rule_type_ids(&rules);
    sim.resolve_type_handles(&rules);
    for (name, human) in [("Americans", true), ("Russians", false)] {
        let owner = sim.intern(name);
        let side = rules.country_side_index(name)?.0;
        let mut house = HouseState::new(owner, side, Some(owner), human, 0, 10);
        house.project_country_mults(&rules, sim.interner());
        sim.houses_mut_for_test().register(house);
    }
    let ours = sim.spawn_object("MTNK", "Americans", 8, 8, 0, &rules)?;
    let theirs = sim.spawn_object("MTNK", "Russians", 20, 20, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    Some((sim, rules, americans, ours, theirs))
}

/// A revealed enemy, our unit healthy and able to hit ground targets.
fn baseline(
    sim: &Simulation,
    rules: &RuleSet,
    americans: InternedId,
    ours: u64,
    theirs: u64,
) -> Snapshot {
    let mut snapshot = Snapshot::take(sim, rules, americans);
    let own = snapshot.objects.get_mut(&ours).expect("our tank exists");
    own.range_cells = Some(5);
    own.can_target_ground = true;
    own.can_target_air = false;
    own.health = own.strength;
    let enemy = snapshot.objects.get_mut(&theirs).expect("enemy exists");
    enemy.visible = true;
    enemy.range_cells = Some(5);
    enemy.in_playfield = true;
    snapshot
}

/// Even odds and full health: engage the nearest visible enemy.
#[test]
fn an_advantaged_unit_attacks() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let snapshot = baseline(&sim, &rules, americans, ours, theirs);
    assert_eq!(assess(&snapshot, ours, BOUNDS), Verdict::Engage(theirs));
}

/// A longer-ranged enemy that can reach us while we cannot reach it: break off.
#[test]
fn an_out_ranged_unit_retreats() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let mut snapshot = baseline(&sim, &rules, americans, ours, theirs);
    // 24 cells away, its 30-cell gun covers us, our 5-cell gun does not.
    snapshot
        .objects
        .get_mut(&theirs)
        .expect("enemy exists")
        .range_cells = Some(30);
    let verdict = assess(&snapshot, ours, BOUNDS);
    assert!(
        matches!(verdict, Verdict::Retreat(_)),
        "expected a retreat, got {verdict:?}"
    );
    // The fall-back cell must be inside the playfield and away from the enemy.
    if let Verdict::Retreat(cell) = verdict {
        assert!(cell.0 < BOUNDS.0 && cell.1 < BOUNDS.1);
        assert!(cell.0 < 8, "moved away along the dominant x axis");
    }
}

/// Already hurt and inside the enemy's reach: break off.
#[test]
fn a_hurt_unit_in_reach_retreats() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let mut snapshot = baseline(&sim, &rules, americans, ours, theirs);
    let own = snapshot.objects.get_mut(&ours).expect("our tank exists");
    own.health = own.strength / 4;
    let enemy = snapshot.objects.get_mut(&theirs).expect("enemy exists");
    enemy.cell = (10, 8);
    enemy.range_cells = Some(10);
    assert!(matches!(
        assess(&snapshot, ours, BOUNDS),
        Verdict::Retreat(_)
    ));
}

/// Pinned in a corner with the threat right beside us: fighting on beats freezing.
#[test]
fn a_unit_with_nowhere_to_run_still_attacks() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let mut snapshot = baseline(&sim, &rules, americans, ours, theirs);
    let own = snapshot.objects.get_mut(&ours).expect("our tank exists");
    own.cell = (0, 0);
    own.health = own.strength / 4;
    let enemy = snapshot.objects.get_mut(&theirs).expect("enemy exists");
    enemy.cell = (1, 0);
    enemy.range_cells = Some(10);
    // Falling back along -x clamps to x=0, which is where the unit already stands.
    assert_eq!(assess(&snapshot, ours, BOUNDS), Verdict::Engage(theirs));
}

/// An enemy the player cannot see is not a target, and does not cause a retreat.
#[test]
fn an_unseen_enemy_never_affects_the_verdict() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let mut snapshot = baseline(&sim, &rules, americans, ours, theirs);
    snapshot
        .objects
        .get_mut(&theirs)
        .expect("enemy exists")
        .visible = false;
    assert_eq!(assess(&snapshot, ours, BOUNDS), Verdict::Hold);
}

/// A cannon-only unit never chases an aircraft it cannot hit (P09).
#[test]
fn a_unit_without_anti_air_does_not_chase_aircraft() {
    let Some((sim, rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let mut snapshot = baseline(&sim, &rules, americans, ours, theirs);
    let enemy = snapshot.objects.get_mut(&theirs).expect("enemy exists");
    enemy.category = EntityCategory::Aircraft;
    enemy.range_cells = Some(30);
    assert_eq!(assess(&snapshot, ours, BOUNDS), Verdict::Hold);
}
