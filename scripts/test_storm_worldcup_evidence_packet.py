#!/usr/bin/env python3
import json
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import storm_worldcup_evidence_packet as packet


def write_germany_template(path: Path):
    template = {
        "id": "germany-post-result-snap-template-20260612",
        "disabled": True,
        "template_status": "draft_only_not_queue_active",
        "prepared_from": {
            "label": "Germany higher",
            "status": "weak_watch",
            "quote_expected_value_xp": 21.8,
        },
        "market": "0x1e7",
        "family": "lognormal",
        "belief": 3.2291,
        "belief_sigma": 0.2702,
        "budget": 100,
        "min_expected_value": 10,
        "result_not_before_utc": "2026-06-14T20:00:00Z",
        "world_cup_post_result": False,
        "pre_result_baseline_captured_at": "2026-06-13T05:37:00Z",
        "pre_result_baseline": {
            "official_fixture_url": "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
            "team_news_urls": [
                "https://bulinews.com/germany-curacao-preview-team-news-and-predicted-lineups",
                "https://www.sportsmole.co.uk/football/germany/world-cup-2026/preview/germany-vs-curacao-prediction-team-news-lineups_599044.html",
                "https://www.standard.co.uk/sport/football/germany-vs-curacao-prediction-kick-off-time-tv-live-stream-team-news-latest-h2h-results-odds-world-cup-2026-preview-b1285707.html",
            ],
            "ratings_context_urls": ["https://www.bundesliga.com/example"],
            "ratings_snapshot_captured_at": "2026-06-13T06:41:00Z",
            "ratings_snapshot": {
                "source": "FIFA/Coca-Cola Men's World Ranking team pages",
                "urls": {
                    "germany": "https://inside.fifa.com/fifa-world-ranking/GER?gender=men",
                    "curacao": "https://inside.fifa.com/fifa-world-ranking/CUW?gender=men",
                },
                "updated_at": "2026-06-11",
                "next_official_update": "2026-07-20",
                "ranks": {
                    "germany": 10,
                    "curacao": 82,
                },
            },
            "odds_context_urls": ["https://www.sportytrader.com/example"],
            "odds_snapshot_captured_at": "2026-06-13T06:35:00Z",
            "odds_snapshot": {
                "source": "SportyTrader full-time result best-odds summary",
                "url": "https://www.sportytrader.com/en/odds/germany-curacao-7937446/",
                "decimal_odds": {
                    "germany": 1.06,
                    "draw": 19.5,
                    "curacao": 60.0,
                },
            },
        },
        "post_result_capture_required": [
            "official completed-match marker",
            "official final score",
            "official confirmed lineups",
            "post-result Germany outright/path odds",
            "fresh quote scout",
        ],
        "evidence": [
            {
                "claim": "FIFA lists Germany vs Curacao as scheduled.",
                "source_role": "official_fixture",
                "source": "FIFA match centre",
                "url": "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
                "post_result": False,
            },
            {
                "claim": "TO_FILL: final score.",
                "source_role": "official_match_result",
                "source": "FIFA match centre",
                "url": "TO_FILL",
                "post_result": False,
            },
        ],
    }
    path.write_text(json.dumps(template), encoding="utf-8")


def fill_packet_evidence(result: dict):
    claims = {
        "official_result": "FIFA official final score Germany 3-0 Curacao and completed marker captured.",
        "confirmed_lineups": "FIFA confirmed lineups and starting XI for Germany and Curacao were captured after full time.",
        "injuries_suspensions": "Post-match source checked injuries, suspensions, bookings, and absences affecting Germany path impact.",
        "odds_move": "Post-result Germany odds movement versus the pre-result baseline Germany 1.06 with post_result_value updated odds Germany 1.04 captured.",
        "ratings_move": "Post-result ratings/model movement versus baseline FIFA ranks Germany 10 and Curacao 82 with post_result_value updated model Germany 86.2 captured.",
        "market_state": "Fresh post-result Deadeye market state from deadeye markets show generated_at 2026-06-14T20:06:30Z with mu=3.2291 and sigma=0.2702 captured.",
        "quote_scout": "Fresh active-portfolio quote scout EV output gap-analysis-active-portfolio-ladder-quote-4000-20260614T200700Z.json generated_at 2026-06-14T20:07:00Z with runner_pass_rows 0 captured after result/state shift.",
    }
    for item in result["evidence_placeholders"]:
        item["status"] = "captured"
        item["post_result"] = True
        item["capture_utc"] = "2026-06-14T20:04:00Z"
        item["claim"] = claims[item["id"]]
        if item["url"] == "TO_FILL":
            item["url"] = "https://example.com/evidence"


def fill_packet_evidence_before_window(result: dict):
    fill_packet_evidence(result)
    for item in result["evidence_placeholders"]:
        item["capture_utc"] = "2026-06-14T19:59:00Z"


