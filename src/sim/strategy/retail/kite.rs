//! Fire and move: stepping out of a shorter-ranged enemy's reach while the gun
//! reloads, and letting the next shot bring the unit back in.
//!
//! This is the half of a kite that the existing micro pass does not do. That
//! pass decides *what to shoot* and hands out an `Attack`; a `Attack` order
//! holds the ground the unit stands on, so a unit whose weapon out-ranges its
//! target still trades shots at the enemy's distance. The reach difference is
//! the whole value of a V3, a Prism tank or a sniper, and spending it needs the
//! unit to be somewhere else while it is reloading.
//!
//! The cycle is one frame's arithmetic, not a state machine:
//!
//! - the unit is shooting at something (`Observation::attack_target`, the
//!   native choice — no second opinion here);
//! - its own reach beats the target's by more than one cell, so there is a band
//!   outside the target's reach that is still inside its own;
//! - the gun is **not** ready, and the unit is inside the target's reach —
//!   that is the moment a shot is coming and nothing is going out;
//! - so it takes one step directly away. A step is one cell, and it is taken
//!   again next frame while the reload lasts and the target does not close.
//!
//! When the reload finishes the pass stops asking, the `Attack` intent from the
//! micro pass takes the unit back into range, and the cycle repeats. Nothing
//! here remembers the band or the cycle: readiness and distance are both read
//! from the world every frame, which is what makes it survive a target that
//! dies, a weapon that never reloads, and a unit that is re-tasked mid-cycle.

use crate::rules::object_type::ObjectCategory;
use crate::rules::ruleset::RuleSet;
use crate::sim::control::Goal;
use crate::sim::control::brain::Outbox;
use crate::sim::control::observation::Observation;

use super::RetailBrain;
use super::tracked::TrackedKind;

/// How much further than the target's reach the unit's own has to be for the
/// band to exist: one cell, so the step out lands *outside* the enemy's reach
/// rather than on its edge.
pub(super) const BAND_MARGIN_CELLS: i32 = 1;

/// One step away from `from` along the line to `target`, when the ground there
/// can be walked on.
///
/// A step, not a destination: the pass runs again next frame, and a unit that
/// is walked to a computed cell at the band's edge teleports its own decision
/// out of the loop. The chosen cell maximises the distance to the target
/// (Chebyshev, ties by the fixed order) so a diagonal step is taken when it is
/// the one that leaves the reach fastest.
pub(super) fn step_away(from: (u16, u16), target: (u16, u16)) -> (u16, u16) {
    let mut best = from;
    let mut best_distance = chebyshev(from, target);
    for (dx, dy) in [
        (0_i32, -1_i32),
        (-1, 0),
        (1, 0),
        (0, 1),
        (-1, -1),
        (1, -1),
        (-1, 1),
        (1, 1),
    ] {
        let (x, y) = (i32::from(from.0) + dx, i32::from(from.1) + dy);
        if x < 0 || y < 0 {
            continue;
        }
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
            continue;
        };
        let distance = chebyshev((x, y), target);
        if distance > best_distance {
            best = (x, y);
            best_distance = distance;
        }
    }
    best
}

fn chebyshev(a: (u16, u16), b: (u16, u16)) -> i32 {
    i32::from(a.0)
        .abs_diff(i32::from(b.0))
        .max(i32::from(a.1).abs_diff(i32::from(b.1))) as i32
}

/// Whether a unit with `mine` reach has a band to fight from against `theirs`.
pub(super) fn out_ranges(mine: i32, theirs: i32) -> bool {
    mine > theirs + BAND_MARGIN_CELLS
}

/// Step out of reach while the gun reloads.
pub(super) fn run(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    for unit in &brain.census.units {
        if !unit.traits.combat || unit.hand || !unit.orderable {
            continue;
        }
        if obs.is_deployed(unit.entity_id) {
            continue;
        }
        if super::orders::ordered_this_frame(out, unit.entity_id) {
            continue;
        }
        let Some(target) = obs.attack_target(unit.entity_id) else {
            continue;
        };
        let Some(enemy) = brain
            .census
            .enemies
            .iter()
            .find(|enemy| enemy.entity_id == target)
        else {
            continue;
        };
        let Some(mine) = obs
            .object_type(rules, unit.type_id)
            .filter(|ty| ty.category != ObjectCategory::Aircraft)
            .map(|ty| super::advance::weapon_range_cells(rules, ty, false))
        else {
            continue;
        };
        let Some(theirs) = obs
            .object_type(rules, enemy.type_id)
            .map(|ty| super::advance::weapon_range_cells(rules, ty, false))
        else {
            continue;
        };
        if !out_ranges(mine, theirs) {
            continue;
        }
        // Ready: the shot is the micro pass's, and this pass has nothing to say.
        if obs.weapon_ready(unit.entity_id) {
            continue;
        }
        // Outside the target's reach already: it can wait out the reload where
        // it stands.
        let distance = chebyshev(unit.cell, enemy.cell);
        if distance > theirs {
            continue;
        }
        let step = step_away(unit.cell, enemy.cell);
        if step == unit.cell || !obs.is_ground_walkable(step.0, step.1) {
            continue;
        }
        if !brain
            .tracked
            .admits(tick, unit.entity_id, TrackedKind::Kite, None, Some(step))
        {
            continue;
        }
        brain
            .tracked
            .record(tick, unit.entity_id, TrackedKind::Kite, None, Some(step));
        out.order(unit.entity_id, Goal::Move { cell: step });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A step away from the target is taken, and it is the direction that
    /// leaves the target's reach fastest: away is the straight one, and a
    /// diagonal target is answered with a straight step out of its corner,
    /// which is the same Chebyshev gain.
    #[test]
    fn the_step_goes_away_from_the_target() {
        assert_eq!(step_away((10, 10), (10, 4)), (10, 11));
        assert_eq!(step_away((10, 10), (16, 10)), (9, 10));
        let diagonal = step_away((10, 10), (4, 4));
        assert!(
            chebyshev(diagonal, (4, 4)) > chebyshev((10, 10), (4, 4)),
            "{diagonal:?} is further out than where it stood"
        );
    }

    /// The band exists only when the reach difference is more than the step:
    /// a one-cell edge is the enemy's own reach as well.
    #[test]
    fn a_band_needs_more_than_one_cell_of_reach() {
        assert!(!out_ranges(8, 8));
        assert!(!out_ranges(9, 8), "one cell is not a band");
        assert!(out_ranges(10, 8));
        assert!(!out_ranges(8, 9), "out-ranged units do not kite");
    }

    /// A unit already outside the target's reach has nothing to step away
    /// from; the check is the caller's, so this fixes the arithmetic it uses.
    #[test]
    fn distance_is_chebyshev() {
        assert_eq!(chebyshev((10, 10), (13, 11)), 3);
        assert_eq!(chebyshev((10, 10), (10, 10)), 0);
    }
}
