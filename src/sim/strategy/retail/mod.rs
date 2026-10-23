//! The retail AI: the native base plan and the house AI's own choices,
//! delivered as orders and intents.
//!
//! Its order comes from `HouseClass__AI_RecalcBuildOptions` (`0x005054B0`), the
//! same generator the computer houses use, so a mod that extends the loaded
//! rules changes what it builds without touching VERA.
//!
//! **It owns its own memory.** The native AI keeps its plan and its generator
//! on the `HouseClass`; a human house's fields are never populated, and a brain
//! must not write them anyway — the computer AI owns them for computer houses.
//! So this brain holds a `BasePlanState` and a `SimRng` of its own and calls the
//! same pure generator on them. `recalc_base_plan` takes no `Simulation`, which
//! is what makes that possible without a refactor.
//!
//! One plan node at a time, like the yard: `AI_Choose_Building` (`0x004FE3E0`)
//! takes the first node `BaseClass::Find_Node` (`0x0042EB50`, `wanted = -1`)
//! reports open and returns at once while a choice waits
//! (`0x004FE3E9..0x004FE416`). The native yard turns that choice into a product
//! and places it; here each step is an intent instead:
//!
//! - no Construction Yard yet: deploy the unit whose `DeploysInto=` is
//!   `[AI] BuildConst=` (`UnitClass::Deploy`, `0x007393C0`) — [`expand`];
//! - a node is open: queue its type once the house may build it
//!   (`HouseClass::CanBuild`, `0x004F7870`); a node whose drain would take the
//!   house past its output gets the side's plant first (`AI_Choose_Building`'s
//!   splice, `0x004FE6F6..0x004FE943`), and the node stays open for the decision
//!   that follows — [`building_choice`];
//! - the product waits for placement: place it at the first cell the game
//!   admits ([`crate::sim::control::placement`]);
//! - the product has left the factory: the node counts filled, the way
//!   `BuildingClass::Unlimbo`'s `0x0042F260` pass records it;
//! - an idle unit factory asks for the combat unit the house's forming teams
//!   still need (`AI_Choose_Unit`'s Unit, Infantry and Aircraft branches,
//!   `0x004FEA60`) — [`unit_choice`]; a combat unit no team has taken holds the
//!   base and answers what comes into it, and every engineer takes the nearest
//!   capturable building — [`orders`];
//! - which half of the queue runs is the house's mode
//!   (`AI_ChooseNextProduction`'s dispatch, `HouseClass+0x1E4`), so below
//!   `[General] AIAlternateProductionCreditCutoff=` the house works one half at
//!   a time instead of letting the yard and the factories compete for the same
//!   credits (`AI_EconomyStateMachine`, `sim::ai_base_building`).
//!
//! Teams are the other half of the house's own AI: `HouseClass::Update`'s team
//! block never runs for a house this brain drives, because its gate is "no
//! human controls it". So when the house's team timer expires, [`teams`] asks
//! the house's own selector (`sim::ai_team_creation::plan_team_creation`) what
//! it would raise and returns it as a state intent; the control layer applies
//! it through that module — the same teams, eviction, `Autocreate=` marks and
//! timer restart, with this brain's draws. A raised team is then the sim's own:
//! `team_script_vm`'s pass has no house gate, so it recruits the house's units
//! (`TeamClass::Recruit`) and drives them by its script like any other house's.
//! The army's own order step therefore stands down for a recruited member
//! (`Observation::team_has_member`).
//!
//! The house's Strategy tick (`HouseClass::AI_Building_Strategy`,
//! `0x004FD500`) is the third block `HouseClass::Update` skips for this house,
//! and the one the other two were waiting on: it is what runs the enemy search
//! and so gives the house an enemy, and without one the team selector admits
//! base-defense triggers only — a house that raises a base guard and never an
//! attack. [`RetailBrain::state`] therefore asks for the tick too, and the
//! control layer applies it through `sim::house_strategy`: the enemy search,
//! the defeated-enemy forget, `AI_TryFireSW`, the emergency state with its fire
//! sale and all-to-hunt, the missing-factory reaction and
//! `AI_Manage_Build_Queue` all run from there, in one owner.
//!
//! **Not driven yet, and not faked** (see `docs/ai-layers.md`):
//! `AI_ChooseNextProduction` (`0x00506EF0`) base defenses. That one needs its
//! native owner split into a read-only decision and an apply half before a
//! brain may call it; until then a node whose prerequisites never appear waits
//! rather than being invented around.
//!
//! RESIDUALS:
//! - The army's *orders for the units no team has taken* are not native: a
//!   team's members are driven by their script, and the rest hold the base
//!   (area guard at the yard) and answer a hostile that comes within
//!   `[General] AISafeDistance=` of it; each engineer takes the nearest
//!   capturable building. The unit *choices* are native: the forming teams'
//!   task forces, drawn by `sim::ai_unit_choice`.
//! - The `forces` switch gates these orders only. A team's script is the sim's
//!   pass for the house, not a command, so switching `forces` off leaves a
//!   recruited member under its script.
//! - The army sees the whole map: the computer AI is omniscient, and fog
//!   honesty is still the open question `docs/autoplay.md` records.
//! - Target selection is one predicate, not the native one: another house's
//!   object this house is at war with (`sim::house_diplomacy`) and that is not
//!   `MultiplayPassive`, nearest first, and only inside the base radius. Native
//!   picks by team script, threat and value, so a play test should expect a
//!   garrison that fights what walks in rather than one that fights what
//!   matters.
//! - Combat units are requested only when the house can pay for them; the base
//!   plan itself has no such gate, because native's gate is
//!   `AI_Manage_Build_Queue`'s credit-cutoff mode, which is unported.
//! - A cell the placement gate admits but `Place_Production` then refuses is
//!   asked for again with a moved sweep; past `GIVE_UP_FRAMES` the product is
//!   abandoned and its node with it, where native replans.
//!
//! Determinism: the plan is generated from this brain's own generator, seeded
//! by its constructor, never from `Simulation::scenario_rng`. Decisions are
//! integer comparisons over an ordered plan, an ordered entity store and a
//! fixed cell scan.

