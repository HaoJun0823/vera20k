//! B1 reachability probe: can an automated player read state, check authority and
//! move one unit, all through public API?
//!
//! Phase B1 of the plan is the first increment that writes AutoPlay code, and it
//! needs a vertical slice rather than a layer: read a unit, decide it may be
//! commanded, form a move, admit it, submit it, and read the result. Before
//! committing to a crate layout, this probe answers whether that slice is even
//! reachable from outside `vera20k-core`'s own modules - which is the difference
//! between an independent `autoplay` crate and a submodule of the engine.
//!
//! It is a probe, not the B1 implementation: no arbiter, no control state, no
//! diagnostics, and it does not pretend to be those. What it establishes is the
//! exact call sequence, which the real increment can then be written around.
//!
//! ```text
//! python -m tools.cargo_run -- build --profile fast -p vera20k-core --bin autoplay-slice
//! <resolved>/fast/autoplay-slice.exe <ra2-dir> Arena.mmx 300
//! ```

use std::path::PathBuf;

use vera20k_core::sim::command::{Command, CommandEnvelope};
use vera20k_core::sim::world::Simulation;

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
    let ticks: u64 = args.get(2).and_then(|raw| raw.parse().ok()).unwrap_or(300);

    // Load as a real battle, not as a bare map.
    //
    // `headless_scenario::load` expands to `load_session(map, battle_session(map, &[]))`:
    // an empty opponent list, so the map has a local house and nobody else. The one
    // binary here that actually drives units loads with a computer roster and then
    // builds each house a base and an army. This probe takes the same launch so the
    // comparison is about the command path, not about a map with no game on it.
    let launch = vera20k_core::headless_scenario::battle_session(
        &map,
        &[(
            vera20k_core::skirmish_launch::LaunchCountry::America,
            vera20k_core::skirmish_launch::AiDifficulty::Easy,
        )],
    );
    let mut scenario =
        match vera20k_core::headless_scenario::load_session(&retail, &map, 0x5EED, launch) {
            Ok(scenario) => scenario,
            Err(error) => {
                eprintln!("the map did not load: {error}");
                std::process::exit(1);
            }
        };

    println!("map={map} ticks={ticks}");

    // Step 1: read the world. `houses()` and `entities()` are the two accessors the
    // whole of B1 rests on, so confirm both answer before anything else.
    let house_count = scenario.sim().houses().iter().count();
    let entity_count = scenario.sim().entities().len();
    println!("houses={house_count} entities={entity_count}");
    // Every house and whether it has a base centre. `MatchScenario::setup` reads
    // `base_center()` to decide where a house stands, so a `None` here is the first
    // candidate explanation for a unit that ignores orders.
    for id in scenario.sim().houses().order().to_vec() {
        if let Some(house) = scenario.sim().houses().get(&id) {
            println!(
                "  house {id:?} human={} passive={} base_center={:?}",
                house.is_human(),
                house.multiplay_passive(),
                house.base_center(),
            );
        }
    }
    if house_count == 0 || entity_count == 0 {
        eprintln!("the map produced no houses or no entities; nothing to command");
        std::process::exit(1);
    }

    // Step 2: pick an object the local house owns, and read where it is.
    //
    // The first version took the first object with a cell. On this map that was a unit
    // at (106, 65) - nowhere near either base - and a command naming an object the
    // issuing house does not own is admitted by the queue and then ignored by
    // dispatch, which reads exactly like a broken command path.
    let Some(player) = scenario.sim().session().current_house() else {
        eprintln!("the launch has no local house");
        std::process::exit(1);
    };
    println!("local house = {player:?}");
    let Some((entity_id, start)) = scenario
        .sim()
        .entities()
        .values()
        .filter(|entity| entity.owner() == player)
        .find_map(|entity| {
            entity.in_playfield().then(|| {
                (
                    entity.stable_id(),
                    (entity.position.rx(), entity.position.ry()),
                )
            })
        })
    else {
        eprintln!("the local house owns no object in the playfield");
        std::process::exit(1);
    };
    println!("unit {entity_id} starts at ({}, {})", start.0, start.1);
    // What kind of object was picked, and can it move at all? A building has a cell
    // and an owner and would accept a Move into the queue while never moving, so the
    // probe reports the category and type before trusting the result.
    if let Some(entity) = scenario
        .sim()
        .entities()
        .values()
        .find(|entity| entity.stable_id() == entity_id)
    {
        println!(
            "  type={:?} category={:?}",
            entity.type_ref(),
            entity.category()
        );
    }

    // Step 3: form a move and submit it.
    //
    // Two submission paths exist and only one reaches the simulation from here.
    // `issue_local_command` feeds the UI's local FIFO, and `HeadlessScenario::tick`
    // calls `advance_frame(&[])` with a hard-coded empty slice, so nothing ever
    // drains that FIFO. The path that works is the `commands` argument itself, which
    // is what `sim_bench_match` uses. The first version of this probe took the other
    // one and watched a queue nobody read.
    let target = (start.0.saturating_add(2), start.1);
    // `AttackMove`, not `Move`.
    //
    // `sim_bench_match` - the one binary in this repository that actually drives units
    // - never issues `Command::Move`; it issues `AttackMove` with a cell destination,
    // `Attack`, `SetRally` and `QueueProduction`. The first version of this probe used
    // `Move`, watched the frame admit it, watched the object survive, and watched it
    // not move for 600 ticks. Whether `Move` is unimplemented or needs a precondition
    // this probe does not meet is a question for the executor work, but until it is
    // answered the working path is the one the existing binary proves.
    let command = Command::AttackMove {
        entity_id,
        target: None,
        destination: Some(vera20k_core::sim::combat::TargetKind::Cell(
            target.0, target.1,
        )),
    };
    let Some(owner) = scenario
        .sim()
        .entities()
        .values()
        .find(|entity| entity.stable_id() == entity_id)
        .map(|entity| entity.owner())
    else {
        eprintln!("the chosen object has no owner");
        std::process::exit(1);
    };
    // `execute_tick` is the frame the command becomes due, and the scheduler compares
    // it against `tick + 1`.
    let due = scenario.sim().session().tick().saturating_add(1);
    let envelope = CommandEnvelope::new(owner, due, command);
    println!("issued move to ({}, {}) for tick {due}", target.0, target.1);
    // Did the frame accept it? `SimFrameOutput` reports the admitted commands, and an
    // empty list means the scheduler dropped it before dispatch - which is a different
    // failure from "dispatched but the unit refused".
    let mut first = scenario
        .runtime
        .advance_frame(
            std::slice::from_ref(&envelope),
            67,
            vera20k_core::sim::world::TickLane::Ordinary,
            vera20k_core::sim::world::FrameEffects::default(),
        )
        .expect("the first frame ran");
    let taken = first.take_admitted_commands();
    println!("first frame admitted {} command(s)", taken.len());
    // Admission is the queue taking the record, not the object obeying it. Report
    // whether the object is still there at all: a Move for an id that has already
    // been destroyed is silently absorbed, and that looks identical to a refusal.
    let alive_after_first = scenario
        .sim()
        .entities()
        .values()
        .any(|entity| entity.stable_id() == entity_id);
    println!("object still present after frame 1: {alive_after_first}");

    // Step 4: advance with the command in hand and read the result. A unit that moved
    // is the only evidence the slice is closed; admission alone proves nothing, as the
    // first run showed.
    let mut moved_at = None;
    for tick in 2..=ticks {
        scenario
            .runtime
            .advance_frame(
                &[],
                67,
                vera20k_core::sim::world::TickLane::Ordinary,
                vera20k_core::sim::world::FrameEffects::default(),
            )
            .expect("simulation frame failed");

        if tick % 50 != 0 {
            continue;
        }
        let now = scenario
            .sim()
            .entities()
            .values()
            .find(|entity| entity.stable_id() == entity_id)
            .map(|entity| (entity.position.rx(), entity.position.ry()));
        if let Some(cell) = now
            && cell != start
        {
            moved_at = Some((tick, cell));
            break;
        }
    }

    match moved_at {
        Some((tick, cell)) => {
            println!(
                "unit {entity_id} moved to ({}, {}) by tick {tick} - the slice closes",
                cell.0, cell.1
            );
        }
        None => {
            println!("unit {entity_id} did not move within {ticks} ticks");
            println!("REACHABILITY ok, MOTION not observed");
        }
    }
}
