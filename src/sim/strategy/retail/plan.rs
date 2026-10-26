//! The build list, and the wallet that pays for it.
//!
//! What the base builds is one ordered list, read top to bottom, and what it
//! may spend is the wallet minus what the lines have already been promised.
//! Those were one question before this module: the order was a chain of
//! conditions, each one reading the wallet of the moment, and a queue is
//! money the wallet has not been charged for yet — a build starts owing its
//! `Cost_Of` and pays it down as it steps (`sim::production::factory`), so
//! queuing costs the house nothing today. The same credits were therefore
//! spent twice: a structure was started with money the army had already been
//! promised, and the wallet never got to the price of a tank.
//!
//! Three numbers come out of here:
//!
//! - **What the house earns per minute**, measured: two samples of
//!   [`Observation::earned`] a window apart. It is what turns the list into a
//!   budget — a structure that costs a minute's income is a decision, not a
//!   reflex, and the log line the brain writes every ten seconds says how the
//!   house is doing rather than only what it holds.
//! - **What is still free**: the wallet minus [`Census::committed`], the
//!   money no line has been promised yet. Every gate below reads that and
//!   none of them reads the balance.
//! - **Whether the base is short of power**, counting the plants still
//!   building at the output they will have when they finish
//!   ([`power_short`]). A plant under construction supplies its `Power=`
//!   scaled by the health it has built, while everything it feeds draws in
//!   full — read naively that is a brownout for as long as the plant is
//!   being built, and the answer it produced was a second plant: the opening
//!   became two plants and one refinery.
//!
//! Determinism: integer arithmetic over the rules' arrays in their own order,
//! and the meter is a ring of samples keyed by the frame they were taken at,
//! so the same replay gives the same rate.

use std::collections::VecDeque;

use crate::rules::object_type::ObjectType;
use crate::rules::ruleset::RuleSet;
use crate::sim::control::observation::Observation;
use crate::sim::intern::InternedId;
use crate::sim::production::ProductionCategory;

use super::build;
use super::census::Census;
use super::expand;

/// A minute at play cadence: the simulation steps 15 logic frames a second,
/// the rate every retail mode runs at, so this is the frame count a per-minute
/// figure is scaled over.
const FRAMES_PER_MINUTE: u64 = 900;

/// How far apart the meter takes its samples: three seconds. Close enough
/// that a rate is current, far enough that the ring stays short.
const SAMPLE_FRAMES: u64 = 45;

/// The shortest window the meter will extrapolate from: ten seconds. Under it
/// one deposit — or no deposit — is the entire sample, and a rate read off
/// that swings by thousands.
const MIN_WINDOW_FRAMES: u64 = 150;

/// How much history the meter keeps: one minute. A rate older than that is a
/// rate from a base that no longer stands.
const WINDOW_FRAMES: u64 = FRAMES_PER_MINUTE;

/// The house's income, measured rather than modelled.
///
/// A model would need the distance from the base to the ore, the harvester's
/// speed and how long a bay takes to unload — every one of them map- and
/// type-specific, and the answer is wrong in exactly the cases that matter (a
/// field that is running out, a bay that is too far away, a harvester that was
/// killed). The house's own books are simpler and always right: what it has
/// earned is the wallet plus everything `Spend_Money` has taken out of it.
pub(super) struct IncomeMeter {
    /// `(frame, earned)` samples, oldest first.
    samples: VecDeque<(u64, i32)>,
}

impl IncomeMeter {
    pub(super) fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    /// Take a sample of `earned` at `frame`, if the last one is old enough.
    pub(super) fn sample(&mut self, frame: u64, earned: i32) {
        if self
            .samples
            .back()
            .is_some_and(|(at, _)| frame.wrapping_sub(*at) < SAMPLE_FRAMES)
        {
            return;
        }
        self.samples.push_back((frame, earned));
        while self.samples.len() > 2
            && frame.wrapping_sub(self.samples.front().map_or(0, |(at, _)| *at)) > WINDOW_FRAMES
        {
            self.samples.pop_front();
        }
    }

