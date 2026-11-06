//! What the retail team AI actually does over a match, read from the outside.
//!
//! Step two of the agreed order: establish whether the native team system can
//! carry AutoPlay's execution loop before anyone designs a TeamController or
//! writes a second task system. Everything here is read-only.
//!
//! Built with `--features telemetry`, which is what makes `succeeded`, `refusal`
//! and `member_count` visible outside the crate. Without it this probe can only
//! see that a team exists, which is exactly the reading that led the first run to
//! an inconclusive answer.
//!
//! What it reports per team, following the agreed observation list:
//!
//! | field | use |
//! |---|---|
//! | id | tracks one team across samples |
//! | first_tick | team age |
//! | last_change | last sample where anything about the team differed |
//! | members | whether recruitment finished |
//! | cursor | script progress, as an action index |
//! | max_cursor | high-water progress |
//! | succeeded | the script reached its end (action 49) |
//! | refusal | the first action this engine cannot run |
//! | left_at | the team left the array, so it was dispersed or died |
//! | typed | formed from a registered TeamType rather than ad hoc |
//!
//! A still cursor is deliberately *not* called stalled. It is reported next to
//! members and refusal so the three explanations - standing order, unmet
//! condition, recruitment never completed - stay distinguishable.
//!
//! ```text
//! python -m tools.cargo_run -- build --profile fast -p vera20k-core \
//!     --bin autoplay-teams --features telemetry
//! <resolved>/fast/autoplay-teams.exe <ra2-dir> Arena.mmx <ticks> [computers]
//! ```

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

use vera20k_core::headless_scenario;
use vera20k_core::headless_scenario::battle_session;
use vera20k_core::skirmish_launch::{AiDifficulty, LaunchCountry};

/// How often to look, in ticks.
const SAMPLE_EVERY: u64 = 500;

/// The benchmark seed this project's numbers were recorded at.
const SEED: u32 = 0x5EED;

