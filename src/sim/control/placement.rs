//! Where a finished building goes: the native site first, a bounded sweep as
//! the fallback.
//!
//! The native answer is `HouseClass::FindBaseBuildingSite` (`0x005060B0`,
//! [`crate::sim::ai_base_site::find_base_building_site`]) — the same search the
//! computer AI makes before it places a structure. It keys off the house's base
//! reservation and plan centre, so for a house that has played as a player (no
//! `+0x5750` plan centre, and a reservation that may be empty) it can answer
//! "no site" or a cell the sidebar's build area refuses.
//!
//! Hence the second half. When the native search gives nothing admissible, the
//! sweep walks outwards from the house's Construction Yard and then its other
//! structures — ring by ring, in a fixed order — and returns the `attempt`-th
//! cell the placement gate admits. That is a policy choice, not a port, and it
//! is recorded as such: a human house has no native plan centre to ask, and the
//! retail game never needs this path because a human clicks. The `attempt`
//! parameter is how the caller moves on after the receiver refuses a cell.

use std::collections::BTreeSet;

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::sim::ai_base_site::{SiteKey, find_base_building_site};
use crate::sim::behavior::view::cell_distance;
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;

/// How far from an anchor the fallback sweep looks, in cells. A power plant is
/// 2x2 and a new foundation must touch an existing `BaseNormal=` one within its
/// `Adjacent=`, so the nearest admissible ring is always well inside this.
const PLACEMENT_SCAN_RADIUS: i32 = 12;

/// How far around a candidate the openness score looks, in cells: a two-cell
/// ring catches the difference between a cell on a lane and one wedged in a
/// corner between two foundations.
const OPENNESS_RADIUS: i32 = 2;

/// How many rings past the nearest admissible one a roomier cell may be taken
/// from. Two is one building's width: enough to step out of a gap, not enough
/// to leave the base.
const PLACEMENT_WINDOW_RINGS: i32 = 2;

/// How many admitted cells the sweep counts before starting over: a cell the
/// gate admits but the receiver refuses must not be asked for forever.
pub const PLACEMENT_ATTEMPTS: u32 = 64;

/// The sectors the base grows into around its first anchor: eight, so a house
/// that has built east does not keep building east.
const SECTORS: usize = 8;

/// Which of the eight sectors around `origin` a cell lies in: east first, then
/// clockwise. A sector is cardinal when one offset is at least twice the other,
/// so a broad wedge counts as the side it points at and only the rest is a
/// diagonal.
///
/// Integer throughout: a decision that picks where a building goes has no
/// float in it.
fn sector_of(origin: Option<(u16, u16)>, cell: (u16, u16)) -> usize {
    let Some(origin) = origin else {
        return 0;
    };
    let (dx, dy) = (
        i32::from(cell.0) - i32::from(origin.0),
        i32::from(cell.1) - i32::from(origin.1),
    );
    let (ax, ay) = (dx.abs(), dy.abs());
    match (dx >= 0, dy >= 0, ax >= 2 * ay, ay >= 2 * ax) {
        (true, _, true, _) => 0,  // east
        (false, _, true, _) => 4, // west
        (_, false, _, true) => 2, // north
        (_, true, _, true) => 6,  // south
        (true, false, ..) => 1,   // north-east
        (false, false, ..) => 3,  // north-west
        (false, true, ..) => 5,   // south-west
        (true, true, ..) => 7,    // south-east
    }
}

/// How many of the house's buildings already stand in each sector around
/// `origin`: the crowding the sweep spreads away from.
fn sector_counts(origin: Option<(u16, u16)>, anchors: &[(u16, u16)]) -> [i32; SECTORS] {
    let mut counts = [0_i32; SECTORS];
    for cell in anchors {
        counts[sector_of(origin, *cell)] += 1;
    }
    counts
}

/// One house's placement answers, for one frame.
pub struct Placement<'a, 'b> {
    obs: &'a Observation<'b>,
    rules: &'a RuleSet,
}

impl<'a, 'b> Placement<'a, 'b> {
    pub fn new(obs: &'a Observation<'b>, rules: &'a RuleSet) -> Self {
        Self { obs, rules }
    }

    /// The cell to place `type_id` at, `attempt`-th admissible candidate.
    ///
    /// The native site wins when the placement gate admits it; otherwise the
    /// outward sweep answers.
    pub fn cell_for(&self, type_id: InternedId, attempt: u32) -> Option<(u16, u16)> {
        let ty = self.obs.object_type(self.rules, type_id)?;
        if let Some(cell) = self.native_site(ty)
            && self.obs.placement_admits(self.rules, ty, cell.0, cell.1)
        {
            log::info!(
                "auto play: site for {} from the native search at {cell:?}",
                ty.id
            );
            return Some(cell);
        }
        let cell = self.sweep(ty, attempt);
        log::info!(
            "auto play: site for {} from the sweep at {cell:?} (attempt {attempt})",
            ty.id
        );
        cell
    }