    /// Credits per minute over the window the meter holds, or `None` before
    /// the window is long enough to extrapolate from.
    pub(super) fn per_minute(&self) -> Option<i32> {
        let (first_frame, first_earned) = *self.samples.front()?;
        let (last_frame, last_earned) = *self.samples.back()?;
        let frames = last_frame.wrapping_sub(first_frame);
        if frames < MIN_WINDOW_FRAMES {
            return None;
        }
        let earned = i64::from(last_earned.wrapping_sub(first_earned));
        let window = i64::try_from(frames).unwrap_or(i64::MAX);
        let minute = i64::try_from(FRAMES_PER_MINUTE).unwrap_or(i64::MAX);
        i32::try_from(earned * minute / window).ok()
    }
}

/// What the wallet can actually spend this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Budget {
    /// What the lines have been promised and have not paid out yet.
    pub(super) committed: i32,
    /// The wallet minus the commitment.
    pub(super) free: i32,
    /// Credits per minute, when the meter has a window.
    pub(super) income: Option<i32>,
}

/// Read the budget: the ledger's own commitment, and the wallet beside it.
pub(super) fn budget(
    rules: &RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    income: Option<i32>,
) -> Budget {
    let committed = census.committed(rules, obs);
    Budget {
        committed,
        free: obs.credits().wrapping_sub(committed),
        income,
    }
}

/// One kind of structure the list asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Item {
    Refinery,
    Barracks,
    Weapons,
    Defense,
    Radar,
    Tech,
    /// The plant an `Aircraft` line needs, read off `Factory=` rather than off
    /// a name list.
    Aircraft,
    /// What the yard maker's `Prerequisite=` list asks for: the way back from
    /// a lost yard.
    Support,
}

/// When a step of the list is allowed to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Gate {
    Always,
    /// The bays have the harvesters that keep them busy
    /// ([`build::HARVESTERS_PER_REFINERY`]): a second bay before the first
    /// one's harvesters is a bay with nothing to unload.
    HarvestersServed,
    /// The free wallet is past this. Throughput and tech are luxuries the base
    /// pays for out of a surplus, never out of the army's money.
    Free(i32),
    /// The base can replace what a push spends: a barracks and a war factory
    /// both stand.
    CanReplace,
    /// The tech stage: a surplus and an army worth unlocking types for.
    Tech,
}

/// One entry of the build list: what is wanted, how many, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Step {
    pub(super) item: Item,
    pub(super) want: usize,
    pub(super) gate: Gate,
}