/// The seven-house roster `sim_bench_match` uses, matched to it so a pressure run
/// here and a `sim-bench --scenario match` run are the same fight.
const COMPUTER_ROSTER: [(LaunchCountry, AiDifficulty); 7] = [
    (LaunchCountry::Russia, AiDifficulty::Hard),
    (LaunchCountry::France, AiDifficulty::Normal),
    (LaunchCountry::Yuri, AiDifficulty::Easy),
    (LaunchCountry::Iraq, AiDifficulty::Hard),
    (LaunchCountry::Korea, AiDifficulty::Normal),
    (LaunchCountry::Cuba, AiDifficulty::Easy),
    (LaunchCountry::GreatBritain, AiDifficulty::Hard),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let retail = args
        .first()
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("RA2_DIR").map(PathBuf::from))
        .unwrap_or_else(|| {
            eprintln!("give the retail directory as the first argument, or set RA2_DIR");
            std::process::exit(2);
        });
    let map = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "Arena.mmx".to_string());
    let ticks: u64 = args
        .get(2)
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(20_000);
    let computers: usize = args
        .get(3)
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(1)
        .clamp(1, COMPUTER_ROSTER.len());

    let launch = battle_session(&map, &COMPUTER_ROSTER[..computers]);
    let mut scenario = match headless_scenario::load_session(&retail, &map, SEED, launch) {
        Ok(scenario) => scenario,
        Err(error) => {
            eprintln!("the map did not load: {error}");
            std::process::exit(1);
        }
    };

    println!(
        "map={map} seed={SEED} ticks={ticks} computers={computers} sample_every={SAMPLE_EVERY}"
    );
    println!(
        "{:>7}  {:>6}  {:>8}  {:>9}  {:>9}  {:>8}  {:>7}",
        "tick", "teams", "members", "maxcursor", "succeeded", "refused", "left"
    );

    let mut seen: BTreeMap<u64, TeamTrace> = BTreeMap::new();
    let mut live_before: BTreeSet<u64> = BTreeSet::new();
    let mut left_total = 0_usize;

    for tick in 1..=ticks {
        scenario.tick();

        if tick % SAMPLE_EVERY != 0 {
            continue;
        }

        let vm = scenario.sim().team_script_vm();
        let mut live_now: BTreeSet<u64> = BTreeSet::new();
        let mut members = 0_usize;
        let mut max_cursor = -1_i32;
        let mut succeeding = 0_usize;
        let mut refused = 0_usize;

        for team in vm.teams_in_order() {
            let id = team.id();
            live_now.insert(id);
            let count = team.member_count();
            let cursor = team.cursor();
            let did_succeed = team.succeeded();
            let refusal = team.refusal();
            members += count;
            max_cursor = max_cursor.max(cursor);
            if did_succeed {
                succeeding += 1;
            }
            if refusal.is_some() {
                refused += 1;
            }

            let entry = seen.entry(id).or_insert_with(|| TeamTrace {
                first_tick: tick,
                last_change: tick,
                ..TeamTrace::default()
            });
            entry.last_tick = tick;
            entry.had_type |= team.team_type_id().is_some();
            if entry.max_cursor != cursor.max(entry.max_cursor)
                || entry.last_members != count
                || entry.was_succeeded != did_succeed
            {
                entry.last_change = tick;
            }
            entry.max_cursor = entry.max_cursor.max(cursor);
            entry.last_members = count;
            entry.was_succeeded |= did_succeed;
            if refusal.is_some() && entry.refusal_at.is_none() {
                entry.refusal_at = Some(tick);
            }
            entry.last_refusal = refusal;
        }

        for gone in live_before.difference(&live_now) {
            left_total += 1;
            if let Some(trace) = seen.get_mut(gone) {
                trace.left_at = Some(tick);
            }
        }
        live_before = live_now;

        println!(
            "{tick:>7}  {:>6}  {members:>8}  {max_cursor:>9}  {succeeding:>9}  {refused:>8}  {left_total:>7}",
            live_before.len()
        );
    }

    let advanced = seen.values().filter(|t| t.max_cursor > 0).count();
    let completed = seen.values().filter(|t| t.was_succeeded).count();
    let ever_refused = seen.values().filter(|t| t.refusal_at.is_some()).count();

    println!();
    println!("=== summary ===");
    println!("distinct teams      {}", seen.len());
    println!(
        "from a TeamType     {}",
        seen.values().filter(|t| t.had_type).count()
    );
    println!(
        "recruited to >= 4   {}",
        seen.values().filter(|t| t.last_members >= 4).count()
    );
    println!("advanced past 0     {advanced}");
    println!("script completed    {completed}");
    println!("ever refused        {ever_refused}");
    println!("left the array      {left_total}");

    if !seen.is_empty() {
        println!();
        println!(
            "{:>8}  {:>8}  {:>10}  {:>10}  {:>9}  {:>7}  {:>7}  {:>7}  note",
            "team", "first", "last", "lastchange", "maxcursor", "members", "ok", "typed",
        );
        for (id, trace) in &seen {
            let note = match (trace.last_refusal, trace.left_at) {
                (Some(refusal), _) => {
                    format!("refused at {}: {refusal:?}", trace.refusal_at.unwrap_or(0))
                }
                (None, Some(at)) => format!("left at {at}"),
                (None, None) if trace.was_succeeded => "completed, still in array".to_string(),
                (None, None) => {
                    let still = trace.last_tick.saturating_sub(trace.last_change);
                    format!("quiet for {still} ticks")
                }
            };
            println!(
                "{id:>8}  {:>8}  {:>10}  {:>10}  {:>9}  {:>7}  {:>7}  {:>7}  {note}",
                trace.first_tick,
                trace.last_tick,
                trace.last_change,
                trace.max_cursor,
                trace.last_members,
                trace.was_succeeded,
                trace.had_type,
            );
        }
    }
}

/// What one team looked like across the samples it appeared in.
#[derive(Debug, Default)]
struct TeamTrace {
    first_tick: u64,
    last_tick: u64,
    /// The last sample where the team's observable state differed from before.
    last_change: u64,
    left_at: Option<u64>,
    max_cursor: i32,
    last_members: usize,
    was_succeeded: bool,
    last_refusal: Option<vera20k_core::sim::team_script_vm::TeamScriptRefusal>,
    refusal_at: Option<u64>,
    had_type: bool,
}
