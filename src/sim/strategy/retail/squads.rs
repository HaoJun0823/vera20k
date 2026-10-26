//! The army's teams: what each unit is *for*, and what it is held to.
//!
//! Above this the brain says "build this, push there"; below it a unit is told
//! to walk or shoot. What was missing is the middle: nothing owned a unit
//! between the two, so every unit was decided for individually, every frame —
//! which is why the opening squad stands in the base (nothing has given it a
//! job), why the whole army walks at one target (there is one fist and no
//! teams to split it into) and why a raid on the base is met by whoever
//! happens to be nearest.
//!
//! A squad here is a **role and a membership**, not a formation or a
//! state machine: it holds who belongs to it and what it is aimed at, and the
//! membership is rebuilt from the ledger every [`REGROUP_FRAMES`]. Rebuilding
//! rather than recruiting is the whole trick — a unit that dies simply is not
//! in the next count, and there is no "recruit / release / reform" bookkeeping
//! to get wrong. What *is* held across rebuilds is the target, for
//! [`TARGET_LOCK_FRAMES`]: a team that re-picks its target every few seconds
//! fights nothing, because it is always walking somewhere new.
//!
//! Roles are the ones a player runs: hold the base, look at what the enemy is
//! doing, and hit him. [`Role::Attack`] is the fist the advance step already
//! swings; this module is what takes units *out* of it when the base needs
//! them or when nobody is looking at the map.
//!
//! Determinism: `BTreeSet` membership in id order, sorted by squared integer
//! distance with the id as tie-break, so the same ledger gives the same squads.

use std::collections::BTreeSet;

use super::census::{Census, Unit};

/// How often the membership is rebuilt, in frames: twenty seconds. Often
/// enough that a dead unit is not carried for long, rare enough that a team is
/// not dissolved under a unit that is walking towards it.
const REGROUP_FRAMES: u64 = 300;

/// How long a team keeps its target, in frames: two minutes.
///
/// A target is worth walking to, and the walk is the expensive part. A team
/// that re-picks one every few seconds spends the whole fight walking between
/// them and arrives at none.
const TARGET_LOCK_FRAMES: u64 = 1_800;

/// How many units a raid on the base may call home.
///
/// Not the whole army: pulling everything back is what loses the base anyway —
/// the attacker only has to wait, and the house has paid for an army that is
/// standing in its own yard. Enough to meet the raid, taken from the nearest.
const DEFEND_MAX: usize = 8;

/// How near the base a hostile has to be for it to be a raid on the base, in
/// cells.
const HOME_THREAT_CELLS: i32 = 8;

/// What a team is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// Meet a hostile that has come into the base.
    Defend,
    /// Look at the map: what the enemy is building, where his ore is going.
    Scout,
    /// The fist: what is left goes to the advance step.
    Attack,
}

/// One team: who is in it, what it is aimed at, and until when.
#[derive(Debug, Clone)]
pub(super) struct Squad {
    pub(super) role: Role,
    pub(super) members: BTreeSet<u64>,
    /// What the team is aimed at, when it is aimed at one object.
    pub(super) target: Option<u64>,
    /// The frame the team may re-pick its target on.
    pub(super) lock_until: u64,
}

impl Squad {
    /// Whether this team is still holding its target at `tick`.
    pub(super) fn locked(&self, tick: u64) -> bool {
        self.target.is_some() && tick < self.lock_until
    }
}

/// Every team, and when the membership was last rebuilt.
#[derive(Debug, Default)]
pub(super) struct Squads {
    squads: Vec<Squad>,
    /// When the membership was last rebuilt. `None` before the first one, so
    /// the army has teams on the first frame rather than twenty seconds into
    /// the match — which is when the opening squad is standing in the base.
    formed_at: Option<u64>,
}

impl Squads {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Rebuild the membership from the ledger, if the term has run.
    pub(super) fn regroup(&mut self, tick: u64, census: &Census) {
        if self
            .formed_at
            .is_some_and(|at| tick.saturating_sub(at) < REGROUP_FRAMES)
        {
            return;
        }
        self.formed_at = Some(tick);
        self.squads.clear();

        let raid = home_threat(census);
        let mut taken: BTreeSet<u64> = BTreeSet::new();

        // The scouts first: a dog that has been sent to look is not one of the
        // fist, and the fist is short of it either way.
        let scouts: BTreeSet<u64> = census
            .units
            .iter()
            .filter(|unit| unit.traits.scout && unit.orderable && !unit.hand)
            .map(|unit| unit.entity_id)
            .collect();
        taken.extend(scouts.iter().copied());
        self.squads.push(Squad {
            role: Role::Scout,
            members: scouts,
            target: None,
            lock_until: tick.saturating_add(TARGET_LOCK_FRAMES),
        });

        // Then the defenders, if the base is being raided: the nearest few,
        // because the nearest are the ones that can arrive. A team that is
        // still locked on a raider keeps it — otherwise every rebuild would
        // send the defenders at a different hostile and they would meet none
        // of them.
        let held = self
            .squads
            .iter()
            .find(|squad| squad.role == Role::Defend)
            .filter(|squad| squad.locked(tick))
            .and_then(|squad| squad.target);
        let raid = raid.map(|(id, _)| id).or(held);
        let raid_cell = raid.and_then(|id| {
            census
                .enemies
                .iter()
                .find(|enemy| enemy.entity_id == id)
                .map(|enemy| enemy.cell)
        });
        if let (Some(threat), Some(cell)) = (raid, raid_cell) {
            let mut nearest: Vec<&Unit> = census
                .units
                .iter()
                .filter(|unit| {
                    unit.traits.combat && unit.orderable && !unit.hand && !taken.contains(&unit.entity_id)
                })
                .collect();
            nearest.sort_by_key(|unit| (squared_distance(unit.cell, cell), unit.entity_id));
            let members: BTreeSet<u64> = nearest
                .into_iter()
                .take(DEFEND_MAX)
                .map(|unit| unit.entity_id)
                .collect();
            taken.extend(members.iter().copied());
            self.squads.push(Squad {
                role: Role::Defend,
                members,
                target: Some(threat),
                lock_until: tick.saturating_add(TARGET_LOCK_FRAMES),
            });
        }

        // What is left is the fist.
        let attack: BTreeSet<u64> = census
            .units
            .iter()
            .filter(|unit| unit.traits.combat && unit.orderable && !unit.hand)
            .map(|unit| unit.entity_id)
            .filter(|id| !taken.contains(id))
            .collect();
        self.squads.push(Squad {
            role: Role::Attack,
            members: attack,
            target: None,
            lock_until: tick,
        });
    }