/// The list, in the order a player builds a base.
///
/// Power is not an entry: it is spliced in ahead of whatever the list answers
/// ([`power_short`]), the way a player does it, because a plant is not a step
/// towards anything — it is what lets the other steps run.
pub(super) const STEPS: &[Step] = &[
    // Income. The first bay is what an opening is made of.
    Step {
        item: Item::Refinery,
        want: 1,
        gate: Gate::Always,
    },
    // The second waits for the harvesters that fill it: past this the ore
    // within reach of the base is the limit, not the bays.
    Step {
        item: Item::Refinery,
        want: 2,
        gate: Gate::HarvestersServed,
    },
    // The barracks before the war factory: it is the cheaper structure and
    // infantry is the cheapest army a house can hold ground with.
    Step {
        item: Item::Barracks,
        want: 1,
        gate: Gate::Always,
    },
    // One war factory: the harvesters, the yard maker and the army all come
    // out of it.
    Step {
        item: Item::Weapons,
        want: 1,
        gate: Gate::Always,
    },
    // Something to hold the base with, before anything that only makes it
    // grow: a radar is worth less than a base that survives the first raid.
    Step {
        item: Item::Defense,
        want: 2,
        gate: Gate::CanReplace,
    },
    // Throughput, once the wallet says the factories cannot spend it. Every
    // extra factory shortens a build by `[General] MultipleFactory=`.
    Step {
        item: Item::Weapons,
        want: 3,
        gate: Gate::Free(build::SURPLUS_CREDITS),
    },
    Step {
        item: Item::Barracks,
        want: 2,
        gate: Gate::Free(build::SURPLUS_CREDITS),
    },
    // Tech: the plant an aircraft line needs, then the radar, then the lab.
    //
    // Ahead of the support building, because the list answers with the first
    // step that is short and open: both gates open on the same wallet, so with
    // the support building first the house spent its surplus on a repair depot
    // every time and never got as far as the plant its aircraft need — the tech
    // stage that never came.
    Step {
        item: Item::Aircraft,
        want: 1,
        gate: Gate::Tech,
    },
    Step {
        item: Item::Radar,
        want: 1,
        gate: Gate::Tech,
    },
    Step {
        item: Item::Tech,
        want: 1,
        gate: Gate::Tech,
    },
    // The way back from a lost yard, bought while the yard still stands: last,
    // because it is insurance against a loss the house has not had.
    Step {
        item: Item::Support,
        want: 1,
        gate: Gate::Free(build::SUPPORT_MIN_CREDITS),
    },
];

/// The structure the list wants now: the first entry whose gate is open and
/// whose count is short, in list order.
pub(super) fn next<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    budget: &Budget,
) -> Option<(&'r ObjectType, InternedId)> {
    STEPS
        .iter()
        .filter(|step| open(step.gate, rules, obs, census, budget))
        .filter(|step| owned(rules, obs, census, step.item) < step.want)
        .find_map(|step| pick(rules, obs, census, step.item))
}

/// Whether the base needs another plant, counting the plants that are still
/// building at the output they will have when they finish.
pub(super) fn power_short(rules: &RuleSet, obs: &Observation<'_>, census: &Census) -> bool {
    planned_output(rules, obs, census).wrapping_sub(obs.power_drain()) < 0
}

/// Output now, plus the output of every plant that is on a line, finished and
/// waiting for a cell, or standing unfinished.
///
/// A plant under construction supplies `Power=` scaled by the health it has
/// built (`sim::power_system`), and a plant that has finished but has not been
/// placed supplies nothing at all. Both are counted here at what they will
/// supply, because the question is not "is the base dark this frame" — the
/// simulation's own answer to that is the brownout — but "does the base need
/// another plant", which is a question about the plant it is already paying
/// for.
pub(super) fn planned_output(rules: &RuleSet, obs: &Observation<'_>, census: &Census) -> i32 {
    let mut output = obs.power_output();
    for line in &census.lines {
        for type_id in [line.product, line.ready].into_iter().flatten() {
            if let Some(ty) = obs.object_type(rules, type_id) {
                output = output.wrapping_add(ty.power.max(0));
            }
        }
    }
    for type_id in &census.ready {
        if let Some(ty) = obs.object_type(rules, *type_id) {
            output = output.wrapping_add(ty.power.max(0));
        }
    }
    output
}

fn open(
    gate: Gate,
    rules: &RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    budget: &Budget,
) -> bool {
    match gate {
        Gate::Always => true,
        Gate::HarvestersServed => {
            census.harvesters >= build::HARVESTERS_PER_REFINERY * census.refineries.max(1)
        }
        Gate::Free(credits) => budget.free >= credits,
        Gate::CanReplace => build::can_replace_units(rules, obs, census),
        Gate::Tech => {
            budget.free >= build::TECH_MIN_CREDITS && census.fighters >= build::ARMY_FOR_TECH
        }
    }
}

