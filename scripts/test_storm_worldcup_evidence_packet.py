#!/usr/bin/env python3
import json
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

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
            "team_news_urls": ["https://bulinews.com/germany-curacao-preview-team-news-and-predicted-lineups"],
            "ratings_context_urls": ["https://www.bundesliga.com/example"],
            "odds_context_urls": ["https://www.sportytrader.com/example"],
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
        "official_result": "Official final score and completed marker captured for Germany vs Curacao.",
        "confirmed_lineups": "Confirmed lineups, starting XI, substitutes, and late absences captured.",
        "injuries_suspensions": "Injuries, suspensions, bookings, and absences checked for path impact.",
        "odds_move": "Post-result Germany odds movement versus the pre-result baseline captured.",
        "ratings_move": "Post-result ratings/model movement versus baseline captured.",
        "market_state": "Fresh post-result Deadeye market state distribution captured.",
        "quote_scout": "Fresh active-portfolio quote scout EV captured after result/state shift.",
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
