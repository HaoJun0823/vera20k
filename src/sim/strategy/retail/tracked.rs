//! What each unit has already been told, and when it may be told again.
//!
//! Every other layer here answers "what should this unit do"; this one answers
//! "does it need telling". Those are different questions, and the second one
//! is where this brain was wrong: it re-decided for every unit every frame and
//! sent the answer again, so a command was issued once per frame for as long
//! as the decision stood.
//!
//! That is invisible for a command that sets a destination, and it is a bug
//! for the two kinds that are not: a **toggle** (the D key flips the pose, so a
//! second press undoes the first) and one whose **state lags the command** (a
//! deploying soldier still reads as standing until the animation finishes, so a
//! step that re-reads the state toggles it straight back up). Both were being
//! patched one at a time — a refresh timer here, a cooldown there — and the
//! garrison, which has neither, was re-issued every frame.
//!
//! One owner now holds the answer: an intent is recorded when it is issued,
//! and a layer that asks for the same one is refused until one of three things
//! is true —
//!
//! - **it expired** (each kind gets its own term: a deploy animation is short,
//!   a walk across the map is long);
//! - **the intent changed** (a different kind, or a different target);
//! - **the target moved** past [`REISSUE_CELLS`], which is the case an expiry
//!   alone cannot see: a unit walking at a target that has itself walked away
//!   is still holding a valid-looking intent.
//!
//! The third is why this is a record of *what was said* rather than a timer:
//! the world moves under the order, and only the order's own target knows when
//! it has moved far enough to be worth repeating.
//!
//! Determinism: `BTreeMap` keyed by the unit id, integer frame and integer
//! squared-distance comparisons only.

use std::collections::BTreeMap;

/// What a unit has been told to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrackedKind {
    /// Walk to a cell.
    Move,
    /// Attack one object.
    Attack,
    /// What is in reach *this second*, decided by the micro layer.
    ///
    /// Separate from [`Self::Attack`] because it has to be able to override a
    /// march and be overridden by one: its term is one micro cycle, so the
    /// fight is re-decided every cycle and a unit that has finished fighting
    /// is free again within one.
    Engage,
    /// Attack-move to a cell: engage what is on the way, keep going.
    AttackMove,
    /// Hold the ground it stands on and answer what comes.
    Guard,
    /// Enter a building: a garrison, or the hut whose span is down.
    Enter,
    /// Capture a building: the engineer's and the spy's click.
    Capture,
    /// The D key: take, or leave, the deployed pose.
    Deploy,
}

/// One micro cycle, in frames: a second at play cadence. The layer that
/// decides what a unit shoots *now* re-decides on this term, so it can both
/// override a march and hand the unit back to it quickly.
pub(super) const MICRO_FRAMES: u64 = 15;

impl TrackedKind {
    /// How long the intent stands before it may be issued again, in frames.
    ///
    /// Each is how long the object needs for the command to be *done* rather
    /// than merely issued: a pose takes its animation, walking into a building
    /// takes the walk. Something that holds ground has no natural end, so it
    /// gets the same term the brain used to refresh an order on.
    fn term(self) -> u64 {
        match self {
            Self::Deploy => 45,
            Self::Engage => MICRO_FRAMES,
            Self::Enter | Self::Capture => 300,
            Self::Move | Self::Attack | Self::AttackMove | Self::Guard => 450,
        }
    }
}

/// One unit's standing intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Tracked {
    pub(super) kind: TrackedKind,
    /// The object the intent names, when it names one.
    pub(super) target: Option<u64>,
    /// The cell the intent names, when it names one: the thing that can move
    /// out from under the order.
    pub(super) cell: Option<(u16, u16)>,
    /// The frame before which it is not issued again.
    pub(super) until: u64,
}

/// How far a target may move before an otherwise-valid intent is worth
/// repeating, in cells. Squared-distance comparisons below; two cells is a
/// target that has visibly walked away, not one that shuffled.
const REISSUE_CELLS: i32 = 2;

/// Every unit's standing intent, and the units a routine is busy with.
#[derive(Debug, Default)]
pub(super) struct Tracker {
    intents: BTreeMap<u64, Tracked>,
    /// Units a managed routine owns, and the frame it hands them back on. A
    /// routine is its object's only issuer, so the layers above ask here before
    /// they tell one anything.
    held: BTreeMap<u64, u64>,
}

impl Tracker {
    pub(super) fn new() -> Self {
        Self {
            intents: BTreeMap::new(),
            held: BTreeMap::new(),
        }
    }

    /// Whether `unit` may be told to do `kind` at `target` / `cell` now.
    ///
    /// True when it has not been told anything, when what it was told has
    /// expired, when the intent itself changed, or when its target has moved
    /// far enough that the order is stale.
    pub(super) fn admits(
        &self,
        now: u64,
        unit: u64,
        kind: TrackedKind,
        target: Option<u64>,
        cell: Option<(u16, u16)>,
    ) -> bool {
        if self.held.get(&unit).is_some_and(|until| now < *until) {
            return false;
        }
        let Some(tracked) = self.intents.get(&unit) else {
            return true;
        };
        if tracked.kind != kind || tracked.target != target {
            return true;
        }
        if now >= tracked.until {
            return true;
        }
        moved(tracked.cell, cell)
    }

