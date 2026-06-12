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

    def test_cpi_preset_builds_normal_probe(self):
        markets = [
            {
                "address": "0x5f",
                "title": "US Inflation in June 2026 (CPI YoY)",
                "marketType": "normal",
            }
        ]
        specs = gap.load_probe_specs(None, "cpi-nowcast-20260612")
        probes = gap.build_probes(markets, specs, 100.0)
        self.assertEqual(len(probes), 1)
        self.assertEqual(probes[0].family, "normal")
        self.assertEqual(probes[0].belief, 4.05)

    def test_runner_blockers_include_ev_floor_and_concentration(self):
        probe = gap.Probe("cpi", "0x1", "normal", 4.05, 0.24, 100.0)
        market = {"address": "0x1", "title": "US Inflation in June 2026 (CPI YoY)"}
        quote = {"on_chain_will_accept": True, "expected_value": 6.44}
        positions = [{"marketAddress": "0x1", "hasPosition": True, "deltaCount": 2}]
        blockers = gap.runner_blockers_for_probe(probe, market, [market], positions, quote)
        self.assertTrue(any("below 10 XP floor" in blocker for blocker in blockers))
        self.assertTrue(any("market concentration cap" in blocker for blocker in blockers))

    def test_runner_blockers_detect_world_cup_from_category(self):
        probe = gap.Probe("france", "0x1", "lognormal", 3.3, 0.27, 100.0)
        market = {"address": "0x1", "category": "World Cup", "title": "When will France be eliminated?"}
        quote = {"on_chain_will_accept": True, "expected_value": 12.0}
        blockers = gap.runner_blockers_for_probe(probe, market, [market], [], quote)
        self.assertIn("World Cup probe has no post-result evidence marker", blockers)

    def test_rank_summary_computes_own_gap(self):
        totals = {"0xaaa": 100.0, "0xbbb": 80.0, "0xccc": 120.0}
        summary = gap.rank_summary(totals, "0xbbb")
        self.assertEqual(summary["rank"], 3)
        self.assertEqual(summary["gap"], 40.0)
        self.assertEqual(summary["top_trader"], "0xccc")

    def test_classify_runner_candidate_when_no_blockers(self):
        item = gap.classify_opportunity(
            runner_blockers=[],
            mark_gap_improvement=15.0,
            belief_gap_improvement=20.0,
            expected_value=12.0,
        )
        self.assertEqual(item["status"], "runner_candidate")
        self.assertEqual(item["priority"], 5)

    def test_classify_durable_watch_before_paint_trap(self):
        item = gap.classify_opportunity(
            runner_blockers=["World Cup probe has no post-result evidence marker"],
            mark_gap_improvement=100.0,
            belief_gap_improvement=75.0,
            expected_value=4.0,
        )
        self.assertEqual(item["status"], "durable_watch")

    def test_classify_paint_trap_when_mark_exceeds_belief(self):
        item = gap.classify_opportunity(
            runner_blockers=["standalone EV 2.000000 below 10 XP floor"],
            mark_gap_improvement=76.0,
            belief_gap_improvement=2.5,
            expected_value=2.0,
        )
        self.assertEqual(item["status"], "paint_trap")

    def test_sort_key_defaults_to_belief_gap(self):
        durable = {
            "scoreboard": {"gap_improvement": 5.0},
            "belief_scoreboard": {"gap_improvement": 80.0},
            "quote": {"expected_value": 3.0},
            "opportunity": {"priority": 4},
        }
        paint = {
            "scoreboard": {"gap_improvement": 70.0},
            "belief_scoreboard": {"gap_improvement": 2.0},
            "quote": {"expected_value": 2.0},
            "opportunity": {"priority": 1},
        }
        self.assertGreater(gap.sort_key(durable, "belief_gap"), gap.sort_key(paint, "belief_gap"))
        self.assertGreater(gap.sort_key(paint, "mark_gap"), gap.sort_key(durable, "mark_gap"))


if __name__ == "__main__":
    unittest.main()
