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
//!   current sequence works before it can be replaced. The **order** is the
//!   part that was changed since: the fist keeps one target, and each unit is
//!   told to attack it from where it stands when the structure is already
//!   inside its own weapon's reach, and to attack-move otherwise. That is not
//!   one of the three: the failure they shared was dispersion.
//! - Only one managed routine is attached: `Retreat`, for a fighting unit
//!   under a third of its strength. `AirSplit` and `SpyApproach` are not,
//!   and the reason is the same for both — a routine that is its object's
//!   only issuer would take an aircraft off the push and a spy off the
//!   [`orders`] capture step, and the attach request carries no preference to
//!   put in their place. Both need the request to carry a target before they
//!   can be engaged without regressing the push.
//! - The ledger ([`census`]) is rebuilt every frame rather than cached across
//!   frames: a cached count that lags the entity store is a decision made on
//!   a stale army. The vectors keep their capacity, so a steady army costs no
//!   allocation; the cost is one walk of the entity store and one of each
//!   house's factories per frame.
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
pub(super) mod census;
pub(super) mod composition;
pub(super) mod expand;
pub(super) mod micro;
pub(super) mod orders;
pub(super) mod plan;
pub(super) mod squads;
pub(super) mod tracked;

use std::collections::{BTreeMap, BTreeSet};

use crate::rules::ruleset::RuleSet;
use crate::rules::unit_role::RoleTable;
use crate::sim::control::brain::{Brain, Outbox};
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;
use crate::sim::rng::SimRng;

use census::Census;
use tracked::Tracker;

/// How long an order on a unit stands before the brain looks again is
/// [`tracked::TrackedKind::term`]'s answer now, and it differs per kind: a pose
/// takes its animation, a walk across the map takes the walk. Fifteen seconds
/// for the standing orders — long enough that a unit is not re-tasked every
/// frame, short enough that one which finished its attack and idled is sent on.

/// Plays one house the way a player would, through commands only.
pub struct RetailBrain {
    /// This brain's own generator. Nothing draws on it yet — every decision
    /// here is deterministic — but the stream stays the brain's own, never
    /// the world's.
    #[allow(dead_code)]
    pub(super) rng: SimRng,
    /// What each unit has already been told, and when it may be told again.
    ///
    /// Every order the brain issues goes through it — this is the L4 layer, and
    /// the reason a toggle cannot undo itself and a walk-in is not restarted
    /// every frame. One owner for "does it need telling", whatever asked.
    pub(super) tracked: Tracker,
    /// The army's teams: what each unit is for, and what it is held to. The
    /// layer between "push there" and "walk here" — rebuilt from the ledger on
    /// its own term, so a dead unit simply is not in the next count.
    pub(super) squads: squads::Squads,
    /// How far the placement sweep has moved for a structure waiting for a
    /// cell, by type: a cell the gate admits but the receiver refuses must
    /// not be asked for again.
    pub(super) site_attempts: BTreeMap<InternedId, u32>,
    /// The entry of the build list the house has decided on, as its index in
    /// [`plan::STEPS`], and the frame the decision may be re-made: the house
    /// saves for what it chose instead of spending the savings on whichever
    /// entry a flickering gate answers next.
    pub(super) plan_hold: Option<(usize, u64)>,
    /// The credits the yard is still short of for the structure it has settled
    /// on. Held back from the factories: a house that decides on a lab and then
    /// spends the lab's money on one more soldier every frame is a house that
    /// never reaches the lab.
    pub(super) yard_reserve: i32,
    /// The buildings whose wrench the house has switched on. The click is a
    /// toggle, so a second one turns the repair off: a building is remembered
    /// here until it is whole again.
    pub(super) repairing: BTreeSet<u64>,
    /// The buildings the house has seen standing whole. A structure still going
    /// up is at a fraction of its strength by definition, and repairing it would
    /// spend credits on a build that is already paid for.
    pub(super) repaired: BTreeSet<u64>,
    /// What each type is for, read out of the warheads once and never
    /// changed. Built on the first frame, because the rules a scenario plays
    /// with are the ones the brain is handed there.
    pub(super) roles: Option<RoleTable>,
    /// The house's own ledger: what it has, what its lines hold, and what the
    /// enemy's field asks for. Rebuilt at the top of every `think`, and the one
    /// place a decision reads "how many of mine" from. Kept inside the module
    /// the ledger's own types are visible in, so a field cannot promise more
    /// than its type can be read with.
    census: Census,
    /// When each fired super may be tried again, by type. The charge is the
    /// real gate; this only keeps a refused launch from spending the frame
    /// budget every frame it stays lit.
    pub(super) super_at: BTreeMap<InternedId, u64>,
    /// The house's income, measured: samples of `Observation::earned` a window
    /// apart. Not a simulation stream and not the brain's generator — it is a
    /// reading of the house's own books, kept here because the brain is the
    /// only thing that asks how fast the house is earning.
    income: plan::IncomeMeter,
}

