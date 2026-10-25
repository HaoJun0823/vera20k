//! The retail brain: the orders and intents a player's hands would produce,
//! for whichever house the control layer drives.
//!
//! It is a player's brain, not the computer AI's. The house it drives is a
//! human house: `HouseClass::Update`'s AI blocks — the base plan, the
//! choosers, the team creation and the strategy tick — are gated on "no human
//! controls it" and never run for it, and this brain does not make them run.
//! So everything here is a command a player could have issued: a deploy, a
//! build, a placement, an order on one of the house's own objects.
//!
//! Three jobs, in this order each frame:
//!
//! - **Get a base started**: deploy the MCV, so the yard exists to build
//!   from, and buy another one back when the yard is gone ([`expand`]).
//! - **Spend the money**: place what the yard has finished, start the next
//!   structure, keep the factories on harvesters, then on what the house's own
//!   teams asked for (`Suggest_New_Object`'s slot) and finally on what the army
//!   is short of ([`build`], over the job weights in `rules::unit_role`). That
//!   module holds the economy; it is this brain's own, because the native one
//!   is a computer house's and comes with money the house has not earned.
//! - **Use what it has**: an engineer repairs the bridge whose hut is down at
//!   home or takes a building it may capture, a spy walks into the building
//!   worth most, a rifleman garrisons a building near the base, an army large
//!   enough is sent at the enemy's base ([`advance`]), and while it is not, an
//!   idle combat unit holds the base ([`orders`]).
//!
//! Determinism: the brain owns its generator and never draws from a
//! simulation stream, and every decision is an integer comparison over the
//! rules' arrays and the entity store in their own order.
//!
//! RESIDUALS:
//! - The push is one rule: army size, then the nearest enemy structure. It
//!   has no retreat, no regrouping and no wave timing, so a push that is
//!   losing keeps going. Each unit's actual fight is the sim's `AttackMove`
//!   and `Guard`.
//! - Who fights, measured (`autoplay-match --dump-teams`, pinned
//!   `Death.mmx`/`0x5eed`): this house never has a native `TeamClass` at all —
//!   `HouseClass::Update`'s team block is gated on the house not being human
//!   (`ai_team_creation::update_team_creation`), which is native behaviour and
//!   stays. The computer house does get teams, and their retail scripts run
//!   (action ids 54/58/5 were observed mid-match), but only about two at a
//!   time and roughly ten of its units are in them; none of its 165
//!   `AITriggerTypes` produced a completed team by tick 6000. So on this side
//!   this brain is the whole army, and no unit of ours is ever inside a team
//!   for the native script and this module to fight over.
//! - Three push variants were built, run end to end and reverted, because
//!   each lost the match the current rule wins at tick 25636: per-unit targets
//!   matched to a structure's defenders by `unit_role` weights (timeout at
//!   36000, enemy still at 4), the fist aimed from the army's own centre
//!   instead of its first fighter (timeout, enemy 4), and one target held
//!   until destroyed (timeout, enemy 11). The push is sensitive to the exact
//!   target sequence, so a change here needs a better model of *why* the
//!   current sequence works before it can be replaced.
//! - The army sees the whole map: fog honesty is still the open question
//!   `docs/autoplay.md` records. So does the purchase: the enemy's composition
//!   is read past the fog, and `unit_role` reads a type's warheads rather than
//!   the armour it will actually meet, so a type's range against the enemy's
//!   and what a turret can bring to bear are not in the ranking yet.
//! - Target selection is one predicate, not the native one: another house's
//!   object this house is at war with, nearest first, and only inside the
//!   base radius.
//! - A yard rebuilt from a bought MCV is not defended differently from the
//!   first one, and the brain does not repair a damaged structure, so a raid
//!   that reaches the base can take it a second time.
//! - With no building left standing, `advance` has no base centre to measure,
//!   and reads the house as not under threat, so the push runs: an army is the
//!   only thing such a house has. What it does not do is rebuild — the maker
//!   it needs can still be refused by `CanBuild`, and the recovery chain in
//!   [`expand`] cannot run without a factory to buy it from.

pub(super) mod advance;
pub(super) mod build;
pub(super) mod expand;
pub(super) mod orders;

use std::collections::BTreeMap;

use crate::rules::ruleset::RuleSet;
use crate::rules::unit_role::RoleTable;
use crate::sim::control::brain::{Brain, Outbox};
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;
use crate::sim::rng::SimRng;

use orders::SentOrder;

/// How long an order on a unit stands before the brain looks again, in frames.
/// Fifteen seconds at play cadence: long enough that a unit is not re-tasked
/// every frame, short enough that one that finished its attack and idled is
/// sent on.
pub(super) const ORDER_REFRESH_FRAMES: u64 = 450;

/// Plays one house the way a player would, through commands only.
pub struct RetailBrain {
    /// This brain's own generator. Nothing draws on it yet — every decision
    /// here is deterministic — but the stream stays the brain's own, never
    /// the world's.
    #[allow(dead_code)]
    pub(super) rng: SimRng,
    /// What each combat unit was sent at, and the frame that order goes stale.
    pub(super) orders: BTreeMap<u64, SentOrder>,
    /// How far the placement sweep has moved for a structure waiting for a
    /// cell, by type: a cell the gate admits but the receiver refuses must
    /// not be asked for again.
    pub(super) site_attempts: BTreeMap<InternedId, u32>,
    /// What each type is for, read out of the warheads once and never
    /// changed. Built on the first spend, because the rules a scenario plays
    /// with are the ones the brain is handed there.
    pub(super) roles: Option<RoleTable>,
}

impl RetailBrain {
    /// `seed` seeds this brain's own generator. It must not be a simulation
    /// stream: the brain's draws are not part of the world's random sequence.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SimRng::new(seed),
            orders: BTreeMap::new(),
            site_attempts: BTreeMap::new(),
            roles: None,
        }
    }

    /// The role table for `rules`: what each type is for, so the factories can
    /// be told what the army is short of rather than what is dearest.
    pub(super) fn roles(&mut self, rules: &RuleSet) -> &RoleTable {
        self.roles.get_or_insert_with(|| RoleTable::build(rules))
    }

    /// This brain's own draw, for the native decisions it asks.
    #[allow(dead_code)]
    pub(super) fn draw(&mut self, low: i32, high: i32) -> i32 {
        self.rng.next_range_i32_inclusive(low, high)
    }
}

impl Brain for RetailBrain {
    fn name(&self) -> &'static str {
        "retail-ai"
    }

    fn think(&mut self, rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
        expand::deploy_yard_maker(self, rules, obs, out);
        build::spend(self, rules, obs, out);
        // Specialists first, in the order a bridge at home, a spy's target and a
        // garrison matter more than another unit standing still; a unit takes
        // one order a frame, so whoever asks first keeps it.
        orders::bridges(self, rules, obs, out);
        orders::captures(self, rules, obs, out);
        orders::garrison(self, rules, obs, out);
        // A house that is pushing owns its whole army for the frame; one that
        // is not keeps holding its base.
        if !advance::advance(self, rules, obs, out) {
            orders::forces(self, rules, obs, out);
        }
    }
}
