//! Run one real match through AutoPlay's observation layer and print its ledgers.
//!
//! This is the autoplay crate's first executable entry point. It does not plan or command
//! anything: it loads a retail map as a real battle, takes a Snapshot every frame, folds
//! each one into the Ledger, and reports the flow and stock a live match produced. That is
//! the piece a play-test needs before a planner can be judged - the numbers have to come
//! from a running game, not from a hand-built test scene.
//!
//! The game client does not depend on this crate yet, so this binary is the runnable build
//! for now. It is deliberately read-only: no orders are issued, and the native team AI plays
//! both sides as it normally would.
//!
//! ```text
//! python -m tools.cargo_run -- build --profile fast -p vera20k-autoplay --bin autoplay-match
//! <resolved>/fast/autoplay-match.exe <ra2-dir> Arena.mmx 600
//! ```

use std::path::PathBuf;

use vera20k_autoplay::control::ControlState;
use vera20k_autoplay::idle::TaskBoard;
use vera20k_autoplay::ledger::Ledger;
use vera20k_autoplay::world::Snapshot;
use vera20k_core::headless_scenario;
use vera20k_core::sim::world::{FrameEffects, TickLane};
use vera20k_core::skirmish_launch::{AiDifficulty, LaunchCountry};
use vera20k_core::util::fixed_math::RA2_LOGIC_FRAMES_PER_SECOND;

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

    let frame_ms = 1000 / RA2_LOGIC_FRAMES_PER_SECOND;
    // An enabled state is an observation lens, not a claim that the client switched
    // AutoPlay on: it asks "of this house's units, how many have nothing to do".
    let control = ControlState::enabled(house);
    let mut ledger = Ledger::new();
    // No planner exists yet, so nothing is ever assigned: the number is the baseline of
    // how much work Phase 2 has to take on, not a claim that AutoPlay is running.
    let tasks = TaskBoard::new();
    for frame in 0..ticks {
        let snapshot = Snapshot::take(scenario.sim(), &scenario.runtime.resources.rules, house);
        ledger.observe(&snapshot);
        let idle = tasks.unassigned(&snapshot, &control).len();
        let _ = scenario.runtime.advance_frame(
            &[],
            frame_ms,
            TickLane::Ordinary,
            FrameEffects::default(),
        );
        if (frame + 1) % 150 == 0 {
            println!(
                "  frame {}: produced={} destroyed={} transferred={} idle={idle}",
                frame + 1,
                ledger.produced().values().sum::<usize>(),
                ledger.destroyed().values().sum::<usize>(),
                ledger.transferred().values().sum::<usize>(),
            );
        }
    }

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