    /// `HouseClass::FindBaseBuildingSite` for the ordinary branch. `None` when
    /// it found nothing (it answers `(0, 0)`) or the cell does not fit `u16`.
    fn native_site(&self, ty: &ObjectType) -> Option<(u16, u16)> {
        let (x, y) = find_base_building_site(
            self.obs.simulation(),
            self.rules,
            self.obs.house(),
            ty,
            SiteKey::Ordinary,
            None,
        );
        if x <= 0 || y <= 0 {
            return None;
        }
        Some((u16::try_from(x).ok()?, u16::try_from(y).ok()?))
    }

    /// The `attempt`-th best cell, from a sweep outwards from the house's
    /// buildings: the Construction Yard first, then everything else in entity
    /// order.
    ///
    /// "Best" is the nearest ring that is not a gap: of the admitted cells,
    /// those with at least three fifths of the roomiest cell's open ground are
    /// considered, and the nearest of those wins, then the roomiest, then a
    /// fixed cell order. Building in ring order alone wedges new foundations
    /// into the one-cell gaps between existing ones — the densest possible base,
    /// and the one that walls its own yard in; ranking by openness alone walks
    /// the base out onto the open ground and leaves the refinery sixteen cells
    /// from the yard, where nothing can defend it. The filter is a policy
    /// choice, like the sweep itself: a human places by eye, and an eye keeps a
    /// gap without crossing the map for it.
    ///
    /// A building retail marks `ProtectWithWall=yes` is then kept on the side
    /// no attack comes from ([`Self::keep_safe_side`]).
    ///
    /// Deterministic: the anchor order, the ring order and the sort keys are
    /// all fixed, and no collection with a random iteration order is involved.
    fn sweep(&self, ty: &ObjectType, attempt: u32) -> Option<(u16, u16)> {
        let blocked = self.occupied_cells();
        let anchors = self.anchors();
        let mut seen: BTreeSet<(u16, u16)> = BTreeSet::new();
        let mut admitted: Vec<(i32, i32, i32, u16, u16)> = Vec::new();
        // How much of the base already stands in each sector around the first
        // anchor: the yard, which is what everything else is built out from.
        let crowded = sector_counts(anchors.first().copied(), &anchors);
        for anchor in &anchors {
            for ring in 1..=PLACEMENT_SCAN_RADIUS {
                for dy in -ring..=ring {
                    for dx in -ring..=ring {
                        if dx.abs().max(dy.abs()) != ring {
                            continue;
                        }
                        let (Some(rx), Some(ry)) = (
                            u16::try_from(i32::from(anchor.0) + dx).ok(),
                            u16::try_from(i32::from(anchor.1) + dy).ok(),
                        ) else {
                            continue;
                        };
                        if !seen.insert((rx, ry))
                            || !self.obs.placement_admits(self.rules, ty, rx, ry)
                        {
                            continue;
                        }
                        let sector = sector_of(anchors.first().copied(), (rx, ry));
                        admitted.push((
                            crowded[sector],
                            self.openness(rx, ry, &blocked),
                            ring,
                            ry,
                            rx,
                        ));
                    }
                }
            }
        }
        // The base stays where it is: only the first rings are considered, so a
        // roomier cell across the map cannot outrank a tighter one beside the
        // yard. Within that window the roomiest cell wins — a foundation wedged
        // into a one-cell gap between two others walls the base in, and a yard
        // with no lane out is a yard that feeds nothing.
        let Some(nearest) = admitted.iter().map(|(_, ring, ..)| *ring).min() else {
            return None;
        };
        let window = nearest + PLACEMENT_WINDOW_RINGS;
        admitted.retain(|(_, _, ring, ..)| *ring <= window);
        if ty.protect_with_wall {
            self.keep_safe_side(&mut admitted, &anchors);
        }
        // Nearest the yard first, then the emptiest sector at that distance,
        // then the roomiest cell in it. The distance is what keeps the base a
        // base: a sweep that asks for the emptiest sector wherever it lies
        // walks the base off the map, one uncrowded ring at a time. The sector
        // is what stops it stacking on the side it started on, and the
        // openness is what keeps a lane between what it has already built.
        let yard = anchors.first().copied();
        let distance = |candidate: &(i32, i32, i32, u16, u16)| {
            yard.map_or(0, |yard| cell_distance((candidate.4, candidate.3), yard))
        };
        admitted.sort_by(|a, b| {
            distance(a)
                .cmp(&distance(b))
                .then(a.0.cmp(&b.0))
                .then(b.1.cmp(&a.1))
                .then(a.2.cmp(&b.2))
                .then(a.3.cmp(&b.3))
                .then(a.4.cmp(&b.4))
        });
        let index = usize::try_from(attempt).ok()?;
        admitted.get(index).map(|(_, _, _, y, x)| (*x, *y))
    }

