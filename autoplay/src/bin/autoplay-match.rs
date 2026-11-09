//! Run one real match through AutoPlay's observation layer and print its ledgers.
//!
//! This is the autoplay crate's first executable entry point. It does not plan or command
//! anything: it loads a retail map as a real battle, takes a Snapshot every frame, folds
//! each one into the Ledger, and reports the flow and stock a live match produced. That is
//! the piece a play-test needs before a planner can be judged - the numbers have to come
//! from a running game, not from a hand-built test scene.
//!
//! The game client does not depend on this crate yet, so this binary is the runnable build
//! for now. It issues at most one Guard order per frame, through Arbiter and Executor,
//! to give a unit without a task one. That is the smallest honest task loop: no strategy
//! and no target selection, just "hold your post" - the one behaviour v2's measured runs
//! support. Everything else in the match still runs as the native AI plays it.
//!
//! ```text
//! python -m tools.cargo_run -- build --profile fast -p vera20k-autoplay --bin autoplay-match
//! <resolved>/fast/autoplay-match.exe <ra2-dir> Arena.mmx 600
//! ```

use std::path::PathBuf;

use vera20k_autoplay::arbiter::{Admission, Arbiter};
use vera20k_autoplay::control::{ControlState, Subsystem};
use vera20k_autoplay::executor::Executor;
use vera20k_autoplay::idle::{TaskBoard, lapsed};
use vera20k_autoplay::ledger::Ledger;
use vera20k_autoplay::world::Snapshot;
use vera20k_core::headless_scenario;
use vera20k_core::sim::command::Command;
use vera20k_core::skirmish_launch::{AiDifficulty, LaunchCountry};

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
    let ticks: u64 = args.get(2).and_then(|raw| raw.parse().ok()).unwrap_or(600);

    let launch = headless_scenario::battle_session(
        &map,
        &[
            (LaunchCountry::America, AiDifficulty::Easy),
            (LaunchCountry::Russia, AiDifficulty::Easy),
        ],
    );
    let mut scenario = match headless_scenario::load_session(&retail, &map, 0x5EED, launch) {
        Ok(scenario) => scenario,
        Err(error) => {
            eprintln!("the map did not load: {error}");
            std::process::exit(1);
        }
    };
    let Some(house) = scenario.sim().session().current_house() else {
        eprintln!("the launch seated no local house");
        std::process::exit(1);
    };

    println!("map={map} ticks={ticks} house={house:?}");
    println!(
        "houses={} objects={}",
        scenario.sim().houses().iter().count(),
        scenario.sim().entities().len()
    );

    // An enabled state is the lens for this run. There is no client switch yet, so the
    // binary drives the task loop itself to prove it against a live match.
    let control = ControlState::enabled(house);
    let mut ledger = Ledger::new();
    let mut tasks = TaskBoard::new();
    let mut assignments = 0usize;
    for frame in 0..ticks {
        let snapshot = Snapshot::take(scenario.sim(), &scenario.runtime.resources.rules, house);
        ledger.observe(&snapshot);

        // Close tasks whose unit can no longer carry them, with a named reason.
        for (unit, end) in lapsed(&snapshot, &control, &tasks) {
            tasks.finish(unit, end);
        }

        // Give one unit without a task something to do, through the same arbiter and
        // executor a client would use. One order per frame keeps the frame advance in
        // one place; the remaining units are picked up on later frames.
        let next = tasks
            .unassigned(&snapshot, &control)
            .into_iter()
            .find_map(|unit| {
                let order = Command::Guard {
                    entity_id: unit,
                    target: None,
                };
                match Arbiter::admit(scenario.sim(), &control, Subsystem::Combat, unit, order) {
                    Admission::Allowed(validated) => Some(validated),
                    Admission::Refused(_) => None,
                }
            });

        match next {
            Some(validated) => {
                let subject = validated.subject;
                if Executor::submit(&mut scenario.runtime, &validated).is_committed() {
                    tasks.assign(subject);
                    assignments += 1;
                }
            }
            None => {
                let _ = Executor::submit_idle(&mut scenario.runtime);
            }
        }

        if (frame + 1) % 150 == 0 {
            println!(
                "  frame {}: produced={} destroyed={} transferred={} active={} ended={:?}",
                frame + 1,
                ledger.produced().values().sum::<usize>(),
                ledger.destroyed().values().sum::<usize>(),
                ledger.transferred().values().sum::<usize>(),
                tasks.assignment_count(),
                tasks.endings(),
            );
        }
    }
    println!("assignments={assignments}");

    println!("observations={}", ledger.observations());
    println!("produced={:?}", ledger.produced());
    println!("destroyed={:?}", ledger.destroyed());
    println!("transferred={:?}", ledger.transferred());
    println!(
        "credits={:?} net_flow={} stock={}",
        ledger.credits(house),
        ledger.net_flow(house),
        ledger.holding(house).map_or(0, |h| h.total()),
    );
}
