#!/usr/bin/env python3
import json
import sys
import tempfile
import unittest
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
    for item in result["evidence_placeholders"]:
        item["status"] = "captured"
        item["post_result"] = True
        item["capture_utc"] = "2026-06-14T20:04:00Z"
        item["claim"] = f"Captured post-result evidence for {item['id']}."
        if item["url"] == "TO_FILL":
            item["url"] = "https://example.com/evidence"


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

    def test_filled_packet_capture_readiness_passes_after_window(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            template_path = Path(tmpdir) / "germany.json"
            write_germany_template(template_path)

            result = packet.build_packet(template_path, now="2026-06-14T20:05:00Z")
            fill_packet_evidence(result)
            readiness = packet.capture_readiness(result, now="2026-06-14T20:06:00Z")

        self.assertTrue(readiness["ready_for_template_update"])
        self.assertEqual(readiness["blockers"], [])
        self.assertEqual(readiness["captured_ids"], list(packet.REQUIRED_EVIDENCE_IDS))

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


if __name__ == "__main__":
    unittest.main()