    /// Keep the cells on the side no attack comes from, when there are any.
    ///
    /// `ProtectWithWall=yes` is retail's own list of what a house cannot afford
    /// to lose — the Construction Yard, the tech lab, the superweapons — the
    /// same list [`crate::sim::strategy::retail::walls`] closes a wall around.
    /// Putting one of those on the enemy's side of the base is putting the tree
    /// it unlocks in front of the first tank that gets through; the sweep's
    /// openness ranking alone does not care which way the base faces.
    ///
    /// The direction is the one measured from the base's own buildings to the
    /// nearest hostile object, so "the back" follows the same enemy the walls
    /// face. A house that has seen no enemy has no front, and keeps every cell.
    fn keep_safe_side(&self, admitted: &mut Vec<(i32, i32, i32, u16, u16)>, anchors: &[(u16, u16)]) {
        let Some((centre, direction)) = self.enemy_side(anchors) else {
            return;
        };
        let safe: Vec<(i32, i32, i32, u16, u16)> = admitted
            .iter()
            .copied()
            .filter(|(_, _, _, y, x)| {
                let dx = i32::from(*x) - i32::from(centre.0);
                let dy = i32::from(*y) - i32::from(centre.1);
                dx * direction.0 + dy * direction.1 <= 0
            })
            .collect();
        if !safe.is_empty() {
            *admitted = safe;
        }
    }

    /// The centre of the house's own buildings and the direction, in cells, its
    /// nearest hostile object lies in. `None` with nothing to anchor on and
    /// nothing coming.
    fn enemy_side(&self, anchors: &[(u16, u16)]) -> Option<((u16, u16), (i32, i32))> {
        let (mut sx, mut sy, mut count) = (0_i64, 0_i64, 0_i64);
        for cell in anchors {
            sx += i64::from(cell.0);
            sy += i64::from(cell.1);
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let centre = (
            u16::try_from(sx / count).ok()?,
            u16::try_from(sy / count).ok()?,
        );
        let enemy = self
            .obs
            .enemy_entities()
            .map(|entity| entity.cell)
            .min_by_key(|cell| cell_distance(*cell, centre))?;
        Some((
            centre,
            (
                i32::from(enemy.0) - i32::from(centre.0),
                i32::from(enemy.1) - i32::from(centre.1),
            ),
        ))
    }

    /// How much open ground surrounds a cell: walkable cells within
    /// [`OPENNESS_RADIUS`] that no building's foundation covers.
    ///
    /// A cell in a corner between two structures scores low and is only taken
    /// when nothing roomier is admissible, which is what keeps lanes between
    /// buildings instead of filling every gap the moment it appears.
    fn openness(&self, x: u16, y: u16, blocked: &BTreeSet<(u16, u16)>) -> i32 {
        let mut open = 0;
        for dy in -OPENNESS_RADIUS..=OPENNESS_RADIUS {
            for dx in -OPENNESS_RADIUS..=OPENNESS_RADIUS {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (Some(cx), Some(cy)) = (
                    u16::try_from(i32::from(x) + dx).ok(),
                    u16::try_from(i32::from(y) + dy).ok(),
                ) else {
                    continue;
                };
                if blocked.contains(&(cx, cy)) {
                    continue;
                }
                if self.obs.is_ground_walkable(cx, cy) {
                    open += 1;
                }
            }
        }
        open
    }

    /// Every cell any building's foundation stands on, own and hostile.
    ///
    /// The path grid already refuses a foundation, but not the cells *beside*
    /// one, and a lane is made of the cells beside one.
    fn occupied_cells(&self) -> BTreeSet<(u16, u16)> {
        let mut cells = BTreeSet::new();
        for (cell, type_id) in self
            .obs
            .own_entities()
            .map(|entity| (entity.cell, entity.type_id))
            .chain(
                self.obs
                    .enemy_entities()
                    .map(|entity| (entity.cell, entity.type_id)),
            )
        {
            let Some(ty) = self.obs.object_type(self.rules, type_id) else {
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

    /// The sweep's anchors: every Construction Yard maker's cell, then the
    /// house's other structures, in entity order.
    fn anchors(&self) -> Vec<(u16, u16)> {
        let mut anchors: Vec<(u16, u16)> = Vec::new();
        let mut rest: Vec<(u16, u16)> = Vec::new();
        for entity in self.obs.own_entities() {
            let Some(ty) = self.obs.object_type(self.rules, entity.type_id) else {
                continue;
            };
            if ty.category != crate::rules::object_type::ObjectCategory::Building {
                continue;
            }
            if ty.build_const_eligible {
                anchors.push(entity.cell);
            } else {
                rest.push(entity.cell);
            }
        }
        for cell in rest {
            if !anchors.contains(&cell) {
                anchors.push(cell);
            }
        }
        anchors
    }
}
