//! Walls around the buildings that are worth more than the wall.
//!
//! What gets walled is the retail data's own answer: `ProtectWithWall=yes`
//! marks a BuildingType the computer may wall in
//! (`BuildingTypeClass ReadINI 0x0046026B..0x0046027F`), and retail sets it on
//! exactly the objects a house cannot afford to lose — the Construction Yard,
//! the tech lab and the superweapons. The same key drives
//! `sim::ai_base_building`'s site border, so a mod that marks a building is
//! walled without a name list here.
//!
//! Two limits come from the engine rather than from the data:
//!
//! - A wall is a `Wall=yes` BuildingType with a one-cell foundation
//!   (`[GAWALL] Adjacent=8`, `Cost=100`, `BuildCat=Combat`), built out of the
//!   **Defense** line, not the yard's building queue. One segment in the queue
//!   is not one cell on the map: `MapClass588750 @ 0x00588750` scans the four
//!   cardinal directions from the cell a wall lands on and constructs every
//!   cell between it and this house's own wall within `GuardRange`
//!   ([`crate::sim::production::wall_placement`]). A straight run is therefore
//!   closed by placing its two ends, the way a player drags one down a line,
//!   and it costs two slots rather than one per cell.
//! - A wall blocks its owner's units as well, and retail has no gate this house
//!   can build, so the arc facing *away* from the enemy is the only part worth
//!   closing. The side facing the base stays open: that is where the yard's
//!   own units come and go.
//!
//! The result is a segment plan, not a picture: the next cell to fill is
//! computed from the base as it stands, so a wall that was interrupted, a
//! building that died, or a base that grew all answer the next question
//! correctly without a stored plan to go stale.

use std::collections::BTreeSet;

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;
use crate::sim::production::{CanBuild, ProductionCategory};

use super::RetailBrain;

/// Credits in hand before a segment is worth a production slot. A wall costs
/// the price of a soldier and stalls nothing, but a house that builds walls on
/// its last credits has an empty base with a pretty outline.
pub(super) const WALL_CREDITS: i32 = 2_000;

/// Structures on the map before the base is worth walling: a yard and a
/// refinery do not need an outline, and the first minutes belong to the
/// opening.
pub(super) const WALL_MIN_STRUCTURES: usize = 6;

/// Segments a house may have standing. A four-cell building's enemy-facing arc
/// is a dozen cells and the base has two or three of them; past this the walls
/// are eating the defenses' whole queue.
pub(super) const WALL_SEGMENT_CAP: usize = 24;

/// The wall segment the base wants: the cell to put it on, the run it closes,
/// and how much of the wall already stands.
pub(super) struct Segment {
    /// The house's own wall type.
    pub(super) type_id: InternedId,
    /// The cell to place on: the end of a run, not the cell next to the last
    /// one placed.
    pub(super) cell: (u16, u16),
    /// Wall cells already standing on the rings the plan fills.
    pub(super) standing: usize,
    /// Cells the engine will build in the gap this segment closes.
    pub(super) fill: usize,
}

/// The wall's own scan order: north, east, south, west
/// (`sim/production/wall_placement::CARDINAL_DIRECTIONS`).
const CARDINAL: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

/// How many cells the engine fills in when a wall goes down on `cell`: the
/// cells between it and this house's own wall, in one cardinal direction, up to
/// the type's `GuardRange` away. Zero when there is nothing to close.
///
/// `MapClass588750 @ 0x00588750` walks each direction from the cell placed and
/// constructs the gap it finds, discarding any direction whose cells are not
/// clear to build. So a run is paid for by its two ends, and the cell worth a
/// production slot is the one that closes the longest gap — a wall next to the
/// one already standing closes nothing at all.
pub(super) fn autofill_gain(
    cell: (u16, u16),
    limit: usize,
    walled: impl Fn(u16, u16) -> bool,
    free: impl Fn(u16, u16) -> bool,
) -> usize {
    if limit == 0 {
        return 0;
    }
    let mut best = 0;
    for (dx, dy) in CARDINAL {
        let (mut x, mut y) = (i32::from(cell.0), i32::from(cell.1));
        for step in 1..=limit {
            x += dx;
            y += dy;
            let (Ok(cx), Ok(cy)) = (u16::try_from(x), u16::try_from(y)) else {
                break;
            };
            if walled(cx, cy) {
                best = best.max(step - 1);
                break;
            }
            if !free(cx, cy) {
                break;
            }
        }
    }
    best
}

/// Whether `ty` is a building the retail data says to wall in.
pub(super) fn protected(ty: &ObjectType) -> bool {
    ty.protect_with_wall && ty.category == ObjectCategory::Building
}