/// How many of `item` the house has.
fn owned(rules: &RuleSet, obs: &Observation<'_>, census: &Census, item: Item) -> usize {
    match item {
        Item::Refinery => census.refineries,
        Item::Barracks => build::count_in(obs, rules, census, &rules.build_barracks_types),
        Item::Weapons => build::count_in(obs, rules, census, &rules.build_weapons_types),
        Item::Defense => build::count_in(obs, rules, census, build::base_defenses(rules, obs)),
        Item::Radar => build::count_in(obs, rules, census, &rules.build_radar_types),
        Item::Tech => build::count_in(obs, rules, census, &rules.build_tech_types),
        // A house that owns a plant of the category has nothing to build, and
        // one that cannot build any has no business asking: both read as
        // owned, which is the answer that stops the step repeating.
        Item::Aircraft => usize::from(
            build::line_factory(rules, obs, census, ProductionCategory::Aircraft).is_none(),
        ),
        // The support building is the yard maker's own prerequisite list, so
        // whether one is wanted is that list's answer, not a count's.
        Item::Support => 0,
    }
}

fn pick<'r>(
    rules: &'r RuleSet,
    obs: &Observation<'_>,
    census: &Census,
    item: Item,
) -> Option<(&'r ObjectType, InternedId)> {
    match item {
        Item::Refinery => build::first_buildable(rules, obs, &rules.build_refinery_types),
        Item::Barracks => build::first_buildable(rules, obs, &rules.build_barracks_types),
        Item::Weapons => build::first_buildable(rules, obs, &rules.build_weapons_types),
        Item::Defense => build::first_buildable(rules, obs, build::base_defenses(rules, obs)),
        Item::Radar => build::first_buildable(rules, obs, &rules.build_radar_types),
        // The lab that authorises a power the house will actually fire.
        Item::Tech => build::tech_building(rules, obs),
        Item::Aircraft => build::line_factory(rules, obs, census, ProductionCategory::Aircraft),
        Item::Support => expand::yard_maker_support(rules, obs, census),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::world::Simulation;

    /// The retail rival arena: real object types, from the repository's own
    /// `ini/` snapshot, so this holds without a retail install.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    /// Two samples ten seconds apart, ten credits earned in between: that is
    /// sixty a minute. The wallet alone would have read it as nothing — the
    /// ten credits went straight back out to the line that was building.
    #[test]
    fn the_meter_reads_income_the_wallet_does_not_show() {
        let mut meter = IncomeMeter::new();
        // 0 s: 1000 in the wallet, 0 paid out.
        meter.sample(0, 1_000);
        assert_eq!(meter.per_minute(), None, "one sample is not a rate");
        // 10 s: the wallet is unchanged, but 100 has been spent, so the house
        // earned 100.
        meter.sample(150, 1_100);
        assert_eq!(meter.per_minute(), Some(600));
    }

    /// A window older than a minute is dropped: a rate from a base that no
    /// longer stands is not the house's income now.
    #[test]
    fn the_meter_forgets_more_than_a_minute() {
        let mut meter = IncomeMeter::new();
        meter.sample(0, 0);
        meter.sample(2_000, 2_000);
        let rate = meter.per_minute().expect("a rate");
        assert!(
            rate < 1_000,
            "the oldest sample is gone, so the rate is over the last minute at most: {rate}"
        );
    }

    /// What a line has been promised is spent already: a build starts owing
    /// its `Cost_Of` and pays it down as it steps, so the wallet still holds
    /// money a decision may not spend a second time.
    #[test]
    fn a_promised_build_is_money_the_wallet_may_not_spend_again() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let mut census = Census::default();
        let plant = sim.interner().get("GAPOWR").expect("the plant's id");
        census.lines[crate::sim::strategy::retail::census::LINE_BUILDING].product = Some(plant);
        let obs = Observation::new(&sim, owner);
        let ty = rules.object("GAPOWR").expect("the plant");

        let budget = budget(&rules, &obs, &census, None);
        assert_eq!(budget.committed, obs.cost_of(&rules, ty));
        assert_eq!(budget.free, obs.credits().wrapping_sub(budget.committed));
    }
}
