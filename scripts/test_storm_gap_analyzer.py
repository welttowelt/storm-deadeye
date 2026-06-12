#!/usr/bin/env python3
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import storm_gap_analyzer as gap


class StormGapAnalyzerTests(unittest.TestCase):
    def test_france_leader_lognormal_mark_matches_indexer(self):
        row = {
            "mean": 3.6,
            "sigma": 0.08,
            "effectiveMean": 3.46,
            "effectiveSigma": 0.13,
            "expectedValue": 263.30429251586446,
            "unrealizedPnl": 222.57466151586448,
        }
        pnl = gap.lognormal_row_pnl_at(row, 3.4620210358378674, 150.09351569999998)
        self.assertIsNotNone(pnl)
        self.assertAlmostEqual(pnl, 222.57466151586448, places=9)

    def test_lognormal_mark_floors_gross_value_at_zero(self):
        row = {
            "mean": 3.63,
            "sigma": 0.13,
            "effectiveMean": 3.6,
            "effectiveSigma": 0.08,
            "expectedValue": 0,
            "unrealizedPnl": -20.866074,
        }
        pnl = gap.lognormal_row_pnl_at(row, 3.4620210358378674, 150.09351569999998)
        self.assertEqual(pnl, -20.866074)

    def test_new_lot_display_pnl_uses_post_mean(self):
        state = {"mean": 3.4620210358378674, "sigma": 0.13399585569756411, "effectiveK": 150.09351569999998}
        quote = {"candidate_mean": 3.3414247657100598, "candidate_sigma": 0.28474119335732373}
        pnl = gap.new_lot_display_pnl(state, quote, 7.965609091781494)
        self.assertLess(pnl, 0.0)
        self.assertAlmostEqual(pnl, -5.8571032836741495, places=9)


if __name__ == "__main__":
    unittest.main()
