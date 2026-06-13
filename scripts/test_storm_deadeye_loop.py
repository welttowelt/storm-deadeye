#!/usr/bin/env python3
import json
import sys
import tempfile
import unittest
from types import SimpleNamespace
from pathlib import Path
from unittest import mock

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
        self.assertEqual(loop.select_ladder_budget(None, 19914.0), 100.0)
        with self.assertRaises(loop.LoopError):
            loop.select_ladder_budget(100, 1050.0)
        with self.assertRaises(loop.LoopError):
            loop.select_ladder_budget(None, 19914.0, require_requested=True)

    def test_campaign_loss_guard(self):
        state = {}
        guard = loop.update_campaign_loss_guard(state, {"totalPnl": 50.0})
        self.assertFalse(guard["loss_halt"])
        guard = loop.update_campaign_loss_guard(state, {"totalPnl": -1451.0})
        self.assertTrue(guard["loss_halt"])
        self.assertGreaterEqual(guard["loss_from_start"], 1500.0)

    def test_min_execute_ev_floor_constant(self):
        self.assertEqual(loop.MIN_EXECUTE_EV, 10.0)

    def test_trade_history_hour_window(self):
        state = {"trade_history": [{"timestamp": 100}, {"timestamp": 3601}, {"timestamp": 7200}]}
        self.assertEqual(len(loop.trade_history_last_hour(state, ts=7200)), 2)

    def test_filter_slug_discovery(self):
        markets = [
            {"domain": "macro", "category": "Economics", "topics": ["Inflation", "CPI"]},
            {"category": "World Cup", "topics": ["France 2026"], "tags": ["Team FRA"]},
        ]
        self.assertEqual(
            loop.discover_filter_slugs(markets),
            ["cpi", "economics", "france-2026", "inflation", "macro", "team-fra", "world-cup"],
        )

    def test_rankings_path_builder(self):
        self.assertEqual(loop.build_rankings_path(limit=5), "/api/rankings?limit=5")
        self.assertEqual(
            loop.build_rankings_path(limit=5, domain="world cup", from_ts=100, to_ts=200),
            "/api/rankings?limit=5&domain=world+cup&from=100&to=200",
        )

    def test_filtered_rankings_use_matching_filtered_stats(self):
        calls = []

        def fake_get(_base_url, path, *, timeout=15):
            calls.append(path)
            if path == "/api/rankings?limit=100&domain=world-cup":
                return 200, [{"trader": "0xaaa", "totalPnl": 20.0, "marketsTraded": 1, "totalTrades": 1}]
            if path == "/api/positions/0x4418f/stats?domain=world-cup":
                return 200, {"totalPnl": 3.0}
            raise AssertionError(f"unexpected path {path}")

        with mock.patch.object(loop, "http_get_json", side_effect=fake_get):
            view = loop.fetch_rankings_view("https://indexer.test", "0x04418f", domain="world-cup")

        self.assertTrue(view["healthy"])
        self.assertIsNone(view["rank"])
        self.assertAlmostEqual(view["pnl"], 3.0)
        self.assertAlmostEqual(view["gap_to_first"], 17.0)
        self.assertEqual(
            calls,
            [
                "/api/rankings?limit=100&domain=world-cup",
                "/api/positions/0x4418f/stats?domain=world-cup",
            ],
        )

    def test_filtered_ranking_mirror_is_not_counted_healthy(self):
        rows = [
            {"trader": "0xaaa", "totalPnl": 20.0, "marketsTraded": 1, "totalTrades": 1},
            {"trader": "0x04418f", "totalPnl": 3.0, "marketsTraded": 1, "totalTrades": 2},
        ]
        view = {"healthy": True, **loop.compute_rank(rows, "0x4418f"), "_rows_signature": loop.ranking_rows_signature(rows)}

        classified = loop.classify_ranking_view(view, loop.ranking_rows_signature(rows))

        self.assertFalse(classified["healthy"])
        self.assertEqual(classified["status"], "mirrored")
        self.assertEqual(classified["mirror_of"], "overall")
        self.assertNotIn("_rows_signature", classified)

    def test_world_cup_detection_uses_category_even_without_title(self):
        market = {"marketType": "lognormal", "category": "World Cup", "title": "When will France be eliminated?"}
        self.assertTrue(loop.is_world_cup_market(market))

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

    def test_world_cup_candidate_requires_post_result_marker(self):
        market = {"marketType": "lognormal", "category": "World Cup", "title": "When will France be eliminated from the 2026 World Cup?"}
        candidate = {
            "id": "wc-1",
            "market": "0x1",
            "family": "lognormal",
            "belief": 3.3,
            "belief_sigma": 0.27,
            "rationale": "Two current market-prior sources imply a wider distribution than the curve.",
            "evidence": [
                {"claim": "Quarterfinal odds imply path risk.", "source": "Odds source"},
                {"claim": "Power rating source implies team strength.", "source": "Rating source"},
            ],
        }
        self.assertIn("World Cup candidate needs post-result evidence marker", loop.validate_candidate(candidate, market))
        candidate["evidence"].append(
            {"claim": "Match completed and final score is official.", "source": "FIFA", "source_role": "official_match_result"}
        )
        self.assertEqual(loop.validate_candidate(candidate, market), [])

    def test_world_cup_candidate_requires_marker_when_category_only(self):
        market = {"marketType": "lognormal", "category": "World Cup", "title": "When will France be eliminated?"}
        candidate = {
            "id": "wc-2",
            "market": "0x2",
            "family": "lognormal",
            "belief": 3.3,
            "belief_sigma": 0.27,
            "rationale": "Two current market-prior sources imply a wider distribution than the curve.",
            "evidence": [
                {"claim": "Odds source implies path risk.", "source": "Odds source"},
                {"claim": "Rating source implies team strength.", "source": "Rating source"},
            ],
        }
        self.assertIn("World Cup candidate needs post-result evidence marker", loop.validate_candidate(candidate, market))

    def test_world_cup_placeholder_result_marker_does_not_pass(self):
        market = {"marketType": "lognormal", "category": "World Cup", "title": "When will France be eliminated?"}
        candidate = {
            "id": "wc-placeholder",
            "market": "0x2",
            "family": "lognormal",
            "belief": 3.3,
            "belief_sigma": 0.27,
            "rationale": "Two current market-prior sources imply a wider distribution than the curve.",
            "evidence": [
                {"claim": "Odds source implies path risk.", "source": "Odds source"},
                {
                    "claim": "TO_FILL: final score.",
                    "source": "FIFA",
                    "source_role": "official_match_result",
                    "url": "TO_FILL",
                    "post_result": False,
                },
            ],
        }
        self.assertIn("World Cup candidate needs post-result evidence marker", loop.validate_candidate(candidate, market))

    def test_concentration_guard_blocks_third_market_lot(self):
        market = {
            "address": "0x1",
            "marketType": "normal",
            "category": "Economics",
            "title": "US Inflation in June 2026 (CPI YoY)",
            "resolution": {"metric": "CPI YoY", "units": "%"},
        }
        candidate = {"id": "cpi-1", "market": "0x01"}
        positions = [{"marketAddress": "0x1", "hasPosition": True, "deltaCount": 2}]
        errors = loop.concentration_errors(candidate, market, positions, [market])
        self.assertTrue(any("market concentration cap" in error for error in errors))
        self.assertTrue(any("settlement concentration cap" in error for error in errors))

    def test_concentration_guard_allows_first_lot_on_new_market(self):
        market = {"address": "0x2", "marketType": "lognormal", "title": "France World Cup wins"}
        candidate = {"id": "fra-1", "market": "0x2"}
        positions = [{"marketAddress": "0x1", "hasPosition": True, "deltaCount": 2}]
        self.assertEqual(loop.concentration_errors(candidate, market, positions, [market]), [])

    def test_summary_key_captures_health_and_candidate_changes(self):
        summary = {
            "rankings": {
                "overall": {"rank": 4, "gap_to_first": 479.0864, "pnl": 50.0161},
                "filters": {
                    "economics": {"healthy": False},
                    "cpi": {
                        "healthy": True,
                        "rank": 2,
                        "pnl": 12.34567,
                        "gap_to_first": 3.21098,
                        "top_pnl": 15.55665,
                        "markets_traded": 1,
                        "total_trades": 2,
                    },
                },
            },
            "gas_tier": "ok",
            "active_portfolio_scout": {
                "generated_at": "2026-06-12T17:05:52Z",
                "coverage": {
                    "active_tradeable_markets": 11,
                    "covered_active_tradeable_markets": 11,
                    "coverage_complete": True,
                },
                "rows": 66,
                "ev_floor_rows": 18,
                "runner_pass_rows": 0,
            },
            "processed_candidates": [{"id": "a", "status": "dry_run_ok"}],
            "templates": [
                {
                    "id": "germany-template",
                    "result_not_before_utc": "2026-06-12T00:00:00Z",
                    "blockers": ["missing_official_result_evidence"],
                    "opportunity_status": "weak_watch",
                }
            ],
        }
        key = loop.summary_key(summary)
        self.assertEqual(key["rank"], 4)
        self.assertEqual(key["unhealthy_filters"], ["economics"])
        self.assertEqual(key["unhealthy_time_windows"], [])
        self.assertEqual(key["unhealthy_filter_time_windows"], [])
        self.assertEqual(
            key["healthy_view_ranks"]["filters"]["cpi"],
            {
                "rank": 2,
                "pnl": 12.3457,
                "gap_to_first": 3.211,
                "top_pnl": 15.5566,
                "markets_traded": 1,
                "total_trades": 2,
            },
        )
        self.assertEqual(
            key["active_portfolio_scout"],
            {
                "active_tradeable": 11,
                "covered": 11,
                "coverage_complete": True,
                "rows": 66,
                "ev_floor_rows": 18,
                "runner_pass_rows": 0,
                "top_signals": [],
            },
        )
        self.assertEqual(key["processed"], [{"id": "a", "status": "dry_run_ok"}])
        self.assertEqual(
            key["post_result_evidence_due"],
            [{"id": "germany-template", "opportunity_status": "weak_watch"}],
        )

    def test_summary_key_ignores_routine_scout_refresh_timestamps(self):
        base = {
            "rankings": {
                "overall": {"rank": 10, "gap_to_first": 915.922009, "pnl": 79.244219},
                "filters": {},
                "time_windows": {},
                "filter_time_windows": {},
            },
            "gas_tier": "ok",
            "active_portfolio_scout": {
                "generated_at": "2026-06-13T03:59:32Z",
                "coverage": {
                    "active_tradeable_markets": 11,
                    "covered_active_tradeable_markets": 11,
                    "coverage_complete": True,
                },
                "rows": 66,
                "ev_floor_rows": 18,
                "runner_pass_rows": 0,
                "top_signals": [
                    {
                        "label": "CPI June Cleveland nowcast",
                        "budget": 250,
                        "expected_value": 54.76837161185969,
                        "would_pass_current_runner": False,
                        "blockers": ["market concentration cap"],
                    }
                ],
            },
            "active_portfolio_scout_refresh": {
                "status": "refreshed",
                "attempted": True,
                "refreshed": True,
                "generated_at": "2026-06-13T03:59:32Z",
            },
        }
        routine = json.loads(json.dumps(base))
        routine["active_portfolio_scout"]["generated_at"] = "2026-06-13T04:59:32Z"
        routine["active_portfolio_scout_refresh"] = {
            "status": "fresh",
            "attempted": False,
            "refreshed": False,
            "previous_age_seconds": 60,
        }

        self.assertEqual(loop.summary_key(base), loop.summary_key(routine))

    def test_summary_key_records_scout_refresh_failure(self):
        summary = {
            "rankings": {
                "overall": {"rank": 10, "gap_to_first": 915.922009, "pnl": 79.244219},
                "filters": {},
                "time_windows": {},
                "filter_time_windows": {},
            },
            "gas_tier": "ok",
            "active_portfolio_scout_refresh": {
                "status": "failed",
                "returncode": 1,
                "error_tail": ["could not derive belief"],
            },
        }

        key = loop.summary_key(summary)

        self.assertEqual(
            key["active_portfolio_scout_refresh"],
            {
                "status": "failed",
                "returncode": 1,
                "error_tail": ["could not derive belief"],
            },
        )

    def test_mailbox_update_migrates_legacy_scout_key_without_entry(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            mailbox = Path(tmpdir) / "mailbox.md"
            mailbox.write_text("# Mailbox\n", encoding="utf-8")
            summary = {
                "rankings": {
                    "overall": {"rank": 10, "gap_to_first": 915.922009, "pnl": 79.244219},
                    "filters": {},
                    "time_windows": {},
                    "filter_time_windows": {},
                },
                "markets": {"active_tradeable": 11},
                "account": {"strk_balance_strk": 1042.0},
                "collateral": {"balance_xp": 19832.0},
                "gas_tier": "ok",
                "active_portfolio_scout": {
                    "generated_at": "2026-06-13T05:07:59Z",
                    "coverage": {
                        "active_tradeable_markets": 11,
                        "covered_active_tradeable_markets": 11,
                        "coverage_complete": True,
                    },
                    "rows": 66,
                    "ev_floor_rows": 18,
                    "runner_pass_rows": 0,
                    "top_signals": [
                        {
                            "label": "CPI June Cleveland nowcast",
                            "budget": 250,
                            "expected_value": 54.76837161185969,
                            "would_pass_current_runner": False,
                            "blockers": ["market concentration cap"],
                        }
                    ],
                },
                "active_portfolio_scout_refresh": {
                    "status": "fresh",
                    "attempted": False,
                    "refreshed": False,
                    "previous_age_seconds": 90,
                },
            }
            legacy_key = {
                "rank": 10,
                "gap": 915.922,
                "pnl": 79.2442,
                "gas_tier": "ok",
                "unhealthy_filters": [],
                "unhealthy_time_windows": [],
                "unhealthy_filter_time_windows": [],
                "healthy_view_ranks": {"filters": {}, "time_windows": {}, "filter_time_windows": {}},
                "active_portfolio_scout": {
                    "generated_at": "2026-06-13T03:59:32Z",
                    "active_tradeable": 11,
                    "covered": 11,
                    "coverage_complete": True,
                    "rows": 66,
                    "ev_floor_rows": 18,
                    "runner_pass_rows": 0,
                },
                "active_portfolio_scout_refresh": {
                    "status": "fresh",
                    "attempted": False,
                    "refreshed": False,
                },
                "processed": [],
                "promoted_templates": [],
                "post_result_evidence_due": [],
            }
            state = {"last_mailbox_key": legacy_key}

            updated = loop.append_mailbox_if_changed(mailbox, state, summary)

            self.assertFalse(updated)
            self.assertEqual(mailbox.read_text(encoding="utf-8"), "# Mailbox\n")
            self.assertEqual(state["last_mailbox_key"], loop.summary_key(summary))

    def test_mailbox_update_records_real_scout_signal_change_after_migration(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            mailbox = Path(tmpdir) / "mailbox.md"
            mailbox.write_text("# Mailbox\n", encoding="utf-8")
            summary = {
                "rankings": {
                    "overall": {"rank": 10, "gap_to_first": 915.922009, "pnl": 79.244219},
                    "filters": {},
                    "time_windows": {},
                    "filter_time_windows": {},
                },
                "markets": {"active_tradeable": 11},
                "account": {"strk_balance_strk": 1042.0},
                "collateral": {"balance_xp": 19832.0},
                "gas_tier": "ok",
                "active_portfolio_scout": {
                    "coverage": {
                        "active_tradeable_markets": 11,
                        "covered_active_tradeable_markets": 11,
                        "coverage_complete": True,
                    },
                    "rows": 66,
                    "ev_floor_rows": 18,
                    "runner_pass_rows": 0,
                    "top_signals": [
                        {
                            "label": "Germany higher",
                            "budget": 100,
                            "expected_value": 21.8469,
                            "would_pass_current_runner": False,
                            "blockers": ["missing post-result evidence"],
                        }
                    ],
                },
            }
            state = {"last_mailbox_key": loop.summary_key(summary)}
            summary["active_portfolio_scout"]["top_signals"][0]["expected_value"] = 24.2

            updated = loop.append_mailbox_if_changed(mailbox, state, summary)

            self.assertTrue(updated)
            self.assertIn("Active portfolio scout", mailbox.read_text(encoding="utf-8"))

    def test_execute_mode_rechecks_fresh_gas_before_candidate_write(self):
        market = {
            "address": "0x1",
            "marketType": "normal",
            "category": "Economics",
            "title": "US Inflation CPI",
            "isActive": True,
            "state": {"isInitialized": True},
        }
        candidate = {
            "id": "cpi-gas-stop",
            "market": "0x1",
            "family": "normal",
            "belief": 3.8,
            "belief_sigma": 0.16,
            "budget": 100.0,
            "rationale": "BLS and nowcast evidence imply a cooler CPI print than the current curve.",
            "evidence": [{"claim": "CPI source", "source": "BLS", "url": "https://www.bls.gov/cpi/"}],
        }
        args = SimpleNamespace(execute=True, trade_journal=Path("/tmp/storm-deadeye-test-journal.jsonl"))
        with (
            mock.patch.object(
                loop,
                "account_snapshot",
                return_value={"account": {"strk_balance_strk": 24.0}, "collateral": {"balance_xp": 19800.0}},
            ),
            mock.patch.object(loop, "deadeye_json", side_effect=AssertionError("write path should stop before doctor")),
        ):
            processed = loop.process_candidates(
                [candidate],
                [market],
                [],
                {"strk_balance_strk": 26.0},
                {"balance_xp": 19800.0},
                {"totalPnl": 83.0},
                {},
                args,
                Path("/tmp/storm-deadeye-test-events.jsonl"),
            )
        self.assertEqual(processed[0]["status"], "write_stopped")
        self.assertIn("fresh STRK balance", processed[0]["reason"])

    def test_smoke_script_retries_one_read_only_failure(self):
        script = Path("/tmp/storm-deadeye-smoke-retry.sh")
        results = [
            loop.CmdResult(["zsh", str(script), "0x1"], 1, "PASS version\n== RESULT: FAIL ==\n", ""),
            loop.CmdResult(["zsh", str(script), "0x1"], 0, "PASS version\n== RESULT: ALL-PASS ==\n", ""),
        ]
        with (
            mock.patch.object(Path, "exists", return_value=True),
            mock.patch.object(loop, "run_cmd", side_effect=results) as run_cmd,
            mock.patch.object(loop.time, "sleep") as sleep,
        ):
            smoke = loop.run_smoke(script, "0x1")

        self.assertTrue(smoke["ok"])
        self.assertEqual(len(smoke["attempts"]), 2)
        self.assertEqual(smoke["attempts"][0]["returncode"], 1)
        self.assertEqual(smoke["attempts"][1]["returncode"], 0)
        self.assertEqual(run_cmd.call_count, 2)
        sleep.assert_called_once_with(loop.SMOKE_SCRIPT_RETRY_DELAY_SECONDS)

    def test_run_cmd_retries_read_only_cli_blip(self):
        procs = [
            SimpleNamespace(returncode=1, stdout="", stderr="temporary dns failure"),
            SimpleNamespace(returncode=0, stdout="ok\n", stderr=""),
        ]
        with (
            mock.patch.object(loop.subprocess, "run", side_effect=procs) as run,
            mock.patch.object(loop.time, "sleep") as sleep,
        ):
            result = loop.run_cmd(["deadeye", "markets", "list"], attempts=3)

        self.assertEqual(result.returncode, 0)
        self.assertEqual(run.call_count, 2)
        sleep.assert_called_once_with(2.0)

    def test_successful_tick_clears_stale_last_error(self):
        state = {"last_error": {"type": "loop.error", "error": "old smoke failure"}}
        loop.clear_last_error_after_success(state)
        self.assertNotIn("last_error", state)

    def test_compact_last_summary_keeps_operator_fields_without_raw_config(self):
        summary = {
            "generated_at": "2026-06-12T13:52:35Z",
            "execute_mode": True,
            "mailbox_updated": False,
            "smoke": {"ok": True, "script": "/tmp/smoke.sh"},
            "gas_tier": "ok",
            "account": {
                "address": "0x4418",
                "rpc_url": "https://rpc.test",
                "indexer_url": "https://indexer.test",
                "strk_balance_strk": 1042.0,
                "strk_balance_base": 1042000000000000000000,
            },
            "collateral": {
                "balance_xp": 19832.0,
                "balance_raw_hex": "0xabc",
                "grant_raw_hex": "0xdef",
            },
            "markets": {"active_tradeable": 11, "total": 12},
            "positions_count": 1,
            "rankings": {
                "overall": {
                    "rank": 9,
                    "pnl": 82.0,
                    "gap_to_first": 1328.0,
                    "top_pnl": 1410.0,
                    "top_trader": "0xaaa",
                    "markets_traded": 1,
                    "total_trades": 2,
                },
                "filters": {
                    "world-cup": {"healthy": False},
                    "economics": {
                        "healthy": True,
                        "rank": 1,
                        "pnl": 44.4,
                        "gap_to_first": 0.0,
                        "top_pnl": 44.4,
                        "markets_traded": 1,
                        "total_trades": 1,
                    },
                },
                "time_windows": {"last-24h": {"healthy": False}},
                "filter_time_windows": {},
            },
            "processed_candidates": [{"id": "candidate-1", "status": "dry_run_ok"}],
            "active_portfolio_scout": {
                "generated_at": "2026-06-12T17:05:52Z",
                "coverage": {
                    "active_tradeable_markets": 11,
                    "covered_active_tradeable_markets": 11,
                    "coverage_complete": True,
                },
                "rows": 66,
                "ev_floor_rows": 18,
                "runner_pass_rows": 0,
                "top_signals": [{"label": "CPI", "expected_value": 22.4}],
            },
            "templates": [
                {
                    "id": "germany-template",
                    "label": "Germany higher",
                    "result_not_before_utc": "2026-06-14T20:00:00Z",
                    "blockers": ["template_result_window_not_reached"],
                    "opportunity_status": "weak_watch",
                    "quote_expected_value_xp": 22.0,
                    "belief_gap_improvement_xp": 22.1,
                }
            ],
            "trade_journal": "/tmp/journal.jsonl",
        }

        compact = loop.compact_last_summary(summary)
        self.assertEqual(compact["overall"]["rank"], 9)
        self.assertEqual(compact["strk_balance"], 1042.0)
        self.assertEqual(compact["xp_balance"], 19832.0)
        self.assertEqual(compact["healthy_views"]["filters"], ["economics"])
        self.assertEqual(compact["unhealthy_views"]["filters"], ["world-cup"])
        self.assertEqual(compact["healthy_view_stats"]["filters"]["economics"]["rank"], 1)
        self.assertEqual(compact["healthy_view_stats"]["filters"]["economics"]["gap_to_first"], 0.0)
        self.assertEqual(compact["active_portfolio_scout"]["coverage"]["covered_active_tradeable_markets"], 11)
        self.assertEqual(compact["active_portfolio_scout"]["runner_pass_rows"], 0)
        self.assertEqual(compact["processed_candidates"], [{"id": "candidate-1", "status": "dry_run_ok"}])
        self.assertEqual(compact["next_template_window"]["id"], "germany-template")
        self.assertEqual(compact["next_template_window"]["result_not_before_utc"], "2026-06-14T20:00:00Z")
        self.assertIsNone(compact["next_durable_template_window"])
        serialized = json.dumps(compact, sort_keys=True)
        for forbidden in ("rpc_url", "indexer_url", "raw_hex", "trade_journal", "script"):
            self.assertNotIn(forbidden, serialized)

    def test_public_loop_summary_uses_compact_no_raw_output(self):
        summary = {
            "generated_at": "2026-06-12T15:40:44Z",
            "execute_mode": True,
            "mailbox_updated": False,
            "smoke": {"ok": True, "script": "/tmp/smoke.sh"},
            "gas_tier": "ok",
            "account": {
                "address": "0x4418",
                "rpc_url": "https://rpc.test",
                "indexer_url": "https://indexer.test",
                "strk_balance_strk": 1042.0,
                "strk_balance_base": 1042000000000000000000,
            },
            "collateral": {
                "balance_xp": 19832.0,
                "balance_raw_hex": "0xabc",
                "grant_raw_hex": "0xdef",
            },
            "markets": {"active_tradeable": 11, "total": 12},
            "positions_count": 1,
            "rankings": {
                "overall": {
                    "rank": 8,
                    "pnl": 82.0,
                    "gap_to_first": 913.0,
                    "top_pnl": 995.0,
                    "top_trader": "0xaaa",
                    "markets_traded": 1,
                    "total_trades": 2,
                },
                "filters": {},
                "time_windows": {},
                "filter_time_windows": {},
            },
            "processed_candidates": [],
            "templates": [],
            "template_promotion": {"promoted": [], "skipped": []},
            "state_dir": "/tmp/state",
            "candidate_file": "/tmp/candidates.jsonl",
            "events_file": "/tmp/events.jsonl",
            "trade_journal": "/tmp/journal.jsonl",
        }

        public = loop.public_loop_summary(summary)

        self.assertEqual(public["overall"]["rank"], 8)
        serialized = json.dumps(public, sort_keys=True)
        for forbidden in (
            "rpc_url",
            "indexer_url",
            "raw_hex",
            "trade_journal",
            "candidate_file",
            "events_file",
            "state_dir",
            "script",
        ):
            self.assertNotIn(forbidden, serialized)

    def test_latest_active_portfolio_scout_summarizes_saved_artifact(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            state_dir = Path(tmpdir)
            artifact = {
                "generated_at": "2026-06-12T17:05:52Z",
                "coverage": {
                    "active_tradeable_markets": 11,
                    "covered_active_tradeable_markets": 11,
                    "coverage_complete": True,
                },
                "results": [
                    {
                        "label": "CPI",
                        "budget": 250,
                        "quote": {"expected_value": 22.4},
                        "runner_gate": {
                            "would_pass_current_runner": False,
                            "blockers": ["market concentration cap"],
                        },
                    },
                    {
                        "label": "Germany",
                        "budget": 100,
                        "quote": {"expected_value": 21.8},
                        "runner_gate": {
                            "would_pass_current_runner": True,
                            "blockers": [],
                        },
                    },
                    {
                        "label": "Spain",
                        "budget": 100,
                        "quote": {"expected_value": 9.7},
                        "runner_gate": {
                            "would_pass_current_runner": False,
                            "blockers": ["below floor"],
                        },
                    },
                ],
            }
            (state_dir / "gap-analysis-active-portfolio-ladder-quote-test.json").write_text(
                json.dumps(artifact),
                encoding="utf-8",
            )

            summary = loop.latest_active_portfolio_scout(state_dir)

        self.assertEqual(summary["generated_at"], "2026-06-12T17:05:52Z")
        self.assertEqual(summary["rows"], 3)
        self.assertEqual(summary["ev_floor_rows"], 2)
        self.assertEqual(summary["runner_pass_rows"], 1)
        self.assertEqual(summary["top_signals"][0]["label"], "CPI")
        serialized = json.dumps(summary, sort_keys=True)
        self.assertNotIn("indexer_url", serialized)
        self.assertNotIn("trader", serialized)

    def test_active_portfolio_scout_stale_check(self):
        scout = {"generated_at": "2026-06-12T17:00:00Z"}
        now = loop.parse_utc_timestamp("2026-06-12T17:30:00Z")
        self.assertFalse(loop.active_portfolio_scout_is_stale(scout, max_age_minutes=60, now=now))
        self.assertTrue(loop.active_portfolio_scout_is_stale(scout, max_age_minutes=20, now=now))
        self.assertTrue(loop.active_portfolio_scout_is_stale(None, max_age_minutes=60, now=now))

    def test_refresh_active_portfolio_scout_skips_fresh_artifact(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            state_dir = Path(tmpdir)
            artifact = {
                "generated_at": loop.utc_now(),
                "coverage": {"active_tradeable_markets": 1, "covered_active_tradeable_markets": 1},
                "results": [],
            }
            (state_dir / "gap-analysis-active-portfolio-ladder-quote-fresh.json").write_text(
                json.dumps(artifact),
                encoding="utf-8",
            )
            with mock.patch.object(loop, "run_cmd") as run_cmd:
                result = loop.refresh_active_portfolio_scout_if_stale(state_dir, max_age_minutes=60)

        self.assertEqual(result["status"], "fresh")
        self.assertFalse(result["attempted"])
        run_cmd.assert_not_called()

    def test_refresh_active_portfolio_scout_runs_quote_only_without_operational_context(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            state_dir = Path(tmpdir)
            calls = []

            def fake_run_cmd(args, *, timeout, check):
                calls.append(args)
                output_path = Path(args[args.index("--output") + 1])
                output_path.write_text(
                    json.dumps({
                        "generated_at": "2026-06-12T17:05:52Z",
                        "coverage": {
                            "active_tradeable_markets": 11,
                            "covered_active_tradeable_markets": 11,
                            "coverage_complete": True,
                        },
                        "results": [
                            {
                                "label": "Germany",
                                "budget": 100,
                                "quote": {"expected_value": 21.8},
                                "runner_gate": {"would_pass_current_runner": False, "blockers": []},
                            }
                        ],
                    }),
                    encoding="utf-8",
                )
                return loop.CmdResult(args, 0, "{}", "")

            with mock.patch.object(loop, "run_cmd", side_effect=fake_run_cmd):
                result = loop.refresh_active_portfolio_scout_if_stale(state_dir, max_age_minutes=60)

        self.assertEqual(result["status"], "refreshed")
        self.assertTrue(result["attempted"])
        self.assertEqual(result["generated_at"], "2026-06-12T17:05:52Z")
        self.assertEqual(len(calls), 1)
        cmd = calls[0]
        self.assertIn("--quote-only", cmd)
        self.assertIn("active-portfolio-20260612", cmd)
        self.assertNotIn("--indexer-url", cmd)
        self.assertNotIn("--trader", cmd)

    def test_next_template_window_picks_earliest_future_window(self):
        templates = [
            {
                "id": "argentina-template",
                "label": "Argentina lower/wider",
                "result_not_before_utc": "2026-06-13T20:00:00Z",
                "blockers": ["template_result_window_not_reached", "template_ev_below_floor"],
                "opportunity_status": "durable_watch",
            },
            {
                "id": "france-template",
                "label": "France lower/wider",
                "result_not_before_utc": "2026-06-16T22:00:00Z",
                "blockers": ["template_result_window_not_reached"],
                "opportunity_status": "durable_watch",
            },
            {
                "id": "germany-template",
                "label": "Germany higher",
                "result_not_before_utc": "2026-06-14T20:00:00Z",
                "blockers": ["template_result_window_not_reached"],
                "opportunity_status": "weak_watch",
            },
            {
                "id": "already-open-template",
                "label": "Already open",
                "result_not_before_utc": "2026-06-12T00:00:00Z",
                "blockers": [],
            },
        ]

        next_window = loop.next_template_window(
            templates,
            now=loop.parse_utc_timestamp("2026-06-12T14:00:00Z"),
        )

        self.assertEqual(next_window["id"], "argentina-template")
        self.assertEqual(next_window["result_not_before_utc"], "2026-06-13T20:00:00Z")

        next_durable_window = loop.next_template_window(
            templates,
            now=loop.parse_utc_timestamp("2026-06-12T14:00:00Z"),
            queueable_opportunities_only=True,
        )

        self.assertEqual(next_durable_window["id"], "france-template")
        self.assertEqual(next_durable_window["result_not_before_utc"], "2026-06-16T22:00:00Z")

    def test_post_result_evidence_due_after_window(self):
        templates = [
            {
                "id": "germany-template",
                "label": "Germany higher",
                "result_not_before_utc": "2026-06-14T20:00:00Z",
                "blockers": [
                    "missing_official_result_evidence",
                    "missing_world_cup_post_result",
                    "evidence_url_to_fill",
                    "template_opportunity_not_durable",
                ],
                "opportunity_status": "weak_watch",
                "quote_expected_value_xp": 21.8,
                "belief_gap_improvement_xp": 21.8,
            },
            {
                "id": "france-template",
                "label": "France lower/wider",
                "result_not_before_utc": "2026-06-16T22:00:00Z",
                "blockers": ["template_result_window_not_reached", "missing_official_result_evidence"],
                "opportunity_status": "avoid",
            },
        ]

        due = loop.post_result_evidence_due(
            templates,
            now=loop.parse_utc_timestamp("2026-06-14T20:30:00Z"),
        )

        self.assertEqual(len(due), 1)
        self.assertEqual(due[0]["id"], "germany-template")
        self.assertEqual(due[0]["result_not_before_utc"], "2026-06-14T20:00:00Z")
        self.assertIn("missing_official_result_evidence", due[0]["blockers"])

    def test_mailbox_update_names_post_result_evidence_due(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            mailbox = Path(tmpdir) / "mailbox.md"
            mailbox.write_text("# Mailbox\n", encoding="utf-8")
            state = {}
            summary = {
                "rankings": {
                    "overall": {"rank": 8, "gap_to_first": 913.0, "pnl": 82.0},
                    "filters": {},
                    "time_windows": {},
                    "filter_time_windows": {},
                },
                "markets": {"active_tradeable": 11},
                "account": {"strk_balance_strk": 1042.0},
                "collateral": {"balance_xp": 19832.0},
                "gas_tier": "ok",
                "processed_candidates": [],
                "templates": [
                    {
                        "id": "germany-template",
                        "label": "Germany higher",
                        "result_not_before_utc": "2026-06-12T00:00:00Z",
                        "blockers": ["missing_official_result_evidence"],
                        "opportunity_status": "weak_watch",
                    }
                ],
            }

            updated = loop.append_mailbox_if_changed(mailbox, state, summary)
            text = mailbox.read_text(encoding="utf-8")

        self.assertTrue(updated)
        self.assertIn("Post-result evidence due", text)
        self.assertIn("germany-template", text)

    def test_load_template_status_reports_placeholder_blockers(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "france.json"
            template_path.write_text(json.dumps({
                "id": "france-template",
                "disabled": True,
                "template_status": "draft_only_not_queue_active",
                "prepared_at": "2026-06-12T13:20:00Z",
                "prepared_from": {
                    "label": "France lower/wider",
                    "status": "durable_watch",
                    "quote_expected_value_xp": 14.3,
                    "belief_gap_improvement_xp": 375.5,
                    "current_blocker": "World Cup probe has no post-result evidence marker",
                },
                "market": "0x1",
                "family": "lognormal",
                "budget": 100,
                "min_expected_value": 10,
                "world_cup_post_result": False,
                "evidence": [
                    {"claim": "Scheduled fixture.", "source": "FIFA", "post_result": False},
                    {
                        "claim": "TO_FILL: final score.",
                        "source": "FIFA",
                        "source_role": "official_match_result",
                        "url": "TO_FILL",
                        "post_result": False,
                    },
                ],
            }), encoding="utf-8")

            statuses = loop.load_template_status(Path(tmpdir))

        self.assertEqual(len(statuses), 1)
        status = statuses[0]
        self.assertFalse(status["queue_ready"])
        self.assertEqual(status["file"], "france.json")
        self.assertEqual(status["label"], "France lower/wider")
        self.assertIn("disabled", status["blockers"])
        self.assertIn("evidence_url_to_fill", status["blockers"])
        self.assertIn("missing_official_result_evidence", status["blockers"])
        self.assertIn("missing_world_cup_post_result", status["blockers"])

    def test_template_status_blocks_stale_ev_below_floor(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "argentina.json"
            template_path.write_text(json.dumps({
                "id": "argentina-template",
                "disabled": False,
                "template_status": "ready_to_queue",
                "prepared_from": {
                    "label": "Argentina lower/wider",
                    "quote_expected_value_xp": 1.35,
                },
                "market": "0x1",
                "family": "lognormal",
                "budget": 100,
                "min_expected_value": 10,
                "world_cup_post_result": True,
                "evidence": [
                    {
                        "claim": "Official final-score marker is present.",
                        "source": "FIFA",
                        "source_role": "official_match_result",
                        "url": "https://www.fifa.com/en/match-centre/match/example",
                        "post_result": True,
                    },
                    {"claim": "Market-prior source remains current.", "source": "Market source"},
                ],
            }), encoding="utf-8")

            statuses = loop.load_template_status(Path(tmpdir))

        self.assertEqual(len(statuses), 1)
        self.assertFalse(statuses[0]["queue_ready"])
        self.assertIn("template_ev_below_floor", statuses[0]["blockers"])

    def test_template_status_blocks_weak_watch(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            template_path.write_text(json.dumps({
                "id": "germany-template",
                "disabled": False,
                "template_status": "ready_to_queue",
                "prepared_from": {
                    "label": "Germany higher",
                    "status": "weak_watch",
                    "quote_expected_value_xp": 22.0,
                },
                "market": "0x1",
                "family": "lognormal",
                "budget": 100,
                "min_expected_value": 10,
                "world_cup_post_result": True,
                "evidence": [
                    {
                        "claim": "Official final-score marker is present.",
                        "source": "FIFA",
                        "source_role": "official_match_result",
                        "url": "https://www.fifa.com/en/match-centre/match/example",
                        "post_result": True,
                    },
                    {"claim": "Market-prior source remains current.", "source": "Market source"},
                ],
            }), encoding="utf-8")

            statuses = loop.load_template_status(Path(tmpdir))

        self.assertEqual(len(statuses), 1)
        self.assertFalse(statuses[0]["queue_ready"])
        self.assertIn("template_opportunity_not_durable", statuses[0]["blockers"])

    def test_template_status_blocks_future_result_window(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "france.json"
            template_path.write_text(json.dumps({
                "id": "france-template",
                "disabled": False,
                "template_status": "ready_to_queue",
                "prepared_from": {
                    "label": "France lower/wider",
                    "status": "durable_watch",
                    "quote_expected_value_xp": 14.3,
                },
                "market": "0x1",
                "family": "lognormal",
                "budget": 100,
                "min_expected_value": 10,
                "result_not_before_utc": "2999-01-01T00:00:00Z",
                "world_cup_post_result": True,
                "evidence": [
                    {
                        "claim": "Official final-score marker is present.",
                        "source": "FIFA",
                        "source_role": "official_match_result",
                        "url": "https://www.fifa.com/en/match-centre/match/example",
                        "post_result": True,
                    },
                    {"claim": "Market-prior source remains current.", "source": "Market source"},
                ],
            }), encoding="utf-8")

            statuses = loop.load_template_status(Path(tmpdir))

        self.assertEqual(len(statuses), 1)
        self.assertFalse(statuses[0]["queue_ready"])
        self.assertEqual(statuses[0]["result_not_before_utc"], "2999-01-01T00:00:00Z")
        self.assertIn("template_result_window_not_reached", statuses[0]["blockers"])


if __name__ == "__main__":
    unittest.main()