def fill_realistic_germany_result_evidence(result: dict):
    claims = {
        "official_result": "FIFA shows the match completed at full time with final score Germany 3-0 Curacao.",
        "confirmed_lineups": "FIFA confirmed lineups and starting XI for Germany and Curacao were captured after full time.",
        "injuries_suspensions": "Post-match report checked injuries, suspensions, bookings, and absences affecting Germany path impact.",
        "odds_move": "Post-result Germany outright odds and path odds movement versus baseline Germany 1.06 with post_result_value updated odds Germany 1.04 captured.",
        "ratings_move": "Post-result ratings model movement for Germany versus baseline ranks Germany 10 and Curacao 82 with post_result_value updated model Germany 86.2 captured.",
        "market_state": "Fresh post-result Deadeye market state from deadeye markets show generated_at 2026-06-14T20:06:30Z with mu=3.2291 and sigma=0.2702 captured.",
        "quote_scout": "Fresh active-portfolio quote scout EV and expected value output gap-analysis-active-portfolio-ladder-quote-4000-20260614T200700Z.json generated_at 2026-06-14T20:07:00Z with runner_pass_rows 0 captured after result/state shift.",
    }
    urls = {
        "official_result": "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
        "confirmed_lineups": "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
        "injuries_suspensions": "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
        "odds_move": "https://www.sportytrader.com/en/odds/germany-curacao-7937446/",
        "ratings_move": "https://www.bundesliga.com/en/bundesliga/news/how-will-germany-line-up-havertz-musiala-wirtz-nagelsmann-world-cup-2026-28807",
        "market_state": "local-cli",
        "quote_scout": "local-cli",
    }
    for item in result["evidence_placeholders"]:
        item["status"] = "captured"
        item["post_result"] = True
        item["capture_utc"] = "2026-06-14T20:07:00Z"
        item["claim"] = claims[item["id"]]
        item["url"] = urls[item["id"]]


