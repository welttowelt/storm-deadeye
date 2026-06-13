#!/usr/bin/env python3
"""Build a fillable post-result evidence packet from a World Cup template.

This is local evidence plumbing only. It never calls Deadeye, never reads wallet
config, and never submits transactions. The output is meant to speed up the
post-result evidence capture before a template can be promoted.
"""

from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import storm_deadeye_loop as loop


DEFAULT_GERMANY_TEMPLATE = loop.DEFAULT_TEMPLATES / "germany-post-result-snap-template-20260612.json"
REQUIRED_EVIDENCE_IDS = (
    "official_result",
    "confirmed_lineups",
    "injuries_suspensions",
    "odds_move",
    "ratings_move",
    "market_state",
    "quote_scout",
)
CAPTURED_STATUSES = {"captured", "complete", "filled"}
PLACEHOLDER_VALUES = {"", "TO_FILL", "<MARKET>"}


def utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def source_urls(template: dict[str, Any]) -> dict[str, list[str]]:
    baseline = template.get("pre_result_baseline") or {}
    official = baseline.get("official_fixture_url") or ""
    if not official:
        for item in template.get("evidence") or []:
            if isinstance(item, dict) and item.get("source_role") == "official_fixture":
                official = str(item.get("url") or "")
                break
    return {
        "official": [official] if official else [],
        "team_news": list(baseline.get("team_news_urls") or []),
        "ratings": list(baseline.get("ratings_context_urls") or []),
        "odds": list(baseline.get("odds_context_urls") or []),
    }


def template_status(template_path: Path, template: dict[str, Any]) -> dict[str, Any]:
    for item in loop.load_template_status(template_path.parent):
        if item.get("file") == template_path.name or item.get("id") == template.get("id"):
            return item
    blockers = loop.template_blockers(template)
    return {
        "file": template_path.name,
        "id": template.get("id"),
        "queue_ready": not blockers,
        "blockers": blockers,
    }


def evidence_placeholders(template: dict[str, Any]) -> list[dict[str, Any]]:
    urls = source_urls(template)
    official_url = (urls["official"] or ["TO_FILL"])[0]
    team_news_url = (urls["team_news"] or [official_url])[0]
    odds_url = (urls["odds"] or ["TO_FILL"])[0]
    ratings_url = (urls["ratings"] or ["TO_FILL"])[0]
    market = template.get("market") or "<market>"
    return [
        {
            "id": "official_result",
            "required": True,
            "status": "missing",
            "source_role": "official_match_result",
            "source": "FIFA match centre",
            "url": official_url,
            "post_result": False,
            "claim": "TO_FILL: official completed-match marker, final score, and capture UTC.",
            "capture_utc": "TO_FILL",
        },
        {
            "id": "confirmed_lineups",
            "required": True,
            "status": "missing",
            "source_role": "official_lineups",
            "source": "FIFA match centre or confirmed lineup source",
            "url": official_url,
            "post_result": False,
            "claim": "TO_FILL: confirmed starting XIs, substitutes, and late absences.",
            "capture_utc": "TO_FILL",
        },
        {
            "id": "injuries_suspensions",
            "required": True,
            "status": "missing",
            "source_role": "team_news",
            "source": "Team news / match report source",
            "url": team_news_url,
            "post_result": False,
            "claim": "TO_FILL: in-match injuries, late absences, bookings, and suspension/path impact.",
            "capture_utc": "TO_FILL",
        },
        {
            "id": "odds_move",
            "required": True,
            "status": "missing",
            "source_role": "odds_snapshot",
            "source": "Odds comparison source",
            "url": odds_url,
            "post_result": False,
            "claim": "TO_FILL: post-result Germany match/path/group odds movement versus baseline.",
            "capture_utc": "TO_FILL",
        },
        {
            "id": "ratings_move",
            "required": True,
            "status": "missing",
            "source_role": "ratings_snapshot",
            "source": "Ratings/model source",
            "url": ratings_url,
            "post_result": False,
            "claim": "TO_FILL: post-result ratings/model movement versus baseline.",
            "capture_utc": "TO_FILL",
        },
        {
            "id": "market_state",
            "required": True,
            "status": "missing",
            "source_role": "deadeye_market_state",
            "source": "deadeye markets show",
            "url": "local-cli",
            "post_result": False,
            "claim": f"TO_FILL: fresh post-result Deadeye market state for {market}.",
            "capture_utc": "TO_FILL",
        },
        {
            "id": "quote_scout",
            "required": True,
            "status": "missing",
            "source_role": "deadeye_quote_scout",
            "source": "storm_gap_analyzer",
            "url": "local-cli",
            "post_result": False,
            "claim": "TO_FILL: fresh active-portfolio quote scout after result/state shift.",
            "capture_utc": "TO_FILL",
        },
    ]


