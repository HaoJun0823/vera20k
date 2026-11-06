//! What the retail team AI actually does over a match, read from the outside.
//!
//! Step two of the agreed AutoPlay order: validate the native team loop before
//! designing a writer interface. Everything here is read-only — the probe reads
//! `Simulation::team_script_vm()` and never creates, alters or disbands a team.
//!
//! Only six of the state's accessors exist in a release build: `id`, `owner`,
//! `team_type_id`, `script_id`, `cursor` and `members`. `formed`, `succeeded`,
//! `refusal` and `member_count` are behind `cfg(test)`, which is itself a finding:
//! the audit could not tell whether the loop closes because the guards that would
//! answer it are compiled out. This probe therefore reads what is available and
//! infers the rest from `cursor` progress and from ids leaving the array.
//!
//! Run through the project runner:
//!
//! ```text
//! python -m tools.cargo_run -- run --profile fast -p vera20k-core \
//!     --bin autoplay-teams -- <ra2-dir> Arena.mmx 20000
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

    let launch = battle_session(&map, &[(LaunchCountry::America, AiDifficulty::Easy)]);
    let mut scenario = match headless_scenario::load_session(&retail, &map, SEED, launch) {
        Ok(scenario) => scenario,
        Err(error) => {
            eprintln!("the map did not load: {error}");
            std::process::exit(1);
        }
    };

    println!("map={map} seed={SEED} ticks={ticks} sample_every={SAMPLE_EVERY}");
    println!(
        "{:>7}  {:>6}  {:>8}  {:>8}  {:>9}  {:>8}",
        "tick", "teams", "members", "scripted", "maxcursor", "typeful"
    );

    let mut seen: BTreeMap<u64, TeamTrace> = BTreeMap::new();
    let mut live_before: BTreeSet<u64> = BTreeSet::new();
    let mut left = 0_usize;

    for tick in 1..=ticks {
        scenario.tick();

        if tick % SAMPLE_EVERY != 0 {
            continue;
        }

        let vm = scenario.sim().team_script_vm();
        let mut live_now: BTreeSet<u64> = BTreeSet::new();
        let mut members = 0_usize;
        let mut scripted = 0_usize;
        let mut typeful = 0_usize;
        let mut max_cursor = -1_i32;

        for team in vm.teams_in_order() {
            let id = team.id();
            live_now.insert(id);
            members += team.members().count();
            // Every team carries a script id; what varies is whether it came from
            // a registered TeamType or was formed ad hoc.
            if team.team_type_id().is_some() {
                typeful += 1;
            }
            if team.script_id() != vera20k_core::sim::intern::InternedId::default() {
                scripted += 1;
            }
            max_cursor = max_cursor.max(team.cursor());

            let entry = seen.entry(id).or_insert_with(|| TeamTrace {
                first_tick: tick,
                ..TeamTrace::default()
            });
            entry.last_tick = tick;
            entry.last_cursor = team.cursor();
            entry.last_members = team.members().count();
            entry.had_type |= team.team_type_id().is_some();
            entry.max_cursor = entry.max_cursor.max(team.cursor());
        }

        for gone in live_before.difference(&live_now) {
            left += 1;
            if let Some(trace) = seen.get_mut(gone) {
                trace.left_at = Some(tick);
            }
        }
        live_before = live_now;

        println!(
            "{tick:>7}  {:>6}  {members:>8}  {scripted:>8}  {max_cursor:>9}  {typeful:>8}",
            live_before.len()
        );
    }

    let advanced = seen.values().filter(|t| t.max_cursor > 0).count();
    println!();
    println!("=== teams seen ===");
    println!("distinct ids        {}", seen.len());
    println!(
        "from a TeamType     {}",
        seen.values().filter(|t| t.had_type).count()
    );
    println!("advanced past action 0  {advanced}");
    println!("left the array      {left}");

    if !seen.is_empty() {
        println!();
        println!(
            "{:>8}  {:>9}  {:>9}  {:>9}  {:>8}  {:>7}  typed",
            "team", "first", "last", "left_at", "maxcur", "members"
        );
        for (id, trace) in &seen {
            println!(
                "{id:>8}  {:>9}  {:>9}  {:>9}  {:>8}  {:>7}  {}",
                trace.first_tick,
                trace.last_tick,
                trace
                    .left_at
                    .map_or_else(|| "-".to_string(), |t| t.to_string()),
                trace.max_cursor,
                trace.last_members,
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
    left_at: Option<u64>,
    max_cursor: i32,
    last_cursor: i32,
    last_members: usize,
    had_type: bool,
}