    /// Every team, in role order: the micro layer walks all of them.
    pub(super) fn all(&self) -> &[Squad] {
        &self.squads
    }

    /// The team of `role`.
    pub(super) fn squad(&self, role: Role) -> Option<&Squad> {
        self.squads.iter().find(|squad| squad.role == role)
    }

}

/// The hostile inside the base, and where it stands: a raid, and the thing the
/// defenders are for. Nearest to the base first, ties broken by id.
fn home_threat(census: &Census) -> Option<(u64, (u16, u16))> {
    // No base, no raid: a house whose yard is gone has nothing to defend, and
    // the ledger says so by having no base cell.
    let base = census.base_cell?;
    census
        .enemies
        .iter()
        .filter(|enemy| squared_distance(enemy.cell, base) <= HOME_THREAT_CELLS * HOME_THREAT_CELLS)
        .min_by_key(|enemy| (squared_distance(enemy.cell, base), enemy.entity_id))
        .map(|enemy| (enemy.entity_id, enemy.cell))
}

/// Squared cell distance: integer comparisons only.
fn squared_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    let dx = i32::from(a.0) - i32::from(b.0);
    let dy = i32::from(a.1) - i32::from(b.1);
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hostile in the base calls some of the army home — and only some,
    /// because pulling the whole army back is how the base is lost anyway.
    #[test]
    fn a_raid_on_the_base_calls_the_nearest_few_home() {
        let mut census = Census::default();
        census.base_cell = Some((20, 20));
        census.enemies.push(crate::sim::control::observation::EnemyEntity {
            entity_id: 999,
            type_id: crate::sim::intern::InternedId::default(),
            cell: (21, 20),
        });
        for id in 0..12u64 {
            census.units.push(Unit {
                entity_id: id,
                type_id: crate::sim::intern::InternedId::default(),
                cell: (20 + u16::try_from(id).unwrap_or(0), 30),
                health_permille: 1_000,
                mission: crate::sim::mission::MissionId::from_known(crate::sim::mission::MissionType::Guard),
                orderable: true,
                hand: false,
                slot: crate::sim::strategy::retail::census::Slot::Infantry,
                tier: 0,
                traits: crate::sim::strategy::retail::census::Traits {
                    combat: true,
                    harvester: false,
                    refinery: false,
                    yard: false,
                    yard_maker: false,
                    engineer: false,
                    infiltrator: false,
                    occupier: false,
                    deployer: false,
                    scout: false,
                },
            });
        }
        let mut squads = Squads::new();
        squads.regroup(0, &census);
        let defend = squads.squad(Role::Defend).expect("a raid is met");
        assert_eq!(defend.members.len(), DEFEND_MAX, "some of the army, not all");
        assert!(
            defend.members.contains(&0),
            "the nearest is called: {:?}",
            defend.members
        );
        assert!(
            !defend.members.contains(&11),
            "the farthest is not: {:?}",
            defend.members
        );
        let attack = squads.squad(Role::Attack).expect("the fist");
        assert_eq!(
            attack.members.len(),
            12 - DEFEND_MAX,
            "the rest stay in the fist"
        );
    }

    /// No hostile in the base, no defenders: the whole army is the fist.
    #[test]
    fn an_untouched_base_keeps_its_army_in_the_fist() {
        let mut census = Census::default();
        census.base_cell = Some((20, 20));
        let mut squads = Squads::new();
        squads.regroup(0, &census);
        assert!(squads.squad(Role::Defend).is_none());
        // And a rebuild inside the term does nothing, so a team is not
        // dissolved under a unit that is walking towards it.
        squads.regroup(REGROUP_FRAMES - 1, &census);
        assert_eq!(squads.squad(Role::Attack).map_or(0, |s| s.members.len()), 0);
    }
}