def is_placeholder(value: Any) -> bool:
    return str(value or "").strip().upper() in PLACEHOLDER_VALUES


def valid_capture_utc(value: Any) -> bool:
    if is_placeholder(value):
        return False
    try:
        loop.parse_utc_timestamp(value)
    except (TypeError, ValueError):
        return False
    return True


def result_window_open_from_packet(packet: dict[str, Any], *, now: str | None = None) -> bool | None:
    template = packet.get("template") or {}
    raw_window = template.get("result_not_before_utc")
    if not raw_window:
        return None
    try:
        return loop.parse_utc_timestamp(raw_window) <= loop.parse_utc_timestamp(now or utc_now())
    except (TypeError, ValueError):
        return None


def evidence_item_blockers(item: dict[str, Any]) -> list[str]:
    item_id = str(item.get("id") or "unknown")
    blockers: list[str] = []
    if str(item.get("status") or "").lower() not in CAPTURED_STATUSES:
        blockers.append(f"{item_id}:status_not_captured")
    if item.get("post_result") is not True:
        blockers.append(f"{item_id}:post_result_not_true")
    if is_placeholder(item.get("claim")) or str(item.get("claim") or "").strip().upper().startswith("TO_FILL"):
        blockers.append(f"{item_id}:claim_placeholder")
    if is_placeholder(item.get("url")):
        blockers.append(f"{item_id}:url_placeholder")
    if not valid_capture_utc(item.get("capture_utc")):
        blockers.append(f"{item_id}:capture_utc_invalid")
    if item_id == "official_result" and str(item.get("source_role") or "") != "official_match_result":
        blockers.append("official_result:source_role_not_official_match_result")
    return blockers


def capture_readiness(packet: dict[str, Any], *, now: str | None = None) -> dict[str, Any]:
    validated_at = now or utc_now()
    blockers: list[str] = []
    if not packet.get("result_window_open"):
        blockers.append("result_window_not_open")
    by_id: dict[str, dict[str, Any]] = {}
    duplicate_ids: set[str] = set()
    for item in packet.get("evidence_placeholders") or []:
        if not isinstance(item, dict):
            blockers.append("evidence_item_not_object")
            continue
        item_id = str(item.get("id") or "")
        if not item_id:
            blockers.append("evidence_item_missing_id")
            continue
        if item_id in by_id:
            duplicate_ids.add(item_id)
        by_id[item_id] = item
    for item_id in sorted(duplicate_ids):
        blockers.append(f"{item_id}:duplicate_id")
    captured_ids: list[str] = []
    item_blockers: dict[str, list[str]] = {}
    for item_id in REQUIRED_EVIDENCE_IDS:
        item = by_id.get(item_id)
        if not item:
            blockers.append(f"{item_id}:missing")
            continue
        current_blockers = evidence_item_blockers(item)
        if current_blockers:
            item_blockers[item_id] = current_blockers
            blockers.extend(current_blockers)
            continue
        captured_ids.append(item_id)
        try:
            if loop.parse_utc_timestamp(item.get("capture_utc")) > loop.parse_utc_timestamp(validated_at):
                blocker = f"{item_id}:capture_utc_after_packet_time"
                item_blockers.setdefault(item_id, []).append(blocker)
                blockers.append(blocker)
                captured_ids.remove(item_id)
        except (TypeError, ValueError):
            pass
    return {
        "validated_at": validated_at,
        "required_ids": list(REQUIRED_EVIDENCE_IDS),
        "captured_ids": captured_ids,
        "ready_for_template_update": not blockers,
        "blockers": sorted(set(blockers)),
        "item_blockers": item_blockers,
    }


