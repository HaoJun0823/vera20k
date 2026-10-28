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
    /// Deterministic: the anchor order, the ring order and the sort keys are
    /// all fixed, and no collection with a random iteration order is involved.
    fn sweep(&self, ty: &ObjectType, attempt: u32) -> Option<(u16, u16)> {
        let blocked = self.occupied_cells();
        let mut seen: BTreeSet<(u16, u16)> = BTreeSet::new();
        let mut admitted: Vec<(i32, i32, u16, u16)> = Vec::new();
        for anchor in self.anchors() {
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
                        admitted.push((
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
        admitted.retain(|(_, ring, ..)| *ring <= window);
        admitted.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
                .then(a.3.cmp(&b.3))
        });
        let index = usize::try_from(attempt).ok()?;
        admitted.get(index).map(|(_, _, y, x)| (*x, *y))
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
