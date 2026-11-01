"""One command for the autoplay loop: build, test, and run the pinned matches.

Why this exists: the loop is three commands and a wait, and the wait is where
the time goes. A pinned match on the dev-profile binary takes about three
minutes; the same match on the fast profile takes about one, and several seeds
run side by side because the machine has cores to spare. This script runs the
whole loop and prints what the run said, so a change can be measured in one
step instead of three.

Usage:
    python -m tools.autoplay_check                       # the pinned seed
    python -m tools.autoplay_check --seeds 0x5EED,19,10  # three seeds at once
    python -m tools.autoplay_check --skip-tests          # matches only
    python -m tools.autoplay_check --ticks 12000         # a shorter look

The retail install is read from RA2_DIR. Without it the build and the tests
still run and the matches are reported as skipped.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]

DEFAULT_SEEDS = ("0x5EED",)
DEFAULT_MAP = "Death.mmx"
DEFAULT_TICKS = 36_000

RESULT = re.compile(r"(won|timeout|lost) at tick (\d+)")
CRATE = re.compile(r"crate picked up at (\(\d+, \d+\)) by house (\S+)")
BUILD_LIST = re.compile(r"the build list answers (\w+) for ([A-Z0-9]+)")
STANDING = re.compile(r"(\d+) standing")
# The census row the match prints every interval: tick, credits, enemy credits,
# power, own, enemy, ready, rejected, not-ready.
CENSUS = re.compile(r"^\s*(\d+)\s+(\d+)\s+(\d+)\s+(-?\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s*$", re.M)
UNOWNED = re.compile(r"unowned=(\d+)")


@dataclass
class MatchReport:
    """What one match said about itself."""

    seed: str
    result: str = "no result"
    tick: int = 0
    seconds: float = 0.0
    crates: list[str] = field(default_factory=list)
    plots: list[str] = field(default_factory=list)
    yards: int = 0
    walls: int = 0
    map: str = ""
    own: int = 0
    enemy: int = 0
    unowned: int = 0

    @property
    def won(self) -> bool:
        return self.result.startswith("won")


def parse_report(seed: str, text: str, seconds: float) -> MatchReport:
    """Read one match log.

    The lines that matter are the ones a policy cannot show on the map: the
    result, the crates that were collected, the plot the build list chose, the
    number of yards the house ended with, and the walls that stand on the ring.
    """
    report = MatchReport(seed=seed, seconds=seconds)
    match = RESULT.search(text)
    if match:
        report.result = f"{match.group(1)} at tick {match.group(2)}"
        report.tick = int(match.group(2))
    report.crates = [f"{cell} ({house})" for cell, house in CRATE.findall(text)]
    report.plots = sorted({
        f"{item}:{name}"
        for item, name in BUILD_LIST.findall(text)
        if item == "Superweapon"
    })
    rows = CENSUS.findall(text)
    if rows:
        report.own = int(rows[-1][4])
        report.enemy = int(rows[-1][5])
    # The last reading, not the worst: the starting force is unowned until the
    # first post exists, and a maximum reports that first frame forever.
    report.unowned = int(UNOWNED.findall(text)[-1]) if UNOWNED.findall(text) else 0
    report.yards = max((int(value) for value in re.findall(r"yard=(\d+)", text)), default=0)
    report.walls = max((int(value) for value in STANDING.findall(text)), default=0)
    return report


def run_command(args: list[str]) -> int:
    """Run a command, streaming its output."""
    print("$", " ".join(args), flush=True)
    return subprocess.call(args, cwd=ROOT)


def match_binary() -> Path | None:
    """The fast-profile match binary, where cargo_run puts it."""
    owned = ROOT / "target" / "owned-worktrees"
    if not owned.is_dir():
        return None
    candidates = sorted(owned.glob("*/fast/autoplay-match.exe"))
    candidates += sorted(owned.glob("*/fast/autoplay-match"))
    # Both packages ship a binary with this name; the newest is the one the
    # build in this run just wrote.
    return max(candidates, key=lambda path: path.stat().st_mtime) if candidates else None


def run_match(
    binary: Path,
    seed: str,
    map_name: str,
    ticks: int,
    log_dir: Path,
    brain: str = "retail",
    opponents: int = 1,
) -> MatchReport:
    """Run one match and read its log.

    Every run is its own process, so the matrix is the pool: one process per
    map and seed, all of them side by side.
    """
    stem = map_name.replace("/", "_").replace("\\", "_")
    log = log_dir / f"match-{stem}-{seed}-{brain}.log"
    started = time.monotonic()
    with log.open("w", encoding="utf-8") as handle:
        subprocess.call(
            [
                str(binary),
                "--map", map_name,
                "--ticks", str(ticks),
                "--seed", seed,
                "--brain", brain,
                "--opponents", str(opponents),
            ],
            cwd=ROOT,
            stdout=handle,
            stderr=subprocess.STDOUT,
            env={**os.environ, "RUST_LOG": "info"},
        )
    seconds = time.monotonic() - started
    report = parse_report(seed, log.read_text(encoding="utf-8", errors="replace"), seconds)
    report.map = map_name
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seeds", default=",".join(DEFAULT_SEEDS),
                        help="comma separated match seeds, run side by side")
    parser.add_argument("--map", default=DEFAULT_MAP, help="the map to play")
    parser.add_argument("--maps", default="",
                        help="comma separated maps, one match per map and seed")
    parser.add_argument("--brain", default="retail", choices=["retail", "v2"],
                        help="which policy drives the local house")
    parser.add_argument("--opponents", type=int, default=1,
                        help="computer houses against the local one")
    parser.add_argument("--ticks", type=int, default=DEFAULT_TICKS, help="how many frames a match runs")
    parser.add_argument("--jobs", type=int, default=0, help="how many matches at once (0: one per seed)")
    parser.add_argument("--skip-build", action="store_true", help="use the binary as it is")
    parser.add_argument("--skip-tests", action="store_true", help="do not run the retail tests")
    parser.add_argument("--log-dir", default=None, help="where the match logs go")
    args = parser.parse_args(argv)

    seeds = [seed.strip() for seed in args.seeds.split(",") if seed.strip()]
    # The match binary reads a decimal seed; a hex one is what a person writes.
    seeds = [str(int(seed, 0)) for seed in seeds]
    log_dir = Path(args.log_dir) if args.log_dir else ROOT / "target" / "autoplay-check"
    log_dir.mkdir(parents=True, exist_ok=True)

    if not args.skip_build:
        build = [sys.executable, "-m", "tools.cargo_run", "--", "build", "--profile", "fast"]
        if args.brain == "v2":
            build += ["-p", "vera20k-autoplay", "--features", "retail-fixture"]
        build += ["--bin", "autoplay-match"]
        code = run_command(build)
        if code != 0:
            print("the build failed; the matches would be the old binary", flush=True)
            return code
    if not args.skip_tests:
        code = run_command([sys.executable, "-m", "tools.cargo_run", "--", "test",
                            "-p", "vera20k-core", "--lib", "sim::strategy::retail"])
        if code != 0:
            print("the retail tests failed", flush=True)
            return code

    binary = match_binary()
    if binary is None:
        print("no fast-profile match binary; build it first", flush=True)
        return 1
    if not os.environ.get("RA2_DIR"):
        print("RA2_DIR is not set: the matches are skipped", flush=True)
        return 0

    maps = [name.strip() for name in args.maps.split(",") if name.strip()] or [args.map]
    combos = [(name, seed) for name in maps for seed in seeds]
    jobs = args.jobs or len(combos)
    with ThreadPoolExecutor(max_workers=max(1, jobs)) as pool:
        reports = list(pool.map(
            lambda combo: run_match(
                binary, combo[1], combo[0], args.ticks, log_dir,
                args.brain, args.opponents,
            ),
            combos,
        ))

    print("", flush=True)
    for report in reports:
        print(f"{report.map} seed {report.seed} [{args.brain}]: {report.result} in {report.seconds:.0f}s", flush=True)
        print(f"  own={report.own} enemy={report.enemy} yards={report.yards} "
              f"walls={report.walls} unowned={report.unowned}", flush=True)
        if report.plots:
            print(f"  superweapon plots: {', '.join(report.plots)}", flush=True)
        if report.crates:
            print(f"  crates picked up: {', '.join(report.crates)}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