pub(super) mod build_plan;
pub(super) mod building_choice;
pub(crate) mod economy;
pub(super) mod expand;
pub(super) mod orders;
pub(super) mod teams;
pub(super) mod unit_choice;

use std::collections::BTreeMap;

use crate::rules::ruleset::RuleSet;
use crate::sim::base_plan::BasePlanState;
use crate::sim::control::brain::{Brain, Outbox};
use crate::sim::control::intent::Intent;
use crate::sim::control::observation::Observation;
use crate::sim::rng::SimRng;

use building_choice::Build;
use orders::SentOrder;

/// How long a node or a placement may make no progress before the brain gives
/// up on it, in frames. Half a minute at play cadence: long enough that a slow
/// building or a momentarily blocked prerequisite is not abandoned, short
/// enough that a play test never watches a dead base.
pub(super) const GIVE_UP_FRAMES: u32 = 900;

/// How long an order on a unit stands before the brain looks again, in frames.
/// Fifteen seconds at play cadence: long enough that a unit is not re-tasked
/// every frame, short enough that one that finished its attack and idled is
/// sent on.
pub(super) const ORDER_REFRESH_FRAMES: u64 = 450;

/// Plays the retail base plan and the house AI's choices for whichever house
/// the control layer drives.
pub struct RetailBrain {
    pub(super) plan: BasePlanState,
    pub(super) rng: SimRng,
    pub(super) generated: bool,
    /// The product the yard has in hand, if any.
    pub(super) build: Option<Build>,
    /// The open node the plan is blocked on, and for how many frames.
    pub(super) blocked: Option<(usize, u32)>,
    /// Nodes this brain gave up on, so the rest of the plan still runs.
    pub(super) skipped: Vec<usize>,
    /// The node whose build was last reported blocked, so a stall is logged
    /// once instead of once per frame.
    pub(super) stall_logged: Option<usize>,
    /// What each combat unit was sent at, and the frame that order goes stale.
    pub(super) orders: BTreeMap<u64, SentOrder>,
}