/// The cells around a building's foundation, one ring out: the cells a wall
/// would stand on.
pub(super) fn perimeter(cell: (u16, u16), foundation: (u16, u16)) -> Vec<(u16, u16)> {
    let (width, height) = (i32::from(foundation.0.max(1)), i32::from(foundation.1.max(1)));
    let (x0, y0) = (i32::from(cell.0), i32::from(cell.1));
    let (x1, y1) = (x0 + width - 1, y0 + height - 1);
    let mut cells: Vec<(u16, u16)> = Vec::new();
    for y in (y0 - 1)..=(y1 + 1) {
        for x in (x0 - 1)..=(x1 + 1) {
            let on_ring = x < x0 || x > x1 || y < y0 || y > y1;
            if !on_ring || x < 0 || y < 0 {
                continue;
            }
            let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
                continue;
            };
            cells.push((x, y));
        }
    }
    cells
}

/// The half of a ring that faces `direction`: cells whose offset from the
/// building's centre points the same way.
///
/// `direction` is in cells and need not be normalised; a zero direction keeps
/// nothing, because a wall with no enemy side would close the base on its own
/// units. Ties (a cell exactly beside the building, neither toward nor away
/// from the enemy) are dropped, which leaves a gap on each side of the arc
/// rather than a corner a unit cannot pass.
pub(super) fn facing(
    cells: &[(u16, u16)],
    centre: (u16, u16),
    direction: (i32, i32),
) -> Vec<(u16, u16)> {
    if direction == (0, 0) {
        return Vec::new();
    }
    cells
        .iter()
        .copied()
        .filter(|(x, y)| {
            let dx = i32::from(*x) - i32::from(centre.0);
            let dy = i32::from(*y) - i32::from(centre.1);
            dx * direction.0 + dy * direction.1 > 0
        })
        .collect()
}

/// The house's wall type, resolved once from the rules and cached: a
/// `Wall=yes` BuildingType the house may actually build (`[GAWALL]
/// Prerequisite=GAPILE`).
pub(super) fn wall_type(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<InternedId> {
    if let Some(type_id) = brain.wall_type {
        return Some(type_id);
    }
    let found = rules
        .all_objects()
        .filter(|ty| ty.wall && ty.category == ObjectCategory::Building)
        .find(|ty| matches!(obs.can_build(rules, ty, true), CanBuild::Yes))
        .and_then(|ty| obs.type_id(&ty.id))?;
    brain.wall_type = Some(found);
    Some(found)
}

/// The next wall segment the base wants, when it wants one.
///
/// `None` while the base is too small, too poor, already walled enough, or has
/// no enemy to face: a wall is only ever built toward something.
pub(super) fn next_segment(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<Segment> {
    if brain.census.home_cells.is_empty() {
        return None;
    }
    let structures = obs
        .own_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| ty.category == ObjectCategory::Building && !ty.wall)
        })
        .count();
    if structures < WALL_MIN_STRUCTURES {
        return None;
    }
    let (centre, targets) = base_targets(rules, obs)?;
    // What the cap counts is the wall on the rings the plan fills. A standing
    // wall is an overlay rather than an object (`Observation::wall_owner_at`),
    // so it is counted where it stands.
    let house = obs.house();
    let mut ring: BTreeSet<(u16, u16)> = BTreeSet::new();
    for (cell, foundation) in &targets {
        ring.extend(perimeter(*cell, *foundation));
    }
    let standing = ring
        .iter()
        .filter(|(x, y)| obs.wall_owner_at(*x, *y) == Some(house))
        .count();
    if standing >= WALL_SEGMENT_CAP {
        return None;
    }
    let blocked = blocked_cells(rules, obs);
    // The enemy side: the nearest hostile object to the base's centre, so the
    // wall faces the way an attack has to come from. A house that has seen no
    // enemy builds no wall.
    let enemy = brain
        .census
        .enemies
        .iter()
        .map(|enemy| (enemy.cell, chebyshev(enemy.cell, centre)))
        .min_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)))?
        .0;
    let direction = (
        i32::from(enemy.0) - i32::from(centre.0),
        i32::from(enemy.1) - i32::from(centre.1),
    );
    let wall = wall_type(brain, rules, obs)?;
    // How far the engine looks for this house's own wall when it fills a gap:
    // the wall type's own `GuardRange`, in cells. A type without one closes
    // nothing, and the plan falls back to the cell beside the last one.
    let limit = obs
        .object_type(rules, wall)
        .and_then(|ty| ty.guard_range)
        .map_or(0, |range| range.to_num::<i32>().max(0) as usize);
    // A cell that already holds a wall, its own or a hostile one, is not walled
    // again; the walkability of the path projection says the same, one step
    // later than the grid does.
    let free = |x: u16, y: u16| {
        !blocked.contains(&(x, y)) && obs.wall_owner_at(x, y).is_none() && obs.is_ground_walkable(x, y)
    };
    for (cell, foundation) in targets {
        let ring = perimeter(cell, foundation);
        // The arc is the building's own, so the offset is taken from its
        // centre: a building at the edge of the base still gets a wall on the
        // side that faces the enemy, not on the side that faces away from the
        // base's centroid.
        let centre = (
            cell.0.saturating_add(foundation.0 / 2),
            cell.1.saturating_add(foundation.1 / 2),
        );
        let arc = facing(&ring, centre, direction);
        // The end of the run, not the cell beside the last one: a segment is
        // worth the gap it closes, because the engine builds the cells between
        // for free. Ties keep the first cell of the arc.
        let mut best: Option<((u16, u16), usize)> = None;
        for cell in arc {
            if !free(cell.0, cell.1) {
                continue;
            }
            let fill = autofill_gain(cell, limit, |x, y| obs.wall_owner_at(x, y) == Some(house), &free);
            if best.is_none_or(|(_, best_fill)| fill > best_fill) {
                best = Some((cell, fill));
            }
        }
        if let Some((cell, fill)) = best {
            return Some(Segment {
                type_id: wall,
                cell,
                standing,
                fill,
            });
        }
    }
    None
}

