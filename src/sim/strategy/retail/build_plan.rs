//! The native base plan: generating it once, and pairing it with what already
//! stands.
//!
//! `HouseClass__AI_RecalcBuildOptions` (`0x005054B0`) walks the side's
//! `[AI] BuildPower`/`BuildRefinery`/… order and fills the plan from the loaded
//! rules; [`crate::sim::base_plan_generation::recalc_base_plan`] is that port,
//! and it takes no `Simulation` — which is why a brain can call it on its own
//! `BasePlanState` without touching the house the computer AI owns.
//!
//! Two things surround the call and belong to the takeover, not to native:
//!
//! - **the anchor.** Natively the Recalc runs when a Construction Yard appears
//!   (the MCV deploy, `0x00739926`), and that pass anchors node zero at the
//!   yard's cell, which is what makes the node that asked for the yard count as
//!   satisfied by the yard itself. Same here.
//! - **the adoption.** A brain installed part-way through a match did not watch
//!   the base appear, so taking a house over must continue its base rather than
//!   build it again: every open node is paired with a standing building of its
//!   type, in plan order and entity order. Native has no such pass because the
//!   AI plan is persisted with the game and a computer house is never taken over
//!   mid-match.

use crate::rules::object_type::ObjectCategory;
use crate::rules::ruleset::RuleSet;
use crate::sim::base_plan::pack_base_plan_cell;
use crate::sim::base_plan_generation::recalc_base_plan;
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;
use crate::sim::strategy::retail::RetailBrain;

impl RetailBrain {
    /// Generate the native plan once, from this brain's own generator, and
    /// adopt whatever the house has already built.
    pub(super) fn generate(&mut self, rules: &RuleSet, obs: &Observation<'_>) {
        if self.generated {
            return;
        }
        let Some(anchor) = self.anchor(rules, obs) else {
            return;
        };
        let (Some(country), Some(side), Some(difficulty), Some(tech_level)) = (
            obs.country(),
            obs.side_index(),
            obs.difficulty(),
            obs.tech_level(),
        ) else {
            return;
        };
        recalc_base_plan(
            &mut self.plan,
            rules,
            obs.type_tech_levels(),
            &country,
            side,
            difficulty,
            tech_level,
            obs.super_weapons(),
            &mut self.rng,
        );
        self.plan.anchor_first_node(pack_base_plan_cell(
            i32::from(anchor.0),
            i32::from(anchor.1),
        ));
        self.adopt_existing_base(rules, obs);
        self.generated = true;
    }

    /// Pair each open node with a standing building of its type, so a takeover
    /// starts where the base is rather than from the first node.
    fn adopt_existing_base(&mut self, rules: &RuleSet, obs: &Observation<'_>) {
        let mut taken: Vec<u64> = Vec::new();
        for index in 0..self.plan.nodes().len() {
            let node = self.plan.nodes()[index];
            if node.type_or_control < 0 || node.packed_cell != 0 {
                continue;
            }
            let Some(entity) = obs.own_entities().find(|entity| {
                !taken.contains(&entity.entity_id)
                    && obs.object_type(rules, entity.type_id).is_some_and(|ty| {
                        ty.category == ObjectCategory::Building
                            && ty.base_plan_type_index == node.type_or_control
                    })
            }) else {
                continue;
            };
            taken.push(entity.entity_id);
            self.plan.assign_site(
                index,
                pack_base_plan_cell(i32::from(entity.cell.0), i32::from(entity.cell.1)),
            );
        }
    }

    /// The cell of the house's Construction Yard, which the plan anchors on.
    /// `None` while the house has none: natively that is the house with no
    /// base, whose plan cannot be worked either.
    pub(super) fn anchor(&self, rules: &RuleSet, obs: &Observation<'_>) -> Option<(u16, u16)> {
        obs.own_entities()
            .find(|entity| {
                obs.object_type(rules, entity.type_id)
                    .is_some_and(|ty| ty.build_const_eligible)
            })
            .map(|entity| entity.cell)
    }

    /// The rules' building type a plan node names, by its `BasePlan` registry
    /// index. `building_ids` is the rules' registration order, which is
    /// deterministic.
    pub(super) fn node_type<'r>(
        &self,
        rules: &'r RuleSet,
        obs: &Observation<'_>,
        node_type_or_control: i32,
    ) -> Option<(&'r crate::rules::object_type::ObjectType, InternedId)> {
        for name in &rules.building_ids {
            let Some(object) = rules.object_in_category(ObjectCategory::Building, name) else {
                continue;
            };
            if object.base_plan_type_index != node_type_or_control {
                continue;
            }
            let Some(type_id) = obs.type_id(name) else {
                continue;
            };
            return Some((object, type_id));
        }
        None
    }
}