    /// Whether anyone at all still has a live `kind` intent on `target`.
    ///
    /// The reverse of `admits`, for the orders that are about a *place* rather
    /// than about a unit: one building takes one engineer, and a second
    /// engineer sent at the same one walks to a building that is already being
    /// taken, arrives after it is taken, and stands there. Whoever asks is
    /// holding the answer, so the claim is recorded like any other intent and
    /// released by the same expiry.
    pub(super) fn claimed_by_other(
        &self,
        now: u64,
        kind: TrackedKind,
        target: u64,
        asker: u64,
    ) -> bool {
        self.intents.iter().any(|(owner, tracked)| {
            *owner != asker
                && tracked.kind == kind
                && tracked.target == Some(target)
                && now < tracked.until
        })
    }

    /// Drop an intent the world does not show.
    ///
    /// An order that was refused, that had no path, or that a later order
    /// cancelled leaves no trace in the simulation, and the intent table used
    /// to go on believing its own record for the intent's whole term: a Move
    /// that never took held a scout in the yard for 450 frames. That is the
    /// heart of the "it asks once and then assumes" behaviour — the brain had
    /// no feedback at all.
    ///
    /// Callers ask for this when the object's own state contradicts the intent,
    /// before [`Self::admits`], so a refused order is asked again next frame
    /// instead of on the term.
    pub(super) fn void(&mut self, unit: u64, kind: TrackedKind) {
        if self
            .intents
            .get(&unit)
            .is_some_and(|tracked| tracked.kind == kind)
        {
            self.intents.remove(&unit);
        }
    }

    /// Record that `unit` has been told to do `kind` at `frame`.
    pub(super) fn record(
        &mut self,
        now: u64,
        unit: u64,
        kind: TrackedKind,
        target: Option<u64>,
        cell: Option<(u16, u16)>,
    ) {
        self.intents.insert(
            unit,
            Tracked {
                kind,
                target,
                cell,
                until: now.saturating_add(kind.term()),
            },
        );
    }

    /// Drop the units the house no longer has, so the table stays the size of
    /// the army and a recycled id cannot inherit another unit's intent.
    pub(super) fn retain(&mut self, live: &std::collections::BTreeSet<u64>) {
        self.intents.retain(|id, _| live.contains(id));
        self.held.retain(|id, _| live.contains(id));
    }

    /// Whether `unit` is still holding an intent of one of `kinds`.
    ///
    /// A layer that wants to know whether another layer already owns the unit
    /// asks this rather than reading the record itself: "the advance step is
    /// pushing it" is a question about the intents above, not about this
    /// unit's mission.
    pub(super) fn stands(&self, now: u64, unit: u64, kinds: &[TrackedKind]) -> bool {
        self.intents
            .get(&unit)
            .is_some_and(|tracked| kinds.contains(&tracked.kind) && now < tracked.until)
    }

    /// Hand `unit` to a routine until `until`: the routine is its only issuer
    /// for those frames.
    pub(super) fn hold(&mut self, unit: u64, until: u64) {
        self.held.insert(unit, until);
    }

    /// Whether a routine owns `unit` right now.
    pub(super) fn held(&self, now: u64, unit: u64) -> bool {
        self.held.get(&unit).is_some_and(|until| now < *until)
    }
}

/// Whether the target has moved far enough for a standing order to be stale.
fn moved(was: Option<(u16, u16)>, now: Option<(u16, u16)>) -> bool {
    let (Some(was), Some(now)) = (was, now) else {
        return was != now;
    };
    let dx = i32::from(was.0) - i32::from(now.0);
    let dy = i32::from(was.1) - i32::from(now.1);
    dx * dx + dy * dy > REISSUE_CELLS * REISSUE_CELLS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toggle issued twice is the bug this layer exists for: the second
    /// press undoes the first, so the answer has to be no until the pose has
    /// had time to take.
    #[test]
    fn a_toggle_is_not_issued_twice() {
        let mut tracker = Tracker::new();
        assert!(tracker.admits(0, 7, TrackedKind::Deploy, None, None));
        tracker.record(0, 7, TrackedKind::Deploy, None, None);
        assert!(
            !tracker.admits(10, 7, TrackedKind::Deploy, None, None),
            "the pose is still being taken"
        );
        assert!(
            tracker.admits(60, 7, TrackedKind::Deploy, None, None),
            "and may be asked again once it has"
        );
    }

    /// An order is stale when its target walks away, even though nothing about
    /// the order itself has changed and it has not expired.
    #[test]
    fn an_order_is_repeated_when_its_target_moves() {
        let mut tracker = Tracker::new();
        let target = 9;
        tracker.record(0, 7, TrackedKind::Attack, Some(target), Some((20, 20)));
        assert!(
            !tracker.admits(10, 7, TrackedKind::Attack, Some(target), Some((21, 20))),
            "one cell is a shuffle, not a move"
        );
        assert!(
            tracker.admits(10, 7, TrackedKind::Attack, Some(target), Some((25, 20))),
            "five cells is the target walking away"
        );
    }

    /// A different intent is a new decision, not a repeat of the old one.
    #[test]
    fn a_different_intent_is_admitted_at_once() {
        let mut tracker = Tracker::new();
        tracker.record(0, 7, TrackedKind::Guard, None, None);
        assert!(tracker.admits(1, 7, TrackedKind::Attack, Some(9), None));
    }

    /// A unit a routine owns is not told anything by the layers above it.
    #[test]
    fn a_held_unit_is_left_to_its_routine() {
        let mut tracker = Tracker::new();
        tracker.hold(7, 100);
        assert!(!tracker.admits(10, 7, TrackedKind::Guard, None, None));
        assert!(tracker.admits(101, 7, TrackedKind::Guard, None, None));
    }
}
