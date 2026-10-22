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

use crate::rules::object_type::ObjectType;
use crate::rules::ruleset::RuleSet;
use crate::sim::ai_base_site::{SiteKey, find_base_building_site};
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;

/// How far from an anchor the fallback sweep looks, in cells. A power plant is
/// 2x2 and a new foundation must touch an existing `BaseNormal=` one within its
/// `Adjacent=`, so the nearest admissible ring is always well inside this.
const PLACEMENT_SCAN_RADIUS: i32 = 12;

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
            return Some(cell);
        }
        self.sweep(ty, attempt)
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

    /// The `attempt`-th admissible cell, sweeping outwards from the house's
    /// buildings: the Construction Yard first, then everything else in entity
    /// order. Deterministic, because both the anchor order and the ring order
    /// are.
    fn sweep(&self, ty: &ObjectType, attempt: u32) -> Option<(u16, u16)> {
        let mut seen: Vec<(u16, u16)> = Vec::new();
        let mut admitted = 0;
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
                        if seen.contains(&(rx, ry))
                            || !self.obs.placement_admits(self.rules, ty, rx, ry)
                        {
                            continue;
                        }
                        seen.push((rx, ry));
                        if admitted == attempt {
                            return Some((rx, ry));
                        }
                        admitted += 1;
                    }
                }
            }
        }
        None
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
