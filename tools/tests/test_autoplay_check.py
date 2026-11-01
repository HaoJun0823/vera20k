"""The autoplay check tool reads a match log; the readings are what a run is
judged on, so they are pinned here."""
from __future__ import annotations

import unittest

from tools.autoplay_check import parse_report

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


if __name__ == "__main__":
    unittest.main()