impl RetailBrain {
    /// `seed` seeds this brain's own generator. It must not be a simulation
    /// stream: the brain's draws are not part of the world's random sequence.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SimRng::new(seed),
            tracked: Tracker::new(),
            squads: squads::Squads::new(),
            site_attempts: BTreeMap::new(),
            plan_hold: None,
            yard_reserve: 0,
            repairing: BTreeSet::new(),
            repaired: BTreeSet::new(),
            roles: None,
            census: Census::default(),
            super_at: BTreeMap::new(),
            income: plan::IncomeMeter::new(),
        }
    }

    /// The role table for `rules`: what each type is for, so the factories can
    /// be told what the army is short of rather than what is dearest.
    pub(super) fn roles(&mut self, rules: &RuleSet) -> &RoleTable {
        self.roles.get_or_insert_with(|| RoleTable::build(rules))
    }

    /// Rebuild this frame's ledger. The role table is built here on the first
    /// frame, because the rules a scenario plays with are the ones the brain is
    /// handed there.
    fn rebuild_census(&mut self, rules: &RuleSet, obs: &Observation<'_>) {
        if self.roles.is_none() {
            self.roles = Some(RoleTable::build(rules));
        }
        // The two halves are separate fields, so the ledger can be written
        // while the table it reads is borrowed.
        let Self { census, roles, .. } = self;
        let Some(roles) = roles.as_ref() else {
            return;
        };
        census.refresh(roles, rules, obs);
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
        // One pass over the house's objects and factories, before any decision
        // asks a question about them. Every step below reads this ledger rather
        // than walking the entity store again.
        self.income.sample(obs.tick(), obs.earned());
        self.rebuild_census(rules, obs);
        // What each unit is *for*, before anything decides what it does: the
        // membership is rebuilt on its own term, not every frame.
        self.squads.regroup(obs.tick(), &self.census);
        // What the wallet can still spend, and what the house earns: the two
        // numbers every step below is gated on, so they are read once.
        let budget = plan::budget(rules, obs, &self.census, self.income.per_minute());
        // One line every ten seconds, the same cadence the board and the yard
        // deploy log on: "the AI builds nothing" has to be answerable from the
        // log the client already writes.
        if obs.tick() % 600 == 0 {
            log::info!(
                "auto play: {} income={}/min",
                self.census.describe(),
                budget.income.map_or(-1, |rate| rate)
            );
        }
        expand::deploy_yard_maker(self, rules, obs, out);
        build::spend(self, rules, obs, &budget, out);
        // Units after the structures: the yard's next choice decides what the
        // house can make, and the lines fill once nothing is waiting to be
        // placed.
        composition::fill_lines(self, rules, obs, &budget, out);
        // A hurt unit is the retreat routine's before it is any order
        // step's: the attach rides the same outbox as the orders below, and
        // the control layer drops this brain's order for an object a routine
        // has just taken.
        orders::managed(self, obs.tick(), out);
        // A building under fire is worth less than the next one the yard makes,
        // and the wrench is the only thing that keeps it: it is a house-scoped
        // click, so it is asked before the units take their frame.
        orders::repairs(self, rules, obs, out);
        // A charged power is worth more than any single order below: it is the
        // one decision here that can take a base apart, and the button it fires
        // through is the sidebar's, not a unit's.
        orders::supers(self, rules, obs, out);
        // Specialists first, in the order a bridge at home, a spy's target and a
        // garrison matter more than another unit standing still; a unit takes
        // one order a frame, so whoever asks first keeps it.
        orders::bridges(self, rules, obs, out);
        orders::captures(self, rules, obs, out);
        orders::garrison(self, rules, obs, out);
        // What each unit shoots *this second*, before the march is decided: a
        // hostile in reach outranks a walk towards a target, and this is the
        // only layer that asks.
        micro::run(self, rules, obs, out);
        // A house that is pushing owns its whole army for the frame; one that
        // is not keeps holding its base.
        if !advance::advance(self, rules, obs, out) {
            orders::forces(self, rules, obs, out);
        }
        // Last, because it is a pose and not a destination: a soldier takes its
        // deployed position where the orders above have left it, and the toggle
        // is issued on the edge only.
        orders::deploy(self, obs, out);
    }
}
