"""The autoplay check tool reads a match log and compares two runs; both
readings are what a run is judged on, so they are pinned here."""
from __future__ import annotations

import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest

from tools.autoplay_check import compare_histories, judge, opening_drift, parse_report

LOG = """[INFO] auto play: building=35 infantry=60 vehicle=40 aircraft=6 ship=0 harv=6 \
ref=3 yard=2 fighters=100 threat=[0,0,0,0,0,0,0] able=[90,98,15,90,38,100,9] \
roster=[E1:40,CAOILD:13,MTNK:12] crates=1[(61, 163)] income=4200/min seen=3/3
[INFO] auto play: the build list answers Superweapon for GACSPH
[INFO] crate picked up at (61, 163) by house Player
[INFO] crate picked up at (96, 54) by house Player
[INFO] auto play: wall segment at (189, 105), 45 standing, closes 4 cells
autoplay-match: won at tick 22915
"""


class ParseReportTests(unittest.TestCase):
    def test_the_result_is_the_line_that_says_so(self):
        report = parse_report("24301", LOG, 57.0)
        self.assertTrue(report.won)
        self.assertEqual(report.tick, 22915)
        self.assertEqual(report.result, "won at tick 22915")

    def test_the_evidence_lines_are_read(self):
        report = parse_report("24301", LOG, 57.0)
        self.assertEqual(report.yards, 2)
        self.assertEqual(report.walls, 45)
        self.assertEqual(report.plots, ["Superweapon:GACSPH"])
        self.assertEqual(
            report.crates,
            ["(61, 163) (Player)", "(96, 54) (Player)"],
        )

    def test_a_log_without_a_result_says_so(self):
        report = parse_report("24301", "nothing here", 1.0)
        self.assertFalse(report.won)
        self.assertEqual(report.result, "no result")


def row(**overrides) -> dict:
    """One history row, in the shape append_history writes."""
    base = {
        "when": "2026-11-02T08:00:00",
        "commit": "8328ef47d",
        "brain": "v2",
        "map": "Death.mmx",
        "country": "america",
        "seed": "24301",
        "opponents": 1,
        "ticks": 12000,
        "result": "timeout at tick 12000",
        "tick": 12000,
        "own": 35,
        "enemy": 47,
        "unowned": 0,
        "stuck": 0,
        "yards": 1,
        "walls": 0,
        "start_credits": 10000,
        "start_own": 13,
        "start_units": 10,
        "start_tech": 10,
        "roster": "AMCV:1,E1:7,MTNK:5",
        "seconds": 30.0,
    }
    base.update(overrides)
    return base


def history_file(directory: Path, name: str, rows: list[dict], broken: int = 0) -> Path:
    """A history file, with `broken` lines that are not JSON."""
    path = directory / name
    lines = [json.dumps(item) for item in rows] + ["{not json"] * broken
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


class CompareTests(unittest.TestCase):
    def test_a_worse_cell_names_what_moved(self):
        """The keep rule as reasons: own down, unowned up, a seed lost."""
        verdict, worse, better = judge(
            row(),
            row(own=32, unowned=2, result="lost at 17680", tick=17680),
        )
        self.assertEqual(verdict, "worse")
        self.assertEqual(better, [])
        self.assertEqual(
            worse,
            ["own 35->32", "unowned 0->2", "lost at 17680 (was survived)"],
        )

    def test_a_house_that_survives_where_it_lost_is_better_and_an_opening_is_a_warning(self):
        """A better cell is named too, and a moved opening is not a delta."""
        before = row(result="lost at 9000", tick=9000)
        after = row(own=40, start_credits=0)
        verdict, worse, better = judge(before, after)
        self.assertEqual(verdict, "better")
        self.assertEqual(worse, [])
        self.assertIn("own 35->40", better)
        self.assertIn("survived (was lost at 9000)", better)
        self.assertEqual(opening_drift(before, after), ["start_credits 10000->0"])

    def test_the_comparison_marks_what_got_worse_and_never_passes_over_a_missing_row(self):
        """Two files on disk, read end to end: the printed table is the verdict."""
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            baseline = history_file(directory, "before.jsonl", [
                row(),
                row(country="russia", seed="19", own=51, enemy=48),
            ])
            current = history_file(directory, "after.jsonl", [
                row(own=32, unowned=2),
                row(country="yuri", seed="10", own=29, enemy=46),
            ], broken=1)
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                compare_histories(baseline, current)
        text = printed.getvalue()
        self.assertIn("1 unreadable line(s)", text)
        self.assertIn("own 35->32; unowned 0->2", text)
        self.assertIn("cells that got worse:", text)
        self.assertIn("missing from this run", text)
        self.assertIn("no baseline row", text)
        self.assertNotIn("no cell got worse", text)
        # One cell got worse, one is missing from each side, and the count adds
        # up: nothing is quietly dropped from the table.
        self.assertIn("cells: 3 compared, 0 same, 0 better, 1 worse, 2 missing", text)


if __name__ == "__main__":
    unittest.main()
