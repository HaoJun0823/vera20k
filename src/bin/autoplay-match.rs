//! Runs one standard skirmish headless, with auto play driving the local
//! house and the retail computer AI driving the opponent.
//!
//! This is the gate for the auto play chain: it plays a whole match with no
//! window and no human input, so "auto play can hold its own in a skirmish"
//! is something that can be run, not something that has to be remembered
//! from the last time somebody played it.
//!
//! Everything it decides depends only on the simulation state, the tick and
//! the pinned seed, in integer math, so a run replays.
//!
//! ```text
//! cargo run --bin autoplay-match -- --ra2-dir <install> --map Death.mmx
//! ```

use std::path::PathBuf;

use vera20k_core::headless_scenario::{self, SIM_TICK_MS};
use vera20k_core::sim::autoplay::AutoPlayHost;
use vera20k_core::sim::control::brain::{Brain, Outbox};
use vera20k_core::sim::control::observation::Observation;
use vera20k_core::sim::intern::InternedId;
use vera20k_core::sim::production::ProductionCategory;
use vera20k_core::sim::strategy::retail::RetailBrain;
use vera20k_core::skirmish_launch::{AiDifficulty, LaunchCountry};

/// The map main's own match bench loads by default, so it is known to have the
/// ore, the starts and the trigger data a match needs.
const DEFAULT_MAP: &str = "Death.mmx";
/// Ten minutes of game time at the native 15 frames a second.
const DEFAULT_TICKS: u64 = 9_000;
/// A progress line every thirty seconds.
const DEFAULT_PROGRESS: u64 = 450;
/// Commands the control layer admits per frame. The client's hotkey uses 8;
/// a headless run is not a player, but the budget is the same shape.
const DEFAULT_PER_FRAME: usize = 8;

struct Args {
    ra2_dir: PathBuf,
    map: String,
    seed: u32,
    ticks: u64,
    progress: u64,
    per_frame: usize,
    opponent: LaunchCountry,
    difficulty: AiDifficulty,
    /// Print what the brain asks for, from a separate brain reading the same
    /// state, for the first this many frames. Diagnostics only: the run itself
    /// is driven by the host's own brain, whose generator has moved on.
    dump: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut ra2_dir = std::env::var("RA2_DIR")
        .ok()
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let mut map = DEFAULT_MAP.to_string();
    let mut seed = 0x5EED_u32;
    let mut ticks = DEFAULT_TICKS;
    let mut progress = DEFAULT_PROGRESS;
    let mut per_frame = DEFAULT_PER_FRAME;
    let mut opponent = LaunchCountry::Russia;
    let mut difficulty = AiDifficulty::Normal;
    let mut dump = 0_u64;

    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.to_string_lossy().into_owned();
        let mut value = || -> Result<String, String> {
            args.next()
                .map(|value| value.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--ra2-dir" => ra2_dir = Some(PathBuf::from(value()?)),
            "--map" => map = value()?,
            "--seed" => seed = value()?.parse().map_err(|_| "seed is a u32".to_string())?,
            "--ticks" => ticks = value()?.parse().map_err(|_| "ticks is a u64".to_string())?,
            "--progress" => {
                progress = value()?.parse().map_err(|_| "progress is a u64".to_string())?
            }
            "--per-frame" => {
                per_frame = value()?.parse().map_err(|_| "per-frame is a usize".to_string())?
            }
            "--opponent" => opponent = parse_country(&value()?)?,
            "--difficulty" => difficulty = parse_difficulty(&value()?)?,
            "--dump" => dump = value()?.parse().map_err(|_| "dump is a u64".to_string())?,
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let ra2_dir = ra2_dir.ok_or("set --ra2-dir or RA2_DIR to the retail install")?;
    Ok(Args {
        ra2_dir,
        map,
        seed,
        ticks,
        progress,
        per_frame,
        opponent,
        difficulty,
        dump,
    })
}

fn parse_country(name: &str) -> Result<LaunchCountry, String> {
    match name.to_ascii_lowercase().as_str() {
        "america" => Ok(LaunchCountry::America),
        "russia" => Ok(LaunchCountry::Russia),
        "yuri" => Ok(LaunchCountry::Yuri),
        "korea" => Ok(LaunchCountry::Korea),
        "iraq" => Ok(LaunchCountry::Iraq),
        "france" => Ok(LaunchCountry::France),
        "germany" => Ok(LaunchCountry::Germany),
        "cuba" => Ok(LaunchCountry::Cuba),
        "libya" => Ok(LaunchCountry::Libya),
        "greatbritain" | "britain" => Ok(LaunchCountry::GreatBritain),
        other => Err(format!("unknown country {other}")),
    }
}

fn parse_difficulty(name: &str) -> Result<AiDifficulty, String> {
    match name.to_ascii_lowercase().as_str() {
        "easy" => Ok(AiDifficulty::Easy),
        "normal" => Ok(AiDifficulty::Normal),
        "hard" => Ok(AiDifficulty::Hard),
        other => Err(format!("unknown difficulty {other}")),
    }
}

const USAGE: &str = "\
autoplay-match: a headless skirmish with auto play on the local house

  --ra2-dir <dir>    the retail install (or RA2_DIR)
  --map <file>       the map to load (default Death.mmx)
  --seed <u32>       the pinned seed (default 0x5eed)
  --ticks <u64>      how many frames to play (default 9000, ten minutes)
  --progress <u64>   a progress line every this many frames (default 450)
  --per-frame <n>    commands the control layer admits per frame (default 8)
  --opponent <name>  the computer house's country (default russia)
  --difficulty <n>   easy | normal | hard (default normal)
";

/// One line of telemetry, so a run can be read without a debugger.
struct Snapshot {
    tick: u64,
    credits: i32,
    enemy_credits: i32,
    power: i32,
    own: usize,
    enemy: usize,
    ready: usize,
    rejected: usize,
    not_ready: usize,
}

fn snapshot(sim: &vera20k_core::sim::world::Simulation, local: InternedId, enemy: InternedId, tick: u64, host: &AutoPlayHost) -> Snapshot {
    let obs = Observation::new(sim, local);
    Snapshot {
        tick,
        credits: obs.credits(),
        enemy_credits: Observation::new(sim, enemy).credits(),
        power: obs.power_balance(),
        own: obs.own_entities().count(),
        enemy: Observation::new(sim, enemy).own_entities().count(),
        ready: obs.ready_buildings().count(),
        rejected: host.rejected(),
        not_ready: host.not_ready(),
    }
}

/// A logger that prints `retail-ai` and friends to stderr. The brain logs why
/// it is waiting, which is the whole point of a headless run: nobody is
/// watching the base to notice it stopped.
struct StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        eprintln!("[{}] {}", record.level(), record.args());
    }

