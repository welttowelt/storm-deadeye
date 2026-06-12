#!/usr/bin/env python3
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import storm_deadeye_loop as loop


class StormDeadeyeLoopTests(unittest.TestCase):
    def test_canonical_address_matches_leading_zero_variants(self):
        self.assertEqual(loop.canonical_address("0x04418F"), "0x4418f")
        self.assertEqual(loop.canonical_address("0x4418f"), "0x4418f")

    def test_compute_rank_and_gap(self):
        rows = [
            {"trader": "0xaaa", "totalPnl": 100.0, "marketsTraded": 3, "totalTrades": 4},
            {"trader": "0x04418f", "totalPnl": 55.5, "marketsTraded": 1, "totalTrades": 1},
        ]
        rank = loop.compute_rank(rows, "0x4418f")
        self.assertEqual(rank["rank"], 2)
        self.assertAlmostEqual(rank["gap_to_first"], 44.5)

    def test_gas_tiers(self):
        self.assertEqual(loop.gas_tier(1043.0), "ok")
        self.assertEqual(loop.gas_tier(99.9), "warn")
        self.assertEqual(loop.gas_tier(49.9), "strong_warn")
        self.assertEqual(loop.gas_tier(24.9), "hard_stop")

    def test_ladder_budget_preserves_reserve(self):
        self.assertEqual(loop.select_ladder_budget(4000, 19914.0), 4000.0)
        self.assertEqual(loop.select_ladder_budget(275, 1300.0), 250.0)
        with self.assertRaises(loop.LoopError):
            loop.select_ladder_budget(100, 1050.0)

    def test_trade_history_hour_window(self):
        state = {"trade_history": [{"timestamp": 100}, {"timestamp": 3601}, {"timestamp": 7200}]}
        self.assertEqual(len(loop.trade_history_last_hour(state, ts=7200)), 2)

    def test_filter_slug_discovery(self):
        markets = [
            {"category": "Economics", "topics": ["Inflation", "CPI"]},
            {"category": "World Cup", "topics": ["France 2026"]},
        ]
        self.assertEqual(loop.discover_filter_slugs(markets), ["cpi", "economics", "france-2026", "inflation", "world-cup"])

    def test_candidate_validation_requires_evidence(self):
        market = {"marketType": "normal", "category": "Economics", "title": "US Inflation CPI"}
        candidate = {
            "id": "cpi-1",
            "market": "0x1",
            "family": "normal",
            "belief": 3.8,
            "belief_sigma": 0.16,
            "rationale": "BLS and nowcast evidence imply a cooler CPI print than the curve.",
            "evidence": [{"claim": "CPI source", "source": "BLS", "url": "https://www.bls.gov/cpi/"}],
        }
        self.assertEqual(loop.validate_candidate(candidate, market), [])
        candidate["evidence"] = []
        self.assertIn("missing evidence list", loop.validate_candidate(candidate, market))

    def test_summary_key_captures_health_and_candidate_changes(self):
        summary = {
            "rankings": {
                "overall": {"rank": 4, "gap_to_first": 479.0864, "pnl": 50.0161},
                "filters": {"economics": {"healthy": False}, "cpi": {"healthy": True}},
            },
            "gas_tier": "ok",
            "processed_candidates": [{"id": "a", "status": "dry_run_ok"}],
        }
        key = loop.summary_key(summary)
        self.assertEqual(key["rank"], 4)
        self.assertEqual(key["unhealthy_filters"], ["economics"])
        self.assertEqual(key["processed"], [{"id": "a", "status": "dry_run_ok"}])


if __name__ == "__main__":
    unittest.main()
