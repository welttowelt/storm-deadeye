#!/usr/bin/env python3
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import storm_template_promoter as promoter


def write_template(path: Path, **overrides):
    template = {
        "id": "france-ready",
        "disabled": False,
        "template_status": "ready_to_queue",
        "prepared_from": {
            "label": "France lower/wider",
            "status": "durable_watch",
            "quote_expected_value_xp": 14.3,
            "belief_gap_improvement_xp": 375.5,
        },
        "market": "0x5e678",
        "family": "lognormal",
        "belief": 3.3346,
        "belief_sigma": 0.2787,
        "budget": 100,
        "min_expected_value": 10,
        "world_cup_post_result": True,
        "rationale": "Official result evidence supports the prepared World Cup snap candidate.",
        "evidence": [
            {
                "claim": "FIFA published the official final score.",
                "source": "FIFA match centre",
                "source_role": "official_match_result",
                "url": "https://www.fifa.com/en/match-centre/match/example",
                "post_result": True,
            },
            {"claim": "Market-prior source remains current.", "source": "Market source"},
        ],
    }
    template.update(overrides)
    path.write_text(json.dumps(template), encoding="utf-8")


class StormTemplatePromoterTests(unittest.TestCase):
    def test_blocked_template_is_not_promoted(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            templates = Path(tmpdir) / "templates"
            templates.mkdir()
            candidates = Path(tmpdir) / "candidates.jsonl"
            write_template(
                templates / "france.json",
                disabled=True,
                template_status="draft_only_not_queue_active",
                world_cup_post_result=False,
                evidence=[
                    {
                        "claim": "TO_FILL: official final score.",
                        "source": "FIFA",
                        "source_role": "official_match_result",
                        "url": "TO_FILL",
                        "post_result": False,
                    }
                ],
            )

            result = promoter.promote_templates(templates, candidates, append=True)

        self.assertEqual(result["promoted"], [])
        self.assertEqual(len(result["skipped"]), 1)
        self.assertIn("template_not_queue_active", result["skipped"][0]["blockers"])
        self.assertIn("missing_official_result_evidence", result["skipped"][0]["blockers"])

    def test_ready_template_appends_once(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            templates = Path(tmpdir) / "templates"
            templates.mkdir()
            candidates = Path(tmpdir) / "candidates.jsonl"
            write_template(templates / "france.json")

            first = promoter.promote_templates(templates, candidates, append=True)
            second = promoter.promote_templates(templates, candidates, append=True)
            rows = [json.loads(line) for line in candidates.read_text(encoding="utf-8").splitlines()]

        self.assertEqual(first["promoted"][0]["id"], "france-ready")
        self.assertEqual(second["promoted"], [])
        self.assertEqual(second["skipped"][0]["reason"], "duplicate_candidate_id")
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["id"], "france-ready")
        self.assertNotIn("disabled", rows[0])
        self.assertTrue(rows[0]["world_cup_post_result"])


if __name__ == "__main__":
    unittest.main()
