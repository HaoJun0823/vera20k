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

use std::collections::BTreeMap;
use std::path::PathBuf;

use vera20k_autoplay::control::ControlState;
use vera20k_autoplay::dispatch::Dispatcher;
use vera20k_autoplay::executor::Executor;
use vera20k_autoplay::frame::{OrderKind, plan_frame};
use vera20k_autoplay::idle::TaskBoard;
use vera20k_autoplay::ledger::Ledger;
use vera20k_autoplay::world::Snapshot;
use vera20k_core::headless_scenario;
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
    let mut dispatcher = Dispatcher::new();
    let mut assignments = 0usize;
    let mut attacks = 0usize;
    let mut guards = 0usize;
    for frame in 0..ticks {
        let snapshot = Snapshot::take(scenario.sim(), &scenario.runtime.resources.rules, house);
        ledger.observe(&snapshot);

        // One frame's decision, as a value: the order to submit (if any) and the task
        // endings already applied to the board. The client will use the same API.
        let plan = plan_frame(
            scenario.sim(),
            &snapshot,
            &control,
            &mut tasks,
            &mut dispatcher,
        );

        match plan {
            Some(plan) => {
                let subject = plan.validated.subject;
                if Executor::submit(&mut scenario.runtime, &plan.validated).is_committed() {
                    dispatcher.note_issued(subject, plan.payload, plan.frame, plan.expected);
                    tasks.assign(subject, plan.frame);
                    assignments += 1;
                    match plan.kind {
                        OrderKind::Attack => attacks += 1,
                        OrderKind::Guard => guards += 1,
                    }
                }
            }
            None => {
                let _ = Executor::submit_idle(&mut scenario.runtime);
            }
        }

        if (frame + 1) % 150 == 0 {
            println!(
                "  frame {}: produced={} destroyed={} transferred={} active={} visible_enemies={} ended={:?}",
                frame + 1,
                ledger.produced().values().sum::<usize>(),
                ledger.destroyed().values().sum::<usize>(),
                ledger.transferred().values().sum::<usize>(),
                tasks.assignment_count(),
                snapshot.enemy_visible().count(),
                tasks.endings(),
            );
        }
    }
    println!("assignments={assignments} attacks={attacks} guards={guards}");
    println!(
        "suppressed_still_running={} suppressed_same_frame={}",
        dispatcher.suppressed_still_running(),
        dispatcher.suppressed_same_frame()
    );

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

    // Rule-derived facts that T03/T04/T05 will decide on. One row per type, so the
    // report stays readable and does not second-guess the rules anywhere else.
    let final_view = Snapshot::take(scenario.sim(), &scenario.runtime.resources.rules, house);
    let mut facts: BTreeMap<&str, (i32, Option<u16>, bool, bool)> = BTreeMap::new();
    for object in final_view.own() {
        facts.entry(&object.type_name).or_insert((
            object.cost,
            object.range_cells,
            object.can_target_air,
            object.can_target_ground,
        ));
    }
    println!("rule_facts(cost,range_cells,aa,ag)={facts:?}");
}
