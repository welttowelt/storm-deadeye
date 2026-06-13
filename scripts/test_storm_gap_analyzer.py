#!/usr/bin/env python3
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

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

    def test_active_portfolio_preset_includes_world_cup_and_cpi(self):
        specs = gap.load_probe_specs(None, "active-portfolio-20260612")
        self.assertEqual(len(specs), len(gap.WORLD_CUP_POD_20260612) + len(gap.CPI_NOWCAST_20260612))
        self.assertTrue(any(spec.get("family") == "normal" for spec in specs))
        self.assertTrue(any("France" in spec.get("label", "") for spec in specs))

    def test_budget_values_ladder_uses_policy_rungs_and_reserve(self):
        budgets = gap.budget_values({}, 1000.0, 1500.0, budget_ladder=True)
        self.assertEqual(budgets, [100.0, 250.0, 500.0])

    def test_budget_values_accepts_explicit_probe_budgets(self):
        budgets = gap.budget_values({"budgets": [250, 100, 2000]}, 4000.0, 1300.0, budget_ladder=True)
        self.assertEqual(budgets, [100.0, 250.0])

    def test_build_probes_expands_budget_ladder(self):
        markets = [
            {
                "address": "0x5f",
                "title": "US Inflation in June 2026 (CPI YoY)",
                "marketType": "normal",
            }
        ]
        specs = gap.load_probe_specs(None, "cpi-nowcast-20260612")
        probes = gap.build_probes(markets, specs, 1000.0, budget_ladder=True, bankroll=19832.0)
        self.assertEqual([probe.budget for probe in probes], [100.0, 250.0, 500.0, 1000.0])

    def test_resolve_context_uses_explicit_overrides_without_account_snapshot(self):
        original_account_snapshot = gap.loop.account_snapshot

        def fail_account_snapshot():
            raise AssertionError("explicit context should not read account snapshot")

        try:
            gap.loop.account_snapshot = fail_account_snapshot
            indexer_url, trader = gap.resolve_context(
                SimpleNamespace(indexer_url="https://indexer.test", trader="0x4418")
            )
        finally:
            gap.loop.account_snapshot = original_account_snapshot

        self.assertEqual(indexer_url, "https://indexer.test")
        self.assertEqual(trader, "0x4418")

    def test_quote_probe_retries_transient_rate_limit(self):
        probe = gap.Probe("cpi", "0x1", "normal", 4.05, 0.24, 100.0)
        calls = []
        original_deadeye_json = gap.loop.deadeye_json
        original_sleep = gap.time.sleep

        def fake_deadeye_json(args, *, timeout):
            calls.append(args)
            if len(calls) == 1:
                raise gap.loop.LoopError("provider error: Request too fast per second")
            return {"on_chain_will_accept": True, "expected_value": 12.0}

        try:
            gap.loop.deadeye_json = fake_deadeye_json
            gap.time.sleep = lambda _: None
            quote = gap.quote_probe(probe, 19832.0)
        finally:
            gap.loop.deadeye_json = original_deadeye_json
            gap.time.sleep = original_sleep

        self.assertEqual(len(calls), 2)
        self.assertTrue(quote["on_chain_will_accept"])

    def test_quote_probe_does_not_retry_non_transient_error(self):
        probe = gap.Probe("cpi", "0x1", "normal", 4.05, 0.24, 100.0)
        calls = []
        original_deadeye_json = gap.loop.deadeye_json

        def fake_deadeye_json(args, *, timeout):
            calls.append(args)
            raise gap.loop.LoopError("quote rejected: insufficient collateral")

        try:
            gap.loop.deadeye_json = fake_deadeye_json
            with self.assertRaises(gap.loop.LoopError):
                gap.quote_probe(probe, 19832.0)
        finally:
            gap.loop.deadeye_json = original_deadeye_json

        self.assertEqual(len(calls), 1)

    def test_output_payload_omits_operational_context_by_default(self):
        args = SimpleNamespace(
            preset="world-cup-pod-20260612",
            sort_by="ev",
            budget_ladder=True,
            quote_only=True,
            include_operational_context=False,
        )
        payload = gap.output_payload(
            args,
            [{"label": "Germany higher"}],
            {"coverage_complete": False},
            trader="0x4418",
            indexer_url="https://indexer.test",
        )
        self.assertNotIn("trader", payload)
        self.assertNotIn("indexer_url", payload)
        self.assertEqual(payload["results"], [{"label": "Germany higher"}])
        self.assertEqual(payload["coverage"], {"coverage_complete": False})

    def test_output_payload_can_include_operational_context_explicitly(self):
        args = SimpleNamespace(
            preset="world-cup-pod-20260612",
            sort_by="ev",
            budget_ladder=True,
            quote_only=True,
            include_operational_context=True,
        )
        payload = gap.output_payload(args, [], {}, trader="0x4418", indexer_url="https://indexer.test")
        self.assertEqual(payload["trader"], "0x4418")
        self.assertEqual(payload["indexer_url"], "https://indexer.test")

    def test_coverage_summary_counts_active_markets_by_category(self):
        markets = [
            {
                "address": "0x1",
                "title": "France World Cup",
                "marketType": "lognormal",
                "category": "soccer",
                "isActive": True,
                "state": {"isInitialized": True},
            },
            {
                "address": "0x2",
                "title": "CPI",
                "marketType": "normal",
                "category": "Economics",
                "isActive": True,
                "state": {"isInitialized": True},
            },
            {
                "address": "0x3",
                "title": "Settled",
                "marketType": "normal",
                "category": "Economics",
                "isActive": False,
                "state": {"isInitialized": True},
            },
        ]
        probes = [gap.Probe("france", "0x1", "lognormal", 3.3, 0.2, 100.0)]
        coverage = gap.coverage_summary(markets, probes)
        self.assertEqual(coverage["active_tradeable_markets"], 2)
        self.assertEqual(coverage["covered_active_tradeable_markets"], 1)
        self.assertFalse(coverage["coverage_complete"])
        self.assertEqual(coverage["by_category"]["soccer"], {"active_tradeable": 1, "covered": 1})
        self.assertEqual(coverage["by_category"]["Economics"], {"active_tradeable": 1, "covered": 0})

    def test_fetch_indexer_retries_transient_status(self):
        calls = []
        original_http_get_json = gap.loop.http_get_json
        original_sleep = gap.time.sleep

        def fake_http_get_json(base_url, path):
            calls.append((base_url, path))
            if len(calls) == 1:
                return 0, {}
            return 200, [{"ok": True}]

        try:
            gap.loop.http_get_json = fake_http_get_json
            gap.time.sleep = lambda _: None
            payload = gap.fetch_indexer("https://indexer.test", "/api/test")
        finally:
            gap.loop.http_get_json = original_http_get_json
            gap.time.sleep = original_sleep

        self.assertEqual(payload, [{"ok": True}])
        self.assertEqual(len(calls), 2)

    def test_fetch_indexer_does_not_retry_client_error(self):
        calls = []
        original_http_get_json = gap.loop.http_get_json

        def fake_http_get_json(base_url, path):
            calls.append((base_url, path))
            return 404, {"error": "missing"}

        try:
            gap.loop.http_get_json = fake_http_get_json
            with self.assertRaises(gap.loop.LoopError):
                gap.fetch_indexer("https://indexer.test", "/api/missing")
        finally:
            gap.loop.http_get_json = original_http_get_json

        self.assertEqual(len(calls), 1)

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

    def test_analyze_probe_readonly_records_market_fetch_failure(self):
        probe = gap.Probe("england", "0xeng", "lognormal", 3.31, 0.27, 100.0)
        market = {
            "address": "0xeng",
            "title": "When will England be eliminated from the 2026 World Cup?",
            "marketType": "lognormal",
        }
        original_quote_probe = gap.quote_probe
        original_fetch_indexer = gap.fetch_indexer

        def fake_quote_probe(_probe, _bankroll):
            return {
                "on_chain_will_accept": True,
                "expected_value": 12.0,
                "candidate_mean": 3.2,
                "candidate_sigma": 0.3,
            }

        def fake_fetch_indexer(_indexer_url, _path):
            raise gap.loop.LoopError("/api/markets/0xeng/traders returned HTTP 0")

        try:
            gap.quote_probe = fake_quote_probe
            gap.fetch_indexer = fake_fetch_indexer
            result = gap.analyze_probe_readonly(
                probe,
                market,
                [market],
                [],
                [],
                "0x4418",
                "https://indexer.test",
                19832.0,
            )
        finally:
            gap.quote_probe = original_quote_probe
            gap.fetch_indexer = original_fetch_indexer

        self.assertEqual(result["opportunity"]["status"], "scout_error")
        self.assertEqual(result["budget"], 100.0)
        self.assertFalse(result["runner_gate"]["would_pass_current_runner"])
        self.assertIn("HTTP 0", result["error"])
        self.assertEqual(result["quote"]["expected_value"], 12.0)

    def test_analyze_probe_readonly_quote_only_skips_trader_fetch(self):
        probe = gap.Probe("france", "0xfra", "lognormal", 3.33, 0.27, 500.0)
        market = {
            "address": "0xfra",
            "title": "When will France be eliminated from the 2026 World Cup?",
            "marketType": "lognormal",
        }
        original_quote_probe = gap.quote_probe
        original_fetch_indexer = gap.fetch_indexer

        def fake_quote_probe(_probe, _bankroll):
            return {
                "on_chain_will_accept": True,
                "expected_value": 18.0,
                "required_collateral": 80.0,
                "candidate_mean": 3.2,
                "candidate_sigma": 0.3,
            }

        def fail_fetch_indexer(_indexer_url, _path):
            raise AssertionError("quote-only should not fetch traders")

        try:
            gap.quote_probe = fake_quote_probe
            gap.fetch_indexer = fail_fetch_indexer
            result = gap.analyze_probe_readonly(
                probe,
                market,
                [market],
                [],
                [],
                "0x4418",
                "https://indexer.test",
                19832.0,
                quote_only=True,
            )
        finally:
            gap.quote_probe = original_quote_probe
            gap.fetch_indexer = original_fetch_indexer

        self.assertEqual(result["opportunity"]["status"], "quote_screen")
        self.assertEqual(result["budget"], 500.0)
        self.assertFalse(result["runner_gate"]["would_pass_current_runner"])
        self.assertIn("World Cup probe has no post-result evidence marker", result["runner_gate"]["blockers"])
        self.assertEqual(result["quote"]["expected_value"], 18.0)

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
