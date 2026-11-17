//! The assist: the things a player does without deciding to.
//!
//! This is not a bot. It does not choose what to build, where to expand, or
//! when to attack — those belong to a bot, and a bot is a separate program with
//! its own rules and its own acceptance. What lives here is the reflex layer:
//! the actions a player performs without a decision, and would be annoyed to
//! have forgotten.
//!
//! Every rule is one sentence, and the sentence is the test:
//!
//! - an intruder at the door is met by the nearest free soldier.

use vera20k_core::rules::capabilities::{Capabilities, Tag};
use vera20k_core::rules::object_type::{ObjectCategory, ObjectType};
use vera20k_core::rules::ruleset::RuleSet;
use vera20k_core::sim::behavior::goal::Goal;
use vera20k_core::sim::control::brain::{Brain, Outbox};
use vera20k_core::sim::control::observation::Observation;

/// How far from the base an intruder still counts as being at the door, in
/// cells (Chebyshev, so a diagonal is one step).
const BASE_GUARD_CELLS: u16 = 12;

/// How many intruders one frame answers. A player does not send the whole army
/// at one rifleman, and neither does this.
const MAX_TARGETS: usize = 4;

/// One object as the reflex layer sees it: who it is, where it stands, and
/// whether it can shoot back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blip {
    pub id: u64,
    pub cell: (u16, u16),
    pub combat: bool,
}

/// Which soldier shoots which intruder.
///
/// The intruders nearest the base come first, each takes the nearest soldier
/// that is not already answering one, and the whole plan is at most
/// `max_targets` pairs. Pure, so the rule can be read against its test without
/// a simulation.
pub fn plan_defense(
    base: (u16, u16),
    own: &[Blip],
    enemies: &[Blip],
    max_targets: usize,
) -> Vec<(u64, u64)> {
    let mut intruders: Vec<&Blip> = enemies
        .iter()
        .filter(|enemy| chebyshev(enemy.cell, base) <= i32::from(BASE_GUARD_CELLS))
        .collect();
    intruders.sort_by_key(|enemy| chebyshev(enemy.cell, base));

    let soldiers: Vec<&Blip> = own.iter().filter(|unit| unit.combat).collect();
    let mut answering = vec![false; soldiers.len()];
    let mut plan = Vec::new();

    for enemy in intruders.into_iter().take(max_targets) {
        let mut best: Option<(usize, i32)> = None;
        for (index, soldier) in soldiers.iter().enumerate() {
            if answering[index] {
                continue;
            }
            let distance = chebyshev(soldier.cell, enemy.cell);
            if best.is_none_or(|(_, held)| distance < held) {
                best = Some((index, distance));
            }
        }
        if let Some((index, _)) = best {
            answering[index] = true;
            plan.push((soldiers[index].id, enemy.id));
        }
    }
    plan
}

fn chebyshev(a: (u16, u16), b: (u16, u16)) -> i32 {
    i32::from(a.0.abs_diff(b.0)).max(i32::from(a.1.abs_diff(b.1)))
}

/// The assist brain.
#[derive(Debug, Default)]
pub struct Assist;

impl Assist {
    /// The base the reflexes defend: the first structure the house still holds.
    /// A house with none has nothing to defend yet, and no rule fires.
    fn base(obs: &Observation<'_>, rules: &RuleSet) -> Option<(u16, u16)> {
        obs.own_entities().find_map(|entity| {
            obs.object_type(rules, entity.type_id)
                .filter(|ty| ty.category == ObjectCategory::Building)
                .map(|_| entity.cell)
        })
    }
}

impl Brain for Assist {
    fn name(&self) -> &'static str {
        "assist"
    }

    fn think(&mut self, rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
        let Some(base) = Self::base(obs, rules) else {
            return;
        };
        let own: Vec<Blip> = obs
            .own_entities()
            .map(|entity| Blip {
                id: entity.entity_id,
                cell: entity.cell,
                combat: obs
                    .object_type(rules, entity.type_id)
                    .is_some_and(|ty| is_combat(rules, ty)),
            })
            .collect();
        let enemies: Vec<Blip> = obs
            .visible_enemy_entities()
            .map(|enemy| Blip {
                id: enemy.entity_id,
                cell: enemy.cell,
                combat: true,
            })
            .collect();

        for (soldier, intruder) in plan_defense(base, &own, &enemies, MAX_TARGETS) {
            out.order(soldier, Goal::Attack { target: intruder });
        }
    }
}

/// Whether a type can shoot: an armed soldier or vehicle. A harvester is a
/// vehicle and is not a soldier, so it never answers an intruder.
fn is_combat(rules: &RuleSet, ty: &ObjectType) -> bool {
    matches!(
        ty.category,
        ObjectCategory::Infantry | ObjectCategory::Vehicle
    ) && Capabilities::of(rules, ty).has(Tag::Armed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blip(id: u64, cell: (u16, u16), combat: bool) -> Blip {
        Blip { id, cell, combat }
    }

    #[test]
    fn an_intruder_at_the_door_is_met_by_the_nearest_soldier() {
        let base = (10, 10);
        let own = [
            blip(1, (12, 10), true),
            blip(2, (30, 30), true),
            blip(3, (11, 10), false),
        ];
        let enemies = [blip(101, (14, 10), true)];

        assert_eq!(plan_defense(base, &own, &enemies, 4), vec![(1, 101)]);
    }

    #[test]
    fn an_intruder_across_the_map_is_left_alone() {
        let base = (10, 10);
        let own = [blip(1, (12, 10), true)];
        let enemies = [blip(101, (120, 10), true)];

        assert!(plan_defense(base, &own, &enemies, 4).is_empty());
    }

    #[test]
    fn one_soldier_answers_one_intruder() {
        let base = (10, 10);
        let own = [blip(1, (12, 10), true)];
        let enemies = [blip(101, (14, 10), true), blip(102, (10, 15), true)];

        assert_eq!(
            plan_defense(base, &own, &enemies, 4),
            vec![(1, 101)],
            "the second intruder waits for another soldier"
        );
    }

    #[test]
    fn a_house_without_a_soldier_answers_nothing() {
        let base = (10, 10);
        let own = [blip(3, (11, 10), false)];
        let enemies = [blip(101, (14, 10), true)];

        assert!(plan_defense(base, &own, &enemies, 4).is_empty());
    }

    #[test]
    fn the_number_of_answers_is_bounded() {
        let base = (10, 10);
        let own: Vec<Blip> = (0..10).map(|i| blip(i, (11 + i as u16, 10), true)).collect();
        let enemies: Vec<Blip> = (0..10)
            .map(|i| blip(100 + i, (13, 11 + i as u16), true))
            .collect();

        assert_eq!(plan_defense(base, &own, &enemies, 3).len(), 3);
    }
}