/// The cells any building's foundation stands on, own and hostile. Walls are
/// not here: they are overlays, and the grid answers them (`wall_owner_at`).
fn blocked_cells(rules: &RuleSet, obs: &Observation<'_>) -> BTreeSet<(u16, u16)> {
    let mut cells: BTreeSet<(u16, u16)> = BTreeSet::new();
    for (cell, type_id) in obs
        .own_entities()
        .map(|entity| (entity.cell, entity.type_id))
        .chain(
            obs.enemy_entities()
                .map(|entity| (entity.cell, entity.type_id)),
        )
    {
        let Some(ty) = obs.object_type(rules, type_id) else {
            continue;
        };
        if ty.category != ObjectCategory::Building {
            continue;
        }
        let (width, height) = ty.foundation.dimensions();
        for dy in 0..i32::from(height.max(1)) {
            for dx in 0..i32::from(width.max(1)) {
                let (Some(x), Some(y)) = (
                    u16::try_from(i32::from(cell.0) + dx).ok(),
                    u16::try_from(i32::from(cell.1) + dy).ok(),
                ) else {
                    continue;
                };
                cells.insert((x, y));
            }
        }
    }
    cells
}

/// The base's centre and the buildings worth walling, in entity order so the
/// plan is deterministic.
fn base_targets(
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<((u16, u16), Vec<((u16, u16), (u16, u16))>)> {
    let mut targets: Vec<(u64, (u16, u16), (u16, u16))> = Vec::new();
    for entity in obs.own_entities() {
        let Some(ty) = obs.object_type(rules, entity.type_id) else {
            continue;
        };
        if !protected(ty) {
            continue;
        }
        let (width, height) = ty.foundation.dimensions();
        targets.push((entity.entity_id, entity.cell, (width, height)));
    }
    if targets.is_empty() {
        return None;
    }
    targets.sort_by_key(|(entity_id, ..)| *entity_id);
    let (mut sx, mut sy, mut count) = (0_i64, 0_i64, 0_i64);
    for (_, cell, _) in &targets {
        sx += i64::from(cell.0);
        sy += i64::from(cell.1);
        count += 1;
    }
    let centre = (
        u16::try_from(sx / count).unwrap_or(0),
        u16::try_from(sy / count).unwrap_or(0),
    );
    Some((
        centre,
        targets
            .into_iter()
            .map(|(_, cell, foundation)| (cell, foundation))
            .collect(),
    ))
}

fn chebyshev(a: (u16, u16), b: (u16, u16)) -> i32 {
    i32::from(a.0)
        .abs_diff(i32::from(b.0))
        .max(i32::from(a.1).abs_diff(i32::from(b.1))) as i32
}

/// Queue one segment, when the base wants one and the Defense line is free.
///
/// Returns `true` when this frame's structure decision was spent on a wall, so
/// the plan does not start something else on top of it.
pub(super) fn queue_segment(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    free_credits: i32,
    out: &mut Outbox,
) -> bool {
    if free_credits < WALL_CREDITS {
        return false;
    }
    let line = brain.census.line(ProductionCategory::Defense);
    if line.busy() {
        return false;
    }
    let Some(segment) = next_segment(brain, rules, obs) else {
        return false;
    };
    log::info!(
        "auto play: wall segment {} at tick {} credits={} cell={:?} standing={} fill={}",
        obs.object_type(rules, segment.type_id)
            .map_or("?", |ty| ty.id.as_str()),
        obs.tick(),
        free_credits,
        segment.cell,
        segment.standing,
        segment.fill,
    );
    out.intent(Intent::QueueProduction {
        type_id: segment.type_id,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ring around a 2x2 foundation is twelve cells: the eight sides plus
    /// the four corners, all one cell out.
    #[test]
    fn a_ring_surrounds_the_foundation() {
        let ring = perimeter((10, 10), (2, 2));
        assert_eq!(ring.len(), 12);
        assert!(ring.contains(&(9, 9)), "corner");
        assert!(ring.contains(&(12, 12)), "far corner");
        assert!(ring.contains(&(11, 9)), "edge");
        assert!(!ring.contains(&(10, 10)), "the foundation itself");
        assert!(!ring.contains(&(11, 11)), "inside the foundation");
    }

    /// Only the half facing the enemy is walled: the base side stays open, so
    /// the house's own units keep their way out.
    #[test]
    fn the_arc_faces_the_enemy() {
        let ring = perimeter((10, 10), (2, 2));
        // A 2x2 standing at (10,10) has its centre at (11,11).
        let arc = facing(&ring, (11, 11), (1, 0));
        assert_eq!(arc.len(), 4, "{arc:?} is the east arc");
        assert!(arc.iter().all(|(x, _)| *x > 11), "{arc:?} is the east arc");
        assert!(!arc.contains(&(9, 11)), "the west side is left open");
        assert!(!arc.contains(&(11, 9)), "a cell straight beside is a tie");
        // Nothing to face means no wall.
        assert!(facing(&ring, (11, 11), (0, 0)).is_empty());
    }

    /// The cell worth a production slot is the end of the run, because the
    /// engine builds the gap between it and the wall that already stands.
    #[test]
    fn the_far_end_of_a_run_is_the_cell_that_closes_it() {
        // One wall at (10,10), then four cells a wall could go on.
        let walled = |x: u16, _y: u16| x == 10;
        let free = |x: u16, _y: u16| (11..=14).contains(&x);
        assert_eq!(
            autofill_gain((14, 10), 8, walled, free),
            3,
            "three cells between the two ends"
        );
        assert_eq!(
            autofill_gain((11, 10), 8, walled, free),
            0,
            "a wall beside the one standing closes nothing"
        );
        assert_eq!(
            autofill_gain((14, 10), 3, walled, free),
            0,
            "past GuardRange the wall is never found"
        );
        // A cell that cannot be built on discards the whole direction.
        assert_eq!(
            autofill_gain((14, 10), 8, walled, |x, y| free(x, y) && x != 13),
            0,
            "a blocked cell between the two ends"
        );
        // A type with no GuardRange closes nothing at all.
        assert_eq!(autofill_gain((14, 10), 0, walled, free), 0);
    }

    /// A standing wall is an overlay, not an object, so the plan counts it from
    /// the grid; and a sold cell keeps its owner there, which is not a wall.
    #[test]
    fn a_standing_wall_is_read_from_the_overlay_grid() {
        use crate::sim::overlay_grid::OverlayGrid;
        use crate::sim::world::Simulation;
        let mut sim = Simulation::new();
        let house = sim.interner.intern("Americans");
        let wall = 2;
        let mut grid = OverlayGrid::new(4, 4);
        grid.place_owned_wall(2, 1, wall, 0, house);
        sim.overlay_grid = Some(grid);
        let obs = Observation::new(&sim, house);
        assert_eq!(obs.wall_owner_at(2, 1), Some(house));
        assert_eq!(obs.wall_owner_at(0, 0), None, "an empty cell");

        // A sale clears the overlay and leaves the owner behind for the grid's
        // retained neighbour counts.
        sim.overlay_grid
            .as_mut()
            .unwrap()
            .cell_mut_for_test(2, 1)
            .overlay_id = None;
        let obs = Observation::new(&sim, house);
        assert_eq!(
            sim.overlay_grid.as_ref().unwrap().cell(2, 1).wall_owner,
            Some(house),
            "the grid keeps the last owner of a sold wall"
        );
        assert_eq!(obs.wall_owner_at(2, 1), None, "a sold wall");
    }

    /// The key is the retail one: a building the data marks is walled, and one
    /// it does not is not, whatever its name.
    #[test]
    fn protected_comes_from_the_data() {
        let rules = match crate::rules::retail_ini_fixture::retail_unlayered_rules() {
            Some(rules) => rules,
            None => return,
        };
        let yard = rules.object("GACNST").expect("the Construction Yard");
        assert!(protected(yard), "retail marks the yard ProtectWithWall=yes");
        let refinery = rules.object("GAREFN").expect("a refinery");
        assert!(
            !protected(refinery),
            "retail does not mark the refinery, so it is not walled"
        );
        let wall = rules.object("GAWALL").expect("the Allied wall");
        assert!(wall.wall, "GAWALL is a wall segment");
    }
}