def read_only_commands(template: dict[str, Any]) -> list[str]:
    market = template.get("market") or "<market>"
    return [
        f"deadeye markets show {market} --output json",
        f"deadeye doctor --market {market} --output plain",
        "python3 scripts/storm_gap_analyzer.py --preset active-portfolio-20260612 --budget 4000 --budget-ladder --quote-only --sort-by ev",
        "python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox --refresh-active-portfolio-scout --active-portfolio-scout-max-age-minutes 0",
        "python3 scripts/storm_worldcup_evidence_packet.py --validate-packet ~/.local/state/storm-deadeye/germany-post-result-evidence-packet.json",
    ]


def build_packet(template_path: Path, *, now: str | None = None) -> dict[str, Any]:
    template = loop.load_json(template_path, {})
    if not isinstance(template, dict):
        raise loop.LoopError(f"{template_path} did not contain a JSON object")
    generated_at = now or utc_now()
    status = template_status(template_path, template)
    window_open = False
    raw_window = template.get("result_not_before_utc")
    if raw_window:
        try:
            window_open = loop.parse_utc_timestamp(raw_window) <= loop.parse_utc_timestamp(generated_at)
        except (TypeError, ValueError):
            window_open = False
    blockers = status.get("blockers") or []
    packet = {
        "generated_at": generated_at,
        "packet_status": "draft_post_result_evidence_packet",
        "template": {
            "path": str(template_path),
            "id": template.get("id"),
            "disabled": bool(template.get("disabled", False)),
            "template_status": template.get("template_status"),
            "market": template.get("market"),
            "family": template.get("family"),
            "direction": (template.get("prepared_from") or {}).get("label"),
            "result_not_before_utc": template.get("result_not_before_utc"),
            "world_cup_post_result": bool(template.get("world_cup_post_result", False)),
        },
        "queue_allowed": False,
        "queue_ready_now": bool(status.get("queue_ready")),
        "queue_blockers": blockers,
        "result_window_open": window_open,
        "pre_result_baseline_captured_at": template.get("pre_result_baseline_captured_at"),
        "pre_result_baseline": template.get("pre_result_baseline") or {},
        "post_result_capture_required": template.get("post_result_capture_required") or [],
        "source_urls": source_urls(template),
        "evidence_placeholders": evidence_placeholders(template),
        "read_only_commands_after_result": read_only_commands(template),
        "template_update_requirements_after_capture": [
            "copy captured evidence rows into the source template",
            "set world_cup_post_result true only after official_result passes validation",
            "remove evidence_url_to_fill placeholders from template evidence",
            "keep template disabled until fresh quote, dry-run, concentration, gas, XP, and trade-cap gates pass",
            "promote only through storm_template_promoter and the Storm Deadeye runner",
        ],
        "non_goals": [
            "does not approve pre-result queueing",
            "does not approve dry-run or execution",
            "does not bypass fresh smoke, doctor, quote, gas, XP, concentration, or trade caps",
        ],
    }
    packet["capture_readiness"] = capture_readiness(packet, now=generated_at)
    return packet


def validate_packet_file(packet_path: Path, *, now: str | None = None) -> dict[str, Any]:
    packet = loop.load_json(packet_path, {})
    if not isinstance(packet, dict):
        raise loop.LoopError(f"{packet_path} did not contain a JSON object")
    window_open = result_window_open_from_packet(packet, now=now)
    if window_open is not None:
        packet["result_window_open"] = window_open
    packet["capture_readiness"] = capture_readiness(packet, now=now)
    return packet


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Build a World Cup post-result evidence packet.")
    parser.add_argument("--template", type=Path, default=DEFAULT_GERMANY_TEMPLATE)
    parser.add_argument("--output", type=Path, help="Write the packet to this JSON file instead of stdout.")
    parser.add_argument("--validate-packet", type=Path, help="Validate an already filled packet instead of building a new one.")
    parser.add_argument("--now", help="UTC timestamp override for tests/backfills, e.g. 2026-06-14T20:05:00Z.")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    if args.validate_packet:
        packet = validate_packet_file(args.validate_packet, now=args.now)
    else:
        packet = build_packet(args.template, now=args.now)
    text = json.dumps(packet, indent=2, sort_keys=True)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text + "\n", encoding="utf-8")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
