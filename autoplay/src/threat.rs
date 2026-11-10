//! Should this unit fight, break off, or hold?
//!
//! This is the first tactical slice above "attack the nearest visible enemy". It uses
//! only facts the [`crate::world::Snapshot`] already carries: rule-derived weapon range
//! in cells, whether the primary weapon can hit air or ground, and the unit's current
//! health against its full strength (T00). Everything is integer arithmetic, so the
//! same world gives the same answer on every platform.
//!
//! What is deliberately not here yet: warhead-versus-armour damage, projectile flight
//! time, dodge, kiting and range-stalling. Those need more rule facts and come later.
//! An unseen enemy never enters the calculation - aiming at one would be cheating.
//!
//! A retreat is a real [`Command::Move`], not a renamed hold: the unit is told to put
//! distance between itself and the threat. When it has no cell to fall back to it keeps
//! fighting rather than freezing, which is the "fight to the end" case.

use std::collections::BTreeMap;

use vera20k_core::map::entities::EntityCategory;

use crate::world::{Object, Snapshot};

/// How many cells to put between the unit and the threat when breaking off.
pub const RETREAT_CELLS: i32 = 4;

/// One unit's tactical answer for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Attack this visible enemy.
    Engage(u64),
    /// Break off to this cell, inside the playfield.
    Retreat((u16, u16)),
    /// Nothing visible that this unit can hurt: hold the post.
    Hold,
}

/// Integer cell distance. A decision must not depend on floating-point behaviour.
fn manhattan(a: (u16, u16), b: (u16, u16)) -> i32 {
    (a.0 as i32 - b.0 as i32).abs() + (a.1 as i32 - b.1 as i32).abs()
}

/// Whether this unit's primary weapon can hurt a target of that category at all.
/// An anti-air-only weapon does not chase ground targets, and a cannon does not
/// chase aircraft. `Structure` counts as ground.
fn can_hurt(attacker: &Object, target: &Object) -> bool {
    match target.category {
        EntityCategory::Aircraft => attacker.can_target_air,
        _ => attacker.can_target_ground,
    }
}

/// A cell away from the threat, clamped to the map.
///
/// `None` means the unit is boxed in - the fall-back cell equals where it already
/// stands, or the threat is on the very same cell. That is the "nowhere to run" case.
fn retreat_cell(own: (u16, u16), threat: (u16, u16), bounds: (u16, u16)) -> Option<(u16, u16)> {
    let dx = own.0 as i32 - threat.0 as i32;
    let dy = own.1 as i32 - threat.1 as i32;
    if dx == 0 && dy == 0 {
        return None;
    }
    // Step along the dominant axis, away from the threat.
    let (step_x, step_y) = if dx.abs() >= dy.abs() {
        (dx.signum(), 0)
    } else {
        (0, dy.signum())
    };
    let max_x = bounds.0.saturating_sub(1) as i32;
    let max_y = bounds.1.saturating_sub(1) as i32;
    let next_x = (own.0 as i32 + step_x * RETREAT_CELLS).clamp(0, max_x);
    let next_y = (own.1 as i32 + step_y * RETREAT_CELLS).clamp(0, max_y);
    let cell = (next_x as u16, next_y as u16);
    (cell != own).then_some(cell)
}

/// Decide for one unit. `bounds` is `(map_width, map_height)`; a retreat cell is
/// always clamped inside it so the order is never addressed outside the playfield.
///
/// `attackers` is how many units already hold an in-force order on each visible
/// enemy, read from the dispatcher rather than kept as a second table. When more than
/// one enemy is visible the unit picks the least-attended one (nearest on a tie), so
/// a group does not pour every shot into the same target (F28 / P08). With only one
/// visible enemy it still engages that enemy - refusing to fight would be the
/// "everyone stands at home" regression.
pub fn assess(
    snapshot: &Snapshot,
    unit: u64,
    bounds: (u16, u16),
    attackers: &BTreeMap<u64, usize>,
) -> Verdict {
    let Some(own) = snapshot.objects.get(&unit) else {
        return Verdict::Hold;
    };
    // Only revealed, in-playfield enemies this unit can actually hurt. An aircraft
    // above a cannon-only unit is not a target, so it never triggers a chase (P09).
    let visible: Vec<&Object> = snapshot
        .enemy_visible()
        .filter(|enemy| enemy.in_playfield && can_hurt(own, enemy))
        .collect();
    if visible.is_empty() {
        return Verdict::Hold;
    }

    let nearest = *visible
        .iter()
        .min_by_key(|enemy| manhattan(own.cell, enemy.cell))
        .expect("non-empty");
    let target = if visible.len() >= 2 {
        *visible
            .iter()
            .min_by_key(|enemy| {
                (
                    attackers.get(&enemy.id).copied().unwrap_or(0),
                    manhattan(own.cell, enemy.cell),
                )
            })
            .expect("non-empty")
    } else {
        nearest
    };
    let distance = manhattan(own.cell, nearest.cell);
    let own_range = i32::from(own.range_cells.unwrap_or(0));
    let enemy_range = i32::from(nearest.range_cells.unwrap_or(0));

    // Three conservative disadvantage tests. None of them claims to know damage or
    // armour - they answer only "can it reach me before I can reach it / am I already
    // hurt and exposed / are they simply more numerous".
    let hurt = own.strength > 0 && own.health.saturating_mul(3) < own.strength;
    let under_fire = hurt && distance <= enemy_range;
    let out_ranged = enemy_range > own_range && distance <= enemy_range && distance > own_range;
    let own_force = snapshot
        .own()
        .filter(|object| object.in_playfield && !object.is_building())
        .count();
    let outnumbered = visible.len() > own_force;

    if !(under_fire || out_ranged || outnumbered) {
        return Verdict::Engage(target.id);
    }
    match retreat_cell(own.cell, nearest.cell, bounds) {
        Some(cell) => Verdict::Retreat(cell),
        // No cell to fall back to: fighting on is better than freezing in place.
        None => Verdict::Engage(target.id),
    }
}
