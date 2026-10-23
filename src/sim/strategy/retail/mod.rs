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
//!   from ([`expand`]).
//! - **Spend the money**: place what the yard has finished, start the next
//!   structure, keep the factories on harvesters and then on soldiers
//!   ([`build`]). That module holds the economy; it is this brain's own,
//!   because the native one is a computer house's and comes with money the
//!   house has not earned.
//! - **Use what it has**: engineers take what they can capture, an army large
//!   enough is sent at the enemy's base ([`advance`]), and while it is not,
//!   an idle combat unit holds the base ([`orders`]).
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
//! - The army sees the whole map: fog honesty is still the open question
//!   `docs/autoplay.md` records.
//! - Target selection is one predicate, not the native one: another house's
//!   object this house is at war with, nearest first, and only inside the
//!   base radius.

pub(super) mod advance;
pub(super) mod build;
pub(super) mod expand;
pub(super) mod orders;

use std::collections::BTreeMap;

use crate::rules::ruleset::RuleSet;
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
}

impl RetailBrain {
    /// `seed` seeds this brain's own generator. It must not be a simulation
    /// stream: the brain's draws are not part of the world's random sequence.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SimRng::new(seed),
            orders: BTreeMap::new(),
            site_attempts: BTreeMap::new(),
        }
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
        expand::deploy_yard_maker(rules, obs, out);
        build::spend(self, rules, obs, out);
        orders::engineers(self, rules, obs, out);
        // A house that is pushing owns its whole army for the frame; one that
        // is not keeps holding its base.
        if !advance::advance(self, rules, obs, out) {
            orders::forces(self, rules, obs, out);
        }
    }
}