impl RetailBrain {
    /// `seed` seeds this brain's own generator. It must not be a simulation
    /// stream: the brain's draws are not part of the world's random sequence.
    pub fn new(seed: u64) -> Self {
        Self {
            plan: BasePlanState::default(),
            rng: SimRng::new(seed),
            generated: false,
            build: None,
            blocked: None,
            skipped: Vec::new(),
            stall_logged: None,
            orders: BTreeMap::new(),
        }
    }

    /// The plan this brain holds, for diagnostics and tests.
    pub fn plan(&self) -> &BasePlanState {
        &self.plan
    }

    /// This brain's own draw, for the native decisions it asks: the choosers
    /// and the team selector both draw on the house's stream in play, and a
    /// brain's stream is its own generator.
    pub(super) fn draw(&mut self, low: i32, high: i32) -> i32 {
        self.rng.next_range_i32_inclusive(low, high)
    }
}

impl Brain for RetailBrain {
    fn name(&self) -> &'static str {
        "retail-ai"
    }

    /// The house's own Strategy tick and its team block, as this brain plays
    /// them.
    ///
    /// Strategy goes first and the team block follows: the tick's enemy search
    /// is what gives the house an enemy, and the team selector admits attack
    /// triggers only for a house that has one. Both are applied before `think`,
    /// because the units this frame's choosers ask for are the ones the teams
    /// just raised need.
    fn state(&mut self, rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
        if obs.strategy_due() {
            out.intent(Intent::AdvanceStrategy);
        }
        if let Some(intent) = teams::raise(self, rules, obs) {
            out.intent(intent);
        }
    }

    fn think(&mut self, rules: &RuleSet, obs: &Observation<'_>, out: &mut Outbox) {
        self.generate(rules, obs);
        if let Some(intent) = building_choice::place(self, rules, obs) {
            out.intent(intent);
        }
        expand::deploy_yard_maker(self, rules, obs, out);

        // `AI_ChooseNextProduction` dispatches on the house's mode, and that is
        // how a retail house keeps the yard and the factories from competing
        // for the same credits: below `AIAlternateProductionCreditCutoff=` the
        // economy state machine puts it on one half at a time, and the other
        // half only runs when this one has nothing to make.
        let mode = obs.production_mode();
        // The budget ([`economy`]) runs over both halves: a node costs several
        // times a unit, and the teams only fill if the factories can pay.
        let yard = economy::yard(rules, obs, obs.army_reserve(rules));
        if yard == economy::Yard::Run {
            economy::resume(obs, out);
        } else {
            economy::hold(obs, out);
        }
        let yard_may_build = yard == economy::Yard::Run;

        // Whichever half the mode picked runs the other one when it has nothing
        // to make, so a mode never idles the house.
        match mode {
            1 => {
                let built = yard_may_build && ask_building(self, rules, obs, out);
                if !built {
                    ask_units(self, rules, obs, out);
                }
            }
            2 => {
                if !ask_units(self, rules, obs, out) && yard_may_build {
                    ask_building(self, rules, obs, out);
                }
            }
            _ => {
                if yard_may_build {
                    ask_building(self, rules, obs, out);
                }
                ask_units(self, rules, obs, out);
            }
        }

        orders::engineers(self, rules, obs, out);
        orders::forces(self, rules, obs, out);
    }
}

/// The yard's half of the queue. `false` when it had nothing to ask for, which
/// is what makes the other half run in modes 1 and 2.
fn ask_building(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) -> bool {
    let Some(intent) = building_choice::queue(brain, rules, obs) else {
        return false;
    };
    out.intent(intent);
    true
}

/// The factories' half of the queue.
///
/// Income first: a harvester the house lacks is worth more than the first tank,
/// and both are the same factory's work.
fn ask_units(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) -> bool {
    if let Some(intent) = unit_choice::harvester(brain, rules, obs) {
        out.intent(intent);
        return true;
    }
    let Some(intent) = unit_choice::combat_unit(brain, rules, obs) else {
        return false;
    };
    out.intent(intent);
    true
}
