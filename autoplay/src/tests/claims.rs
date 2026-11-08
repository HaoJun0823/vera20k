//! Command-source arbitration.
//!
//! The engine cannot help here: a `CommandEnvelope` carries an owner, a due frame and a
//! payload, and no field saying who issued it. So the ranking lives in this crate, and what
//! these tests pin is the rule the plan states absolutely - a player's order is never
//! covered - plus the cases where covering is legitimate.

use vera20k_core::sim::intern::InternedId;

use crate::claims::{Claims, Conceded, Source};

/// The rule the plan states without qualification.
#[test]
fn a_player_order_is_never_covered() {
    let mut claims = Claims::new();
    claims.claim(1, Source::Player);
    assert!(
        matches!(
            claims.may_write(1, Source::AutoPlay),
            Err(Conceded::Outranked {
                holder: Source::Player,
                asker: Source::AutoPlay
            })
        ),
        "a human click outranks this crate, always"
    );
    // And a script, which authors the map.
    assert!(matches!(
        claims.may_write(1, Source::Script),
        Err(Conceded::Outranked { .. })
    ));
}

/// A script-authored order outranks a policy's, because the map author's intent is content.
#[test]
fn a_script_order_outranks_autoplay() {
    let mut claims = Claims::new();
    claims.claim(2, Source::Script);
    assert!(claims.may_write(2, Source::AutoPlay).is_err());
    // But the player still outranks the script.
    assert!(claims.may_write(2, Source::Player).is_ok());
}

/// Changing its own mind is not a conflict: the same source may always write over itself.
#[test]
fn a_source_may_always_write_over_itself() {
    let mut claims = Claims::new();
    for source in [Source::AutoPlay, Source::Script, Source::Player] {
        claims.claim(3, source);
        assert!(
            claims.may_write(3, source).is_ok(),
            "{} changing its mind is not a conflict",
            source.describe()
        );
    }
}

/// An unclaimed unit is writable by anyone.
#[test]
fn an_unclaimed_unit_is_writable() {
    let claims = Claims::new();
    for unit in [4, 5, 6] {
        assert!(claims.may_write(unit, Source::AutoPlay).is_ok());
        assert!(claims.may_write(unit, Source::Player).is_ok());
    }
}

/// Release is what makes a handover possible: a claim nobody holds must not block.
#[test]
fn releasing_a_claim_lets_a_lower_source_write() {
    let mut claims = Claims::new();
    claims.claim(7, Source::Player);
    assert!(claims.may_write(7, Source::AutoPlay).is_err());

    claims.release(7);
    assert_eq!(claims.holder(7), None);
    assert!(
        claims.may_write(7, Source::AutoPlay).is_ok(),
        "the player gave it up, so the unit is available again"
    );
}

/// A claim belongs to a match; a recycled id must not inherit one.
#[test]
fn a_forgotten_unit_does_not_keep_its_claim() {
    let mut claims = Claims::new();
    claims.claim(8, Source::Player);
    claims.forget(8);
    assert_eq!(claims.holder(8), None);
    assert!(claims.may_write(8, Source::AutoPlay).is_ok());
}

/// A new match starts with no claims, for the same reason the dispatcher does.
#[test]
fn a_new_match_holds_no_claims() {
    let mut first = Claims::new();
    first.claim(9, Source::Player);
    first.note_conceded();
    assert_eq!(first.holder(9), Some(Source::Player));
    assert_eq!(first.conceded(), 1);

    let second = Claims::new();
    assert_eq!(second.holder(9), None);
    assert_eq!(second.conceded(), 0);
}

/// Retrying does not climb the ranking: a refused source stays refused while a better claim
/// holds the unit.
#[test]
fn a_refused_source_does_not_climb_by_retrying() {
    let mut claims = Claims::new();
    claims.claim(10, Source::Player);
    for _ in 0..5 {
        assert!(
            claims.may_write(10, Source::AutoPlay).is_err(),
            "repeatedly asking does not make the claim weaker"
        );
        claims.note_conceded();
    }
    assert_eq!(claims.conceded(), 5, "and each refusal is counted");
    assert_eq!(
        claims.holder(10),
        Some(Source::Player),
        "the holder is unchanged"
    );
}

/// The refusal names both sides, so a log can say who beat whom.
#[test]
fn a_refusal_names_both_sources() {
    let owner = InternedId::from_index(3);
    let house = InternedId::from_index(4);
    assert!(
        Conceded::Outranked {
            holder: Source::Player,
            asker: Source::AutoPlay,
        }
        .describe()
        .contains("player")
    );
    let not_ours = Conceded::NotThisHouse { owner, house }.describe();
    assert!(not_ours.contains("belongs to"), "{not_ours}");
}

/// The ranking itself, stated once so a reordering is a deliberate edit rather than an
/// accident.
#[test]
fn the_ranking_is_player_then_script_then_autoplay() {
    assert!(Source::Player > Source::Script);
    assert!(Source::Script > Source::AutoPlay);
}
