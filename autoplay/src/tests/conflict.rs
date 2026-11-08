//! The whole conflict path, through the arbiter.
//!
//! The previous commit built the ranking and left it unconnected, which is a component with
//! tests and no callers. This is the connection: `admit_with_source` consults claims after
//! permission and ownership, and the ordering of those checks is itself something worth
//! pinning, because getting it wrong reports the wrong reason.

use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::command::Command;
use vera20k_core::sim::house_state::HouseState;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::world::Simulation;

use crate::arbiter::{Admission, Arbiter};
use crate::claims::{Claims, Conceded, Source};
use crate::control::{ControlState, Refusal, Subsystem};

fn scene() -> Option<(Simulation, RuleSet, InternedId, u64, u64)> {
    let rules = vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules;
    let mut sim = Simulation::with_seed(0xC1A1);
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
    let ours = sim.spawn_object("MTNK", "Americans", 40, 40, 0, &rules)?;
    let theirs = sim.spawn_object("MTNK", "Russians", 60, 60, 0, &rules)?;
    let americans = sim.interner().get("Americans")?;
    Some((sim, rules, americans, ours, theirs))
}

fn order(id: u64) -> Command {
    Command::AttackMove {
        entity_id: id,
        target: None,
        destination: Some(TargetKind::Cell(42, 40)),
    }
}

/// A unit nobody has claimed is writable, and the claim records the writer.
#[test]
fn an_unclaimed_unit_is_taken_and_the_claim_recorded() {
    let Some((sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut claims = Claims::new();
    let admission = Arbiter::admit_with_source(
        &sim,
        &control,
        Some(&claims),
        Source::AutoPlay,
        Subsystem::Combat,
        ours,
        order(ours),
    );
    assert!(admission.is_allowed());
    assert_eq!(
        claims.holder(ours),
        None,
        "nothing is recorded by the arbiter"
    );
    // The caller records the claim only once the order actually went out, so a refused one
    // cannot take the unit by failing loudly.
    claims.claim(ours, Source::AutoPlay);
    assert_eq!(claims.holder(ours), Some(Source::AutoPlay));
}

/// The case the plan states absolutely: a player takes the unit, and AutoPlay stands down.
#[test]
fn a_player_claim_makes_a_later_autoplay_order_concede() {
    let Some((sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut claims = Claims::new();

    // AutoPlay orders first, and takes it.
    assert!(
        Arbiter::admit_with_source(
            &sim,
            &control,
            Some(&claims),
            Source::AutoPlay,
            Subsystem::Combat,
            ours,
            order(ours),
        )
        .is_allowed()
    );
    claims.claim(ours, Source::AutoPlay);

    // The player clicks.
    assert!(
        Arbiter::admit_with_source(
            &sim,
            &control,
            Some(&claims),
            Source::Player,
            Subsystem::Combat,
            ours,
            order(ours),
        )
        .is_allowed(),
        "a player outranks AutoPlay and may always write"
    );
    claims.claim(ours, Source::Player);

    // AutoPlay tries again and must stand down, with the reason naming both sides.
    match Arbiter::admit_with_source(
        &sim,
        &control,
        Some(&claims),
        Source::AutoPlay,
        Subsystem::Combat,
        ours,
        order(ours),
    ) {
        Admission::Refused(Refusal::Conceded(Conceded::Outranked { holder, asker })) => {
            assert_eq!(holder, Source::Player);
            assert_eq!(asker, Source::AutoPlay);
        }
        other => panic!("expected an outranked refusal, got {other:?}"),
    }
    claims.note_conceded();
    assert_eq!(claims.conceded(), 1, "and the concession is counted");
}

/// If the player lets go, the unit becomes available again.
#[test]
fn a_released_unit_can_be_taken_again() {
    let Some((sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut claims = Claims::new();
    claims.claim(ours, Source::Player);
    assert!(
        Arbiter::admit_with_source(
            &sim,
            &control,
            Some(&claims),
            Source::AutoPlay,
            Subsystem::Combat,
            ours,
            order(ours),
        )
        .refusal()
        .is_some()
    );

    claims.release(ours);
    assert!(
        Arbiter::admit_with_source(
            &sim,
            &control,
            Some(&claims),
            Source::AutoPlay,
            Subsystem::Combat,
            ours,
            order(ours),
        )
        .is_allowed()
    );
}

/// The order of the checks, which decides which reason a caller sees.
///
/// Ownership is stronger than a competing claim: a foreign object is not in this player's
/// gift to concede or contest, so permission must be reported first. Getting this backwards
/// would have a planner conclude "somebody else wants this" about a unit it never owned.
#[test]
fn ownership_is_reported_before_a_competing_claim() {
    let Some((sim, _rules, americans, _, theirs)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    let mut claims = Claims::new();
    // Give the foreign unit a claim, so both refusals are available.
    claims.claim(theirs, Source::Player);

    match Arbiter::admit_with_source(
        &sim,
        &control,
        Some(&claims),
        Source::AutoPlay,
        Subsystem::Combat,
        theirs,
        order(theirs),
    ) {
        Admission::Refused(Refusal::NotOurObject { .. }) => {}
        other => panic!("ownership must be reported before a competing claim, got {other:?}"),
    }
}

/// A switched-off automated player does not even look at the world, so it reports that
/// rather than a claim conflict.
#[test]
fn the_permission_switch_is_reported_before_a_competing_claim() {
    let Some((sim, _rules, americans, ours, _)) = scene() else {
        return;
    };
    let control = ControlState::disabled(americans);
    let mut claims = Claims::new();
    claims.claim(ours, Source::Player);
    match Arbiter::admit_with_source(
        &sim,
        &control,
        Some(&claims),
        Source::AutoPlay,
        Subsystem::Combat,
        ours,
        order(ours),
    ) {
        Admission::Refused(Refusal::GlobalDisabled) => {}
        other => panic!("the switch is reported first, got {other:?}"),
    }
}

/// A caller with no claim state still gets permission and ownership, which is why `admit`
/// keeps its old signature and delegates.
#[test]
fn admission_without_claims_still_checks_permission_and_ownership() {
    let Some((sim, _rules, americans, ours, theirs)) = scene() else {
        return;
    };
    let control = ControlState::enabled(americans);
    assert!(Arbiter::admit(&sim, &control, Subsystem::Combat, ours, order(ours)).is_allowed());
    assert!(matches!(
        Arbiter::admit(&sim, &control, Subsystem::Combat, theirs, order(theirs)).refusal(),
        Some(Refusal::NotOurObject { .. })
    ));
    // And delegation is the same code path, not a parallel one: both report the same
    // ownership refusal for the same input.
    assert!(matches!(
        Arbiter::admit_with_source(
            &sim,
            &control,
            None,
            Source::AutoPlay,
            Subsystem::Combat,
            theirs,
            order(theirs),
        )
        .refusal(),
        Some(Refusal::NotOurObject { .. })
    ));
}