    fn flush(&self) {}
}

fn main() -> Result<(), String> {
    static LOGGER: StderrLog = StderrLog;
    log::set_logger(&LOGGER).map_err(|error| error.to_string())?;
    log::set_max_level(log::LevelFilter::Info);
    log::info!("logger up");

    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            if error.is_empty() {
                print!("{USAGE}");
                return Ok(());
            }
            eprintln!("autoplay-match: {error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let session = headless_scenario::battle_session(&args.map, &[(args.opponent, args.difficulty)]);
    let mut scenario = headless_scenario::load_session(&args.ra2_dir, &args.map, args.seed, session)?;
    let sim = &scenario.runtime.simulation;

    let local = sim
        .session()
        .current_house()
        .ok_or("the launch has no local house")?;
    let enemy = sim
        .houses()
        .order()
        .iter()
        .copied()
        .find(|id| *id != local && sim.houses().get(id).is_some_and(|house| !house.is_human()))
        .ok_or("the launch has no computer opponent")?;

    println!(
        "autoplay-match: {} on {}, seed {:#x}, {} ticks, {} per frame",
        sim.interner().resolve(local),
        args.map,
        args.seed,
        args.ticks,
        args.per_frame
    );
    println!(
        "{:>6} {:>9} {:>9} {:>7} {:>6} {:>6} {:>6} {:>9} {:>9}",
        "tick", "credits", "ecred", "power", "own", "enemy", "ready", "rejected", "notready"
    );

    if args.dump > 0 {
        for entity in Observation::new(sim, local).own_entities() {
            println!(
                "  start: {} {} at {:?} health {} mission {:?} orderable {}",
                entity.entity_id,
                sim.interner().resolve(entity.type_id),
                entity.cell,
                entity.health,
                entity.mission,
                entity.orderable
            );
        }
    }

    let mut host = AutoPlayHost::new(
        Box::new(RetailBrain::new(u64::from(args.seed))),
        local,
        args.per_frame,
    );
    let mut last = snapshot(sim, local, enemy, 0, &host);
    println!(
        "{:>6} {:>9} {:>9} {:>7} {:>6} {:>6} {:>6} {:>9} {:>9}",
        last.tick, last.credits, last.enemy_credits, last.power, last.own, last.enemy, last.ready,
        last.rejected, last.not_ready
    );

    let mut outcome = "timeout";
    for tick in 1..=args.ticks {
        if tick <= args.dump {
            let sim = &scenario.runtime.simulation;
            let obs = Observation::new(sim, local);
            let mut brain = RetailBrain::new(u64::from(args.seed));
            let mut out = Outbox::default();
            brain.think(&scenario.runtime.resources.rules, &obs, &mut out);
            let funnel = vera20k_core::sim::behavior::Funnel::new(
                sim,
                &scenario.runtime.resources.rules,
                local,
            );
            for order in &out.orders {
                println!(
                    "  brain@{tick}: {:?} -> {:?}",
                    order,
                    funnel.prepare(order.actor, order.goal)
                );
            }
            println!("  brain@{tick}: intents={:?}", out.intents);
        }
        {
            let runtime = &mut scenario.runtime;
            host.step(&mut runtime.simulation, &runtime.resources.rules);
        }
        scenario
            .runtime
            .advance_frame_for_tooling(&[], SIM_TICK_MS)
            .map_err(|error| format!("tick {tick}: {error}"))?;

        let sim = &scenario.runtime.simulation;
        let won = is_defeated(sim, enemy);
        let lost = is_defeated(sim, local);
        if lost {
            outcome = "lost";
        } else if won {
            outcome = "won";
        }
        if tick % args.progress == 0 || outcome != "timeout" {
            if args.dump > 0 {
                let obs = Observation::new(sim, local);
                let mut counts: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for entity in obs.own_entities() {
                    *counts
                        .entry(format!(
                            "{}#{:?}",
                            sim.interner().resolve(entity.type_id),
                            entity.mission
                        ))
                        .or_default() += 1;
                }
                let line: Vec<String> = counts
                    .iter()
                    .map(|(name, count)| format!("{name} {count}"))
                    .collect();
                println!("  own@{tick}: {}", line.join(", "));
                let yard = obs
                    .building_in_production()
                    .map(|(type_id, suspended)| {
                        format!(
                            "{} held={suspended}",
                            sim.interner().resolve(type_id)
                        )
                    })
                    .unwrap_or_else(|| "idle".to_owned());
                let rules = &scenario.runtime.resources.rules;
                let held: Vec<String> = [
                    ProductionCategory::Building,
                    ProductionCategory::Vehicle,
                    ProductionCategory::Infantry,
                    ProductionCategory::Aircraft,
                ]
                .into_iter()
                .map(|category| {
                    let text = obs
                        .factory_product(category)
                        .map(|(type_id, suspended)| {
                            let progress = obs
                                .production_progress(rules, type_id)
                                .unwrap_or(0);
                            format!("{}@{progress} held={suspended}", sim.interner().resolve(type_id))
                        })
                        .unwrap_or_else(|| "-".to_owned());
                    format!("{category:?}={text}")
                })
                .collect();
                println!(
                    "  spend@{tick}: mode {} yard {yard} reserve {} | {}",
                    obs.production_mode(),
                    obs.army_reserve(rules),
                    held.join(" ")
                );
            }
            last = snapshot(sim, local, enemy, tick, &host);
            println!(
                "{:>6} {:>9} {:>9} {:>7} {:>6} {:>6} {:>6} {:>9} {:>9}",
                last.tick,
                last.credits,
                last.enemy_credits,
                last.power,
                last.own,
                last.enemy,
                last.ready,
                last.rejected,
                last.not_ready
            );
        }
        if outcome != "timeout" {
            break;
        }
    }

    println!("autoplay-match: {outcome} at tick {}", last.tick);
    if args.dump > 0 {
        let sim = &scenario.runtime.simulation;
        let obs = Observation::new(sim, local);
        for entity in obs.own_entities() {
            println!(
                "  final: {} at {:?} health {} mission {:?} inprod {}",
                sim.interner().resolve(entity.type_id),
                entity.cell,
                entity.health,
                entity.mission,
                obs.in_production(&scenario.runtime.resources.rules, entity.type_id)
            );
        }
        for entity in Observation::new(sim, enemy).own_entities() {
            println!(
                "  enemy: {} at {:?} health {}",
                sim.interner().resolve(entity.type_id),
                entity.cell,
                entity.health
            );
        }
    }
    Ok(())
}

fn is_defeated(sim: &vera20k_core::sim::world::Simulation, house: InternedId) -> bool {
    sim.houses()
        .get(&house)
        .is_some_and(|house| house.is_defeated())
}