class StormWorldCupEvidencePacketTests(unittest.TestCase):
    def test_packet_keeps_blocked_template_non_queueable(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            result = packet.build_packet(template_path, now="2026-06-13T05:42:00Z")

        self.assertFalse(result["queue_allowed"])
        self.assertFalse(result["queue_ready_now"])
        self.assertFalse(result["result_window_open"])
        self.assertIn("disabled", result["queue_blockers"])
        self.assertIn("missing_official_result_evidence", result["queue_blockers"])
        self.assertIn("missing_world_cup_post_result", result["queue_blockers"])
        self.assertFalse(result["capture_readiness"]["ready_for_template_update"])
        self.assertIn("result_window_not_open", result["capture_readiness"]["blockers"])
        self.assertEqual(result["capture_status"]["next_action"], "wait_for_result_window")
        self.assertEqual(result["capture_status"]["missing_ids"], list(packet.REQUIRED_EVIDENCE_IDS))
        self.assertEqual(result["capture_status"]["blocker_count"], 29)

    def test_packet_has_required_evidence_placeholders_and_commands(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")

        ids = {item["id"] for item in result["evidence_placeholders"]}
        self.assertTrue(result["result_window_open"])
        self.assertIn("official_result", ids)
        self.assertIn("confirmed_lineups", ids)
        self.assertIn("injuries_suspensions", ids)
        self.assertIn("odds_move", ids)
        self.assertIn("ratings_move", ids)
        self.assertIn("market_state", ids)
        self.assertIn("quote_scout", ids)
        self.assertEqual(
            result["source_urls"]["official"],
            ["https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464"],
        )
        self.assertTrue(any("deadeye markets show 0x1e7" in cmd for cmd in result["read_only_commands_after_result"]))
        self.assertTrue(any("storm_gap_analyzer.py" in cmd for cmd in result["read_only_commands_after_result"]))
        self.assertTrue(any("--validate-packet" in cmd for cmd in result["read_only_commands_after_result"]))
        status_rows = {row["id"]: row for row in result["capture_status"]["rows"]}
        self.assertFalse(status_rows["official_result"]["captured"])
        self.assertFalse(status_rows["official_result"]["claim_ready"])
        self.assertTrue(status_rows["official_result"]["url_ready"])
        placeholders = {item["id"]: item for item in result["evidence_placeholders"]}
        self.assertEqual(
            placeholders["official_result"]["source_options"],
            ["https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464"],
        )
        self.assertIn(
            "https://www.sportsmole.co.uk/football/germany/world-cup-2026/preview/germany-vs-curacao-prediction-team-news-lineups_599044.html",
            placeholders["injuries_suspensions"]["source_options"],
        )
        self.assertIn(
            "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
            placeholders["injuries_suspensions"]["source_options"],
        )
        self.assertEqual(
            placeholders["ratings_move"]["url"],
            "https://inside.fifa.com/fifa-world-ranking/GER?gender=men",
        )
        self.assertEqual(
            status_rows["ratings_move"]["source_options"],
            [
                "https://www.bundesliga.com/example",
                "https://inside.fifa.com/fifa-world-ranking/GER?gender=men",
                "https://inside.fifa.com/fifa-world-ranking/CUW?gender=men",
            ],
        )
        plan_rows = {row["id"]: row for row in result["capture_plan"]["rows"]}
        self.assertEqual(result["capture_plan"]["result_not_before_utc"], "2026-06-14T20:00:00Z")
        self.assertIn(
            "<row-specific claim_template with source values filled>",
            result["capture_plan"]["row_capture_command_template"],
        )
        self.assertNotIn("<specific claim>", result["capture_plan"]["row_capture_command_template"])
        self.assertEqual(
            plan_rows["official_result"]["capture_utc_must_be_at_or_after"],
            "2026-06-14T20:00:00Z",
        )
        self.assertEqual(plan_rows["official_result"]["source_role"], "official_match_result")
        self.assertEqual(
            plan_rows["official_result"]["primary_url"],
            "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
        )
        self.assertEqual(
            plan_rows["official_result"]["claim_template"],
            "FIFA shows the match completed at full time with final score Germany <score> Curacao.",
        )
        self.assertIn("--capture-row official_result", plan_rows["official_result"]["capture_command"])
        self.assertIn("'FIFA match centre'", plan_rows["official_result"]["capture_command"])
        self.assertIn(
            "https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
            plan_rows["official_result"]["capture_command"],
        )
        self.assertIn(
            "FIFA shows the match completed at full time with final score Germany <score> Curacao.",
            plan_rows["official_result"]["capture_command"],
        )
        self.assertNotIn("<specific claim>", plan_rows["official_result"]["capture_command"])
        self.assertIn(
            {"label": "numeric_score_value", "accepted_pattern": r"\b\d{1,2}\s*(?:-|:|\u2013|\u2014)\s*\d{1,2}\b"},
            plan_rows["official_result"]["claim_must_include"],
        )
        self.assertIn("deadeye markets show 0x1e7", plan_rows["market_state"]["read_only_command"])
        self.assertIn("storm_gap_analyzer.py", plan_rows["quote_scout"]["read_only_command"])
        self.assertEqual(
            plan_rows["market_state"]["claim_template"],
            "Fresh post-result Deadeye market state from deadeye markets show generated_at <timestamp> with mu=<mu> and sigma=<sigma> captured.",
        )
        self.assertIn(
            "Fresh post-result Deadeye market state from deadeye markets show generated_at <timestamp> with mu=<mu> and sigma=<sigma> captured.",
            plan_rows["market_state"]["capture_command"],
        )
        self.assertIn("mu_value", [
            marker["label"] for marker in plan_rows["market_state"]["claim_must_include"]
        ])
        self.assertIn("sigma_value", [
            marker["label"] for marker in plan_rows["market_state"]["claim_must_include"]
        ])
        self.assertIn("--capture-row market_state", plan_rows["market_state"]["capture_command"])
        self.assertIn("--url local-cli", plan_rows["market_state"]["capture_command"])
        self.assertIn("--capture-row quote_scout", plan_rows["quote_scout"]["capture_command"])
        self.assertIn("--url local-cli", plan_rows["quote_scout"]["capture_command"])
        for item_id, row in plan_rows.items():
            self.assertIn("claim_template", row, item_id)
            self.assertIn(row["claim_template"], row["capture_command"], item_id)
            self.assertNotIn("<specific claim>", row["capture_command"], item_id)
        self.assertIn("--validate-packet", result["capture_plan"]["validation_command"])
        self.assertIn("storm_deadeye_loop.py", result["capture_plan"]["runner_command"])
        reachability = result["source_reachability"]
        self.assertFalse(reachability["checked"])
        self.assertTrue(reachability["advisory_only"])
        self.assertEqual(reachability["url_count"], 9)
        self.assertEqual(reachability["probes"], [])
        self.assertFalse(result["pre_window_readiness"]["ready_for_result_window"])
        self.assertIn("source_reachability_not_checked", result["pre_window_readiness"]["blockers"])

    def test_source_checked_packet_is_pre_window_ready_but_not_queue_approved(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            def fake_probe(url, *, timeout_seconds, checked_at):
                return {
                    "url": url,
                    "checked_at": checked_at,
                    "status": 200,
                    "reachable": True,
                }

            with mock.patch.object(packet, "probe_source_url", side_effect=fake_probe):
                result = packet.build_packet(
                    template_path,
                    now="2026-06-13T08:30:00Z",
                    check_sources=True,
                    source_timeout_seconds=0.25,
                )

        readiness = result["pre_window_readiness"]
        self.assertTrue(readiness["ready_for_result_window"])
        self.assertEqual(readiness["blockers"], [])
        self.assertTrue(readiness["source_reachability_checked"])
        self.assertEqual(readiness["source_reachability_reachable_count"], 9)
        self.assertFalse(result["queue_allowed"])
        self.assertFalse(result["capture_readiness"]["ready_for_template_update"])
        self.assertEqual(result["capture_status"]["next_action"], "wait_for_result_window")

    def test_capture_plan_summary_is_operator_readable(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T08:30:00Z")

        summary = packet.capture_plan_summary(result)

        self.assertIn("Germany post-result capture plan", summary)
        self.assertIn("template: germany-post-result-snap-template-20260612", summary)
        self.assertIn("official_result: official_match_result", summary)
        self.assertIn("odds_move: odds_snapshot", summary)
        self.assertIn("post_result_numeric_value", summary)
        self.assertIn("deadeye markets show", summary)
        self.assertIn("--capture-row odds_move", summary)
        self.assertIn("runner_pass_rows", summary)
        self.assertIn("Runner: python3 scripts/storm_deadeye_loop.py", summary)
        self.assertNotIn("<specific claim>", summary)

    def test_pre_window_readiness_blocks_generic_claim_template(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            with mock.patch.object(
                packet,
                "probe_source_url",
                return_value={
                    "url": "https://example.com",
                    "checked_at": "2026-06-13T08:30:00Z",
                    "status": 200,
                    "reachable": True,
                },
            ):
                result = packet.build_packet(
                    template_path,
                    now="2026-06-13T08:30:00Z",
                    check_sources=True,
                    source_timeout_seconds=0.25,
                )
            for row in result["capture_plan"]["rows"]:
                if row["id"] == "official_result":
                    row["claim_template"] = "<specific claim>"
                    row["capture_command"] = row["capture_command"].replace(
                        "FIFA shows the match completed at full time with final score Germany <score> Curacao.",
                        "<specific claim>",
                    )

            readiness = packet.pre_window_readiness(result)

        self.assertFalse(readiness["ready_for_result_window"])
        self.assertIn("official_result:claim_template_placeholder", readiness["blockers"])

    def test_source_reachability_probe_is_advisory_and_row_mapped(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            def fake_probe(url, *, timeout_seconds, checked_at):
                return {
                    "url": url,
                    "checked_at": checked_at,
                    "status": 200 if "sportsmole" not in url else 0,
                    "reachable": "sportsmole" not in url,
                }

            with mock.patch.object(packet, "probe_source_url", side_effect=fake_probe) as probe:
                result = packet.build_packet(
                    template_path,
                    now="2026-06-13T07:30:00Z",
                    check_sources=True,
                    source_timeout_seconds=0.25,
                )

        reachability = result["source_reachability"]
        self.assertTrue(reachability["checked"])
        self.assertTrue(reachability["advisory_only"])
        self.assertEqual(reachability["checked_at"], "2026-06-13T07:30:00Z")
        self.assertEqual(reachability["url_count"], 9)
        self.assertEqual(reachability["unreachable_count"], 1)
        self.assertEqual(probe.call_count, 9)
        rows = {row["id"]: row for row in reachability["rows"]}
        self.assertIn(
            "https://www.sportsmole.co.uk/football/germany/world-cup-2026/preview/germany-vs-curacao-prediction-team-news-lineups_599044.html",
            rows["injuries_suspensions"]["unreachable_options"],
        )
        self.assertEqual(rows["market_state"]["source_options"], [])
        self.assertEqual(rows["quote_scout"]["source_options"], [])

    def test_validate_packet_check_sources_refreshes_reachability(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T07:30:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            with mock.patch.object(
                packet,
                "probe_source_url",
                return_value={
                    "url": "https://example.com",
                    "checked_at": "2026-06-13T07:31:00Z",
                    "status": 200,
                    "reachable": True,
                },
            ):
                validated = packet.validate_packet_file(
                    packet_path,
                    now="2026-06-13T07:31:00Z",
                    check_sources=True,
                    source_timeout_seconds=0.25,
                )

        self.assertTrue(validated["source_reachability"]["checked"])
        self.assertEqual(validated["source_reachability"]["checked_at"], "2026-06-13T07:31:00Z")
        self.assertEqual(validated["source_reachability"]["reachable_count"], 9)

    def test_packet_preserves_numeric_pre_result_odds_snapshot(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            result = packet.build_packet(template_path, now="2026-06-13T06:36:00Z")

        snapshot = result["pre_result_baseline"]["odds_snapshot"]
        self.assertEqual(snapshot["source"], "SportyTrader full-time result best-odds summary")
        self.assertEqual(snapshot["decimal_odds"]["germany"], 1.06)
        self.assertEqual(snapshot["decimal_odds"]["draw"], 19.5)
        self.assertEqual(snapshot["decimal_odds"]["curacao"], 60.0)

    def test_packet_preserves_official_ratings_snapshot(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            result = packet.build_packet(template_path, now="2026-06-13T06:42:00Z")

        snapshot = result["pre_result_baseline"]["ratings_snapshot"]
        self.assertEqual(snapshot["source"], "FIFA/Coca-Cola Men's World Ranking team pages")
        self.assertEqual(snapshot["updated_at"], "2026-06-11")
        self.assertEqual(snapshot["next_official_update"], "2026-07-20")
        self.assertEqual(snapshot["ranks"]["germany"], 10)
        self.assertEqual(snapshot["ranks"]["curacao"], 82)

    def test_filled_packet_capture_readiness_passes_after_window(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_packet_evidence(result)
            readiness = packet.capture_readiness(result, now="2026-06-14T20:06:00Z")
            result["capture_readiness"] = readiness
            status = packet.evidence_capture_status(result)

        self.assertTrue(readiness["ready_for_template_update"])
        self.assertEqual(readiness["blockers"], [])
        self.assertEqual(readiness["captured_ids"], list(packet.REQUIRED_EVIDENCE_IDS))
        self.assertEqual(status["next_action"], "apply_to_template")
        self.assertEqual(status["missing_ids"], [])
        self.assertTrue(all(row["captured"] for row in status["rows"]))

    def test_realistic_post_result_packet_clears_strict_gates(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        self.assertTrue(validated["capture_readiness"]["ready_for_template_update"])
        self.assertEqual(validated["capture_readiness"]["blockers"], [])
        self.assertEqual(validated["capture_status"]["next_action"], "apply_to_template")
        self.assertEqual(validated["capture_status"]["missing_ids"], [])
        self.assertEqual(validated["capture_status"]["captured_ids"], list(packet.REQUIRED_EVIDENCE_IDS))

    def test_official_result_claim_accepts_final_whistle_marker(self):
        blockers = packet.claim_keyword_blockers(
            "official_result",
            "FIFA official final score Germany 3-0 Curacao and final whistle marker captured.",
        )

        self.assertEqual(blockers, [])

    def test_official_result_claim_requires_score_value(self):
        blockers = packet.claim_keyword_blockers(
            "official_result",
            "Official final score and final whistle marker captured for Germany vs Curacao.",
        )

        self.assertIn("official_result:claim_missing_score_value", blockers)

    def test_official_result_claim_requires_source_and_teams(self):
        blockers = packet.claim_keyword_blockers(
            "official_result",
            "Final score 3-0 and final whistle marker captured.",
        )

        self.assertIn("official_result:claim_missing_official_source", blockers)
        self.assertIn("official_result:claim_missing_germany", blockers)
        self.assertIn("official_result:claim_missing_curacao", blockers)

    def test_lineup_claim_requires_teams_confirmation_and_post_result_timing(self):
        blockers = packet.claim_keyword_blockers(
            "confirmed_lineups",
            "Confirmed lineups and starting XI captured.",
        )

        self.assertIn("confirmed_lineups:claim_missing_germany", blockers)
        self.assertIn("confirmed_lineups:claim_missing_curacao", blockers)
        self.assertIn("confirmed_lineups:claim_missing_post_result", blockers)

        accepted = packet.claim_keyword_blockers(
            "confirmed_lineups",
            "FIFA confirmed lineups and starting XI for Germany and Curacao were captured after full time.",
        )

        self.assertEqual(accepted, [])

    def test_injury_claim_requires_post_match_germany_path_and_source(self):
        blockers = packet.claim_keyword_blockers(
            "injuries_suspensions",
            "Injuries, suspensions, bookings, and absences captured.",
        )

        self.assertIn("injuries_suspensions:claim_missing_post_result", blockers)
        self.assertIn("injuries_suspensions:claim_missing_germany", blockers)
        self.assertIn("injuries_suspensions:claim_missing_path_impact", blockers)
        self.assertIn("injuries_suspensions:claim_missing_source_or_checked", blockers)

        accepted = packet.claim_keyword_blockers(
            "injuries_suspensions",
            "Post-match source checked injuries, suspensions, bookings, and absences affecting Germany path impact.",
        )

        self.assertEqual(accepted, [])

    def test_odds_move_claim_requires_post_result_movement_and_baseline(self):
        blockers = packet.claim_keyword_blockers(
            "odds_move",
            "Germany odds captured.",
        )

        self.assertIn("odds_move:claim_missing_post_result", blockers)
        self.assertIn("odds_move:claim_missing_movement", blockers)
        self.assertIn("odds_move:claim_missing_baseline", blockers)

        accepted = packet.claim_keyword_blockers(
            "odds_move",
            "Post-result Germany odds movement versus the pre-result baseline with post_result_value updated odds Germany 1.04 captured.",
        )

        self.assertEqual(accepted, [])

    def test_ratings_move_claim_requires_post_result_movement_and_baseline(self):
        blockers = packet.claim_keyword_blockers(
            "ratings_move",
            "Germany rating captured.",
        )

        self.assertIn("ratings_move:claim_missing_post_result", blockers)
        self.assertIn("ratings_move:claim_missing_movement", blockers)
        self.assertIn("ratings_move:claim_missing_baseline", blockers)

        accepted = packet.claim_keyword_blockers(
            "ratings_move",
            "Post-result ratings/model movement for Germany versus baseline with post_result_value updated model Germany 86.2 captured.",
        )

        self.assertEqual(accepted, [])

    def test_odds_move_capture_requires_stored_baseline_value_when_available(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "odds_move":
                    item["claim"] = "Post-result Germany odds movement versus the pre-result baseline captured."
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "odds_move:claim_missing_baseline_odds_value",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["odds_move"])

    def test_odds_move_capture_requires_post_result_value_or_delta(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "odds_move":
                    item["claim"] = "Post-result Germany odds movement versus the pre-result baseline Germany 1.06 captured."
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "odds_move:claim_missing_post_result_value",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["odds_move"])

    def test_ratings_move_capture_requires_stored_baseline_value_when_available(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "ratings_move":
                    item["claim"] = "Post-result ratings model movement for Germany versus baseline captured."
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "ratings_move:claim_missing_baseline_rating_value",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["ratings_move"])

    def test_ratings_move_capture_requires_post_result_value_or_delta(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "ratings_move":
                    item["claim"] = "Post-result ratings model movement for Germany versus baseline ranks Germany 10 and Curacao 82 captured."
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "ratings_move:claim_missing_post_result_value",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["ratings_move"])

    def test_quote_scout_capture_requires_artifact_time_and_runner_pass_count(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "quote_scout":
                    item["claim"] = "Fresh active-portfolio quote scout EV captured after result/state shift."
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "quote_scout:claim_missing_artifact",
            validated["capture_readiness"]["blockers"],
        )
        self.assertIn(
            "quote_scout:claim_missing_generated_at",
            validated["capture_readiness"]["blockers"],
        )
        self.assertIn(
            "quote_scout:claim_missing_runner_pass_rows",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["quote_scout"])

    def test_market_state_capture_requires_values_command_and_time(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "market_state":
                    item["claim"] = "Fresh post-result Deadeye market state distribution with mu and sigma captured."
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "market_state:claim_missing_artifact_or_command",
            validated["capture_readiness"]["blockers"],
        )
        self.assertIn(
            "market_state:claim_missing_generated_at",
            validated["capture_readiness"]["blockers"],
        )
        self.assertIn(
            "market_state:claim_missing_mu_value",
            validated["capture_readiness"]["blockers"],
        )
        self.assertIn(
            "market_state:claim_missing_sigma_value",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["market_state"])

    def test_capture_plan_includes_baseline_values_for_move_rows(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            result = packet.build_packet(template_path, now="2026-06-13T06:42:00Z")

        rows = {row["id"]: row for row in result["capture_plan"]["rows"]}
        self.assertIn("Germany 1.06", rows["odds_move"]["claim_template"])
        self.assertIn("Curacao 60.0", rows["odds_move"]["claim_template"])
        self.assertIn("Germany 10", rows["ratings_move"]["claim_template"])
        self.assertIn("Curacao 82", rows["ratings_move"]["claim_template"])
        self.assertIn("baseline_value", [
            marker["label"] for marker in rows["odds_move"]["claim_must_include"]
        ])
        self.assertIn("baseline_value", [
            marker["label"] for marker in rows["ratings_move"]["claim_must_include"]
        ])
        self.assertIn("post_result_numeric_value", [
            marker["label"] for marker in rows["odds_move"]["claim_must_include"]
        ])
        self.assertIn("post_result_numeric_value", [
            marker["label"] for marker in rows["ratings_move"]["claim_must_include"]
        ])

    def test_validate_packet_recomputes_stale_result_window_flag(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T20:05:00Z")
            self.assertFalse(result["result_window_open"])
            fill_packet_evidence(result)
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:06:00Z")

        self.assertTrue(validated["result_window_open"])
        self.assertTrue(validated["capture_readiness"]["ready_for_template_update"])
        self.assertEqual(validated["capture_status"]["next_action"], "apply_to_template")
        self.assertEqual(validated["capture_status"]["missing_ids"], [])

    def test_validate_packet_blocks_capture_times_before_result_window(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T20:05:00Z")
            fill_packet_evidence_before_window(result)
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:06:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "official_result:capture_utc_before_result_window",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["next_action"], "fill_required_evidence")
        self.assertEqual(validated["capture_status"]["missing_ids"], list(packet.REQUIRED_EVIDENCE_IDS))

    def test_validate_packet_blocks_wrong_source_role_for_required_row(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_packet_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "odds_move":
                    item["source_role"] = "team_news"
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:06:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "odds_move:source_role_not_odds_snapshot",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["odds_move"])

    def test_validate_packet_blocks_non_http_url_for_public_evidence(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_packet_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "official_result":
                    item["url"] = "local-cli"
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:06:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "official_result:url_not_http",
            validated["capture_readiness"]["blockers"],
        )
        status_rows = {row["id"]: row for row in validated["capture_status"]["rows"]}
        self.assertFalse(status_rows["official_result"]["url_ready"])
        self.assertEqual(validated["capture_status"]["missing_ids"], ["official_result"])

    def test_validate_packet_allows_local_cli_for_deadeye_evidence(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_realistic_germany_result_evidence(result)
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:08:00Z")

        status_rows = {row["id"]: row for row in validated["capture_status"]["rows"]}
        self.assertTrue(validated["capture_readiness"]["ready_for_template_update"])
        self.assertTrue(status_rows["market_state"]["url_ready"])
        self.assertTrue(status_rows["quote_scout"]["url_ready"])

    def test_capture_row_updates_one_row_but_keeps_packet_unready(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            captured = packet.capture_evidence_row(
                packet_path,
                "official_result",
                claim="FIFA shows the match completed at full time with final score Germany 3-0 Curacao.",
                source="FIFA match centre",
                url="https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
                capture_utc="2026-06-14T20:06:00Z",
            )

        rows = {row["id"]: row for row in captured["capture_status"]["rows"]}
        item = {
            item["id"]: item
            for item in captured["evidence_placeholders"]
        }["official_result"]
        self.assertFalse(captured["capture_readiness"]["ready_for_template_update"])
        self.assertEqual(captured["capture_readiness"]["captured_ids"], ["official_result"])
        self.assertEqual(rows["official_result"]["blockers"], [])
        self.assertEqual(item["status"], "captured")
        self.assertTrue(item["post_result"])
        self.assertEqual(item["source_role"], "official_match_result")
        self.assertEqual(
            captured["capture_status"]["missing_ids"],
            [
                "confirmed_lineups",
                "injuries_suspensions",
                "odds_move",
                "ratings_move",
                "market_state",
                "quote_scout",
            ],
        )

    def test_capture_row_rejects_pre_window_capture_and_does_not_write(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            with self.assertRaisesRegex(Exception, "capture_utc_before_result_window"):
                packet.capture_evidence_row(
                    packet_path,
                    "official_result",
                    claim="FIFA shows the match completed at full time with final score Germany 3-0 Curacao.",
                    source="FIFA match centre",
                    url="https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464",
                    capture_utc="2026-06-14T19:59:00Z",
                )
            unchanged = json.loads(packet_path.read_text(encoding="utf-8"))

        row = {
            item["id"]: item
            for item in unchanged["evidence_placeholders"]
        }["official_result"]
        self.assertEqual(row["status"], "missing")
        self.assertFalse(row["post_result"])

    def test_capture_row_rejects_wrong_source_role_assertion(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            with self.assertRaisesRegex(Exception, "source_role must be odds_snapshot"):
                packet.capture_evidence_row(
                    packet_path,
                    "odds_move",
                    claim="Post-result Germany odds movement versus the pre-result baseline captured.",
                    source="Odds comparison source",
                    url="https://www.sportytrader.com/en/odds/germany-curacao-7937446/",
                    capture_utc="2026-06-14T20:07:00Z",
                    source_role="team_news",
                )

    def test_capture_row_allows_local_cli_for_market_state(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            captured = packet.capture_evidence_row(
                packet_path,
                "market_state",
                claim="Fresh post-result Deadeye market state from deadeye markets show generated_at 2026-06-14T20:06:30Z with mu=3.2291 and sigma=0.2702 captured.",
                source="deadeye markets show",
                url="local-cli",
                capture_utc="2026-06-14T20:07:00Z",
            )

        rows = {row["id"]: row for row in captured["capture_status"]["rows"]}
        self.assertTrue(rows["market_state"]["url_ready"])
        self.assertEqual(captured["capture_readiness"]["captured_ids"], ["market_state"])

    def test_validate_packet_blocks_generic_claim_for_required_row(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_packet_evidence(result)
            for item in result["evidence_placeholders"]:
                if item["id"] == "official_result":
                    item["claim"] = "Captured post-result evidence for official_result."
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            validated = packet.validate_packet_file(packet_path, now="2026-06-14T20:06:00Z")

        self.assertFalse(validated["capture_readiness"]["ready_for_template_update"])
        self.assertIn(
            "official_result:claim_missing_completion_marker",
            validated["capture_readiness"]["blockers"],
        )
        self.assertIn(
            "official_result:claim_missing_score",
            validated["capture_readiness"]["blockers"],
        )
        self.assertEqual(validated["capture_status"]["missing_ids"], ["official_result"])

    def test_main_writes_packet_file(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            output_path = root / "packet.json"
            write_germany_template(template_path)

            rc = packet.main([
                "--template",
                str(template_path),
                "--output",
                str(output_path),
                "--now",
                "2026-06-14T20:05:00Z",
            ])

            payload = json.loads(output_path.read_text(encoding="utf-8"))

        self.assertEqual(rc, 0)
        self.assertEqual(payload["template"]["id"], "germany-post-result-snap-template-20260612")

    def test_main_captures_row_and_writes_packet_file(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            rc = packet.main([
                "--packet",
                str(packet_path),
                "--capture-row",
                "quote_scout",
                "--claim",
                "Fresh active-portfolio quote scout EV and expected value output gap-analysis-active-portfolio-ladder-quote-4000-20260614T200700Z.json generated_at 2026-06-14T20:07:00Z with runner_pass_rows 0 captured after result/state shift.",
                "--source",
                "storm_gap_analyzer",
                "--url",
                "local-cli",
                "--capture-utc",
                "2026-06-14T20:07:00Z",
                "--output",
                str(packet_path),
            ])
            payload = json.loads(packet_path.read_text(encoding="utf-8"))

        self.assertEqual(rc, 0)
        self.assertEqual(payload["capture_readiness"]["captured_ids"], ["quote_scout"])
        rows = {row["id"]: row for row in payload["capture_status"]["rows"]}
        self.assertTrue(rows["quote_scout"]["captured"])
        self.assertFalse(payload["capture_readiness"]["ready_for_template_update"])

    def test_main_validates_filled_packet_file(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            output_path = root / "validated.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T20:05:00Z")
            fill_packet_evidence(result)
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            rc = packet.main([
                "--validate-packet",
                str(packet_path),
                "--output",
                str(output_path),
                "--now",
                "2026-06-14T20:06:00Z",
            ])
            payload = json.loads(output_path.read_text(encoding="utf-8"))

        self.assertEqual(rc, 0)
        self.assertTrue(payload["capture_readiness"]["ready_for_template_update"])

    def test_main_prints_capture_plan_from_packet_file(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            output = io.StringIO()
            with redirect_stdout(output):
                rc = packet.main([
                    "--packet",
                    str(packet_path),
                    "--print-capture-plan",
                    "--now",
                    "2026-06-13T20:06:00Z",
                ])

        text = output.getvalue()
        self.assertEqual(rc, 0)
        self.assertIn("Germany post-result capture plan", text)
        self.assertIn("--capture-row official_result", text)
        self.assertIn("post_result_numeric_value", text)
        self.assertNotIn('"packet_status"', text)

    def test_apply_packet_refuses_unready_packet(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            with self.assertRaisesRegex(Exception, "packet is not ready"):
                packet.apply_packet_to_template(
                    packet_path,
                    template_path=template_path,
                    now="2026-06-13T20:06:00Z",
                )

            template = json.loads(template_path.read_text(encoding="utf-8"))

        self.assertFalse(template["world_cup_post_result"])
        self.assertTrue(any(item.get("url") == "TO_FILL" for item in template["evidence"]))

    def test_apply_packet_updates_template_evidence_but_keeps_disabled(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_packet_evidence(result)
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            summary = packet.apply_packet_to_template(
                packet_path,
                template_path=template_path,
                now="2026-06-14T20:06:00Z",
            )
            template = json.loads(template_path.read_text(encoding="utf-8"))

        self.assertTrue(summary["applied"])
        self.assertFalse(summary["queue_allowed"])
        self.assertTrue(template["world_cup_post_result"])
        self.assertTrue(template["disabled"])
        self.assertEqual(template["template_status"], "draft_only_not_queue_active")
        self.assertEqual(template["post_result_evidence_status"], "captured_not_queue_approved")
        self.assertFalse(any(item.get("url") == "TO_FILL" for item in template["evidence"]))
        packet_ids = {item.get("evidence_packet_id") for item in template["evidence"]}
        self.assertTrue(set(packet.REQUIRED_EVIDENCE_IDS).issubset(packet_ids))
        self.assertTrue(any(item.get("source_role") == "official_fixture" for item in template["evidence"]))
        applied_by_id = {
            item.get("evidence_packet_id"): item
            for item in template["evidence"]
            if item.get("evidence_packet_id")
        }
        self.assertIn(
            "https://inside.fifa.com/fifa-world-ranking/GER?gender=men",
            applied_by_id["ratings_move"]["source_options"],
        )

    def test_main_applies_filled_packet_to_template(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            output_path = root / "apply-summary.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_packet_evidence(result)
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            rc = packet.main([
                "--apply-to-template",
                str(packet_path),
                "--template",
                str(template_path),
                "--output",
                str(output_path),
                "--now",
                "2026-06-14T20:06:00Z",
            ])
            summary = json.loads(output_path.read_text(encoding="utf-8"))

        self.assertEqual(rc, 0)
        self.assertTrue(summary["applied"])
        self.assertTrue(summary["disabled"])

    def test_main_apply_unready_packet_returns_error(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            template_path = root / "germany.json"
            packet_path = root / "packet.json"
            write_germany_template(template_path)
            result = packet.build_packet(template_path, now="2026-06-13T20:05:00Z")
            packet_path.write_text(json.dumps(result), encoding="utf-8")

            output = io.StringIO()
            with redirect_stdout(output):
                rc = packet.main([
                    "--apply-to-template",
                    str(packet_path),
                    "--template",
                    str(template_path),
                    "--now",
                    "2026-06-13T20:06:00Z",
                ])
            payload = json.loads(output.getvalue())

        self.assertEqual(rc, 1)
        self.assertFalse(payload["ok"])
        self.assertIn("packet is not ready", payload["error"])


if __name__ == "__main__":
    unittest.main()
