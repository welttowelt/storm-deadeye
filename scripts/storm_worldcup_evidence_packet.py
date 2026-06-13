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
    result_window = None
    raw_window = (packet.get("template") or {}).get("result_not_before_utc")
    if raw_window:
        try:
            result_window = loop.parse_utc_timestamp(raw_window)
        except (TypeError, ValueError):
            result_window = None
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
            capture_utc = loop.parse_utc_timestamp(item.get("capture_utc"))
            if result_window and capture_utc < result_window:
                blocker = f"{item_id}:capture_utc_before_result_window"
                item_blockers.setdefault(item_id, []).append(blocker)
                blockers.append(blocker)
                captured_ids.remove(item_id)
                continue
            if capture_utc > loop.parse_utc_timestamp(validated_at):
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


def evidence_capture_status(packet: dict[str, Any]) -> dict[str, Any]:
    readiness = packet.get("capture_readiness") or {}
    item_blockers = readiness.get("item_blockers") or {}
    rows: list[dict[str, Any]] = []
    by_id = {
        str(item.get("id")): item
        for item in packet.get("evidence_placeholders") or []
        if isinstance(item, dict)
    }
    captured_ids = set(readiness.get("captured_ids") or [])
    missing_ids: list[str] = []
    for item_id in REQUIRED_EVIDENCE_IDS:
        item = by_id.get(item_id)
        blockers = list(item_blockers.get(item_id) or [])
        claim = (item or {}).get("claim")
        if item_id not in captured_ids:
            missing_ids.append(item_id)
        rows.append({
            "id": item_id,
            "captured": item_id in captured_ids,
            "status": (item or {}).get("status"),
            "source_role": (item or {}).get("source_role"),
            "source": (item or {}).get("source"),
            "url": (item or {}).get("url"),
            "post_result": (item or {}).get("post_result"),
            "claim_ready": bool(item)
            and not is_placeholder(claim)
            and not str(claim or "").strip().upper().startswith("TO_FILL"),
            "url_ready": bool(item) and not is_placeholder((item or {}).get("url")),
            "capture_utc_ready": bool(item) and valid_capture_utc((item or {}).get("capture_utc")),
            "blockers": blockers or ([] if item_id in captured_ids else [f"{item_id}:missing"]),
        })

    if readiness.get("ready_for_template_update"):
        next_action = "apply_to_template"
    elif not packet.get("result_window_open"):
        next_action = "wait_for_result_window"
    elif missing_ids:
        next_action = "fill_required_evidence"
    else:
        next_action = "resolve_capture_blockers"

    return {
        "ready_for_template_update": bool(readiness.get("ready_for_template_update")),
        "result_window_open": bool(packet.get("result_window_open")),
        "captured_ids": list(readiness.get("captured_ids") or []),
        "missing_ids": missing_ids,
        "blocker_count": len(readiness.get("blockers") or []),
        "next_action": next_action,
        "rows": rows,
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
    packet["capture_status"] = evidence_capture_status(packet)
    return packet


def validate_packet_file(packet_path: Path, *, now: str | None = None) -> dict[str, Any]:
    packet = loop.load_json(packet_path, {})
    if not isinstance(packet, dict):
        raise loop.LoopError(f"{packet_path} did not contain a JSON object")
    window_open = result_window_open_from_packet(packet, now=now)
    if window_open is not None:
        packet["result_window_open"] = window_open
    packet["capture_readiness"] = capture_readiness(packet, now=now)
    packet["capture_status"] = evidence_capture_status(packet)
    return packet


def packet_template_path(packet: dict[str, Any], override: Path | None = None) -> Path:
    if override is not None:
        return override
    raw_path = (packet.get("template") or {}).get("path")
    if not raw_path:
        raise loop.LoopError("packet does not include template.path; pass --template")
    return Path(raw_path)


def captured_evidence_for_template(packet: dict[str, Any]) -> list[dict[str, Any]]:
    by_id = {
        str(item.get("id")): item
        for item in packet.get("evidence_placeholders") or []
        if isinstance(item, dict)
    }
    captured: list[dict[str, Any]] = []
    for item_id in REQUIRED_EVIDENCE_IDS:
        item = by_id[item_id]
        captured.append({
            "claim": item.get("claim"),
            "source_role": item.get("source_role"),
            "source": item.get("source"),
            "url": item.get("url"),
            "post_result": True,
            "capture_utc": item.get("capture_utc"),
            "evidence_packet_id": item_id,
        })
    return captured


def template_evidence_without_placeholders(template: dict[str, Any]) -> list[dict[str, Any]]:
    kept: list[dict[str, Any]] = []
    for item in template.get("evidence") or []:
        if not isinstance(item, dict):
            continue
        if str(item.get("url") or "").strip().upper() == "TO_FILL":
            continue
        if str(item.get("claim") or "").strip().upper().startswith("TO_FILL"):
            continue
        if str(item.get("evidence_packet_id") or "") in REQUIRED_EVIDENCE_IDS:
            continue
        kept.append(item)
    return kept


def apply_packet_to_template(
    packet_path: Path,
    *,
    template_path: Path | None = None,
    now: str | None = None,
) -> dict[str, Any]:
    packet = validate_packet_file(packet_path, now=now)
    readiness = packet.get("capture_readiness") or {}
    if not readiness.get("ready_for_template_update"):
        blockers = readiness.get("blockers") or []
        raise loop.LoopError("packet is not ready for template update: " + ", ".join(blockers))
    target = packet_template_path(packet, template_path)
    template = loop.load_json(target, {})
    if not isinstance(template, dict):
        raise loop.LoopError(f"{target} did not contain a JSON object")
    packet_template = packet.get("template") or {}
    packet_id = packet_template.get("id")
    packet_market = packet_template.get("market")
    if packet_id and template.get("id") and packet_id != template.get("id"):
        raise loop.LoopError(f"packet template id {packet_id} does not match {template.get('id')}")
    if packet_market and template.get("market") and loop.canonical_address(packet_market) != loop.canonical_address(template.get("market")):
        raise loop.LoopError("packet market does not match template market")

    captured = captured_evidence_for_template(packet)
    template["evidence"] = template_evidence_without_placeholders(template) + captured
    template["world_cup_post_result"] = True
    template["disabled"] = True
    template["post_result_evidence_status"] = "captured_not_queue_approved"
    template["post_result_evidence_applied_at"] = now or utc_now()
    template["post_result_evidence_packet"] = {
        "path": str(packet_path),
        "generated_at": packet.get("generated_at"),
        "validated_at": readiness.get("validated_at"),
        "required_ids": readiness.get("required_ids") or list(REQUIRED_EVIDENCE_IDS),
        "captured_ids": readiness.get("captured_ids") or [],
    }
    loop.save_json(target, template)
    return {
        "applied": True,
        "template_path": str(target),
        "template_id": template.get("id"),
        "market": template.get("market"),
        "world_cup_post_result": template.get("world_cup_post_result"),
        "disabled": template.get("disabled"),
        "template_status": template.get("template_status"),
        "evidence_rows": len(template.get("evidence") or []),
        "captured_ids": readiness.get("captured_ids") or [],
        "queue_allowed": False,
        "next_required_gate": "fresh quote, dry-run, concentration, gas, XP, and trade-cap gates",
    }


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Build a World Cup post-result evidence packet.")
    parser.add_argument("--template", type=Path, help="Template path. Defaults to the Germany template when building.")
    parser.add_argument("--output", type=Path, help="Write the packet to this JSON file instead of stdout.")
    parser.add_argument("--validate-packet", type=Path, help="Validate an already filled packet instead of building a new one.")
    parser.add_argument("--apply-to-template", type=Path, help="Validate a filled packet and copy its captured evidence into the template.")
    parser.add_argument("--now", help="UTC timestamp override for tests/backfills, e.g. 2026-06-14T20:05:00Z.")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    try:
        if args.apply_to_template:
            packet = apply_packet_to_template(args.apply_to_template, template_path=args.template, now=args.now)
        elif args.validate_packet:
            packet = validate_packet_file(args.validate_packet, now=args.now)
        else:
            packet = build_packet(args.template or DEFAULT_GERMANY_TEMPLATE, now=args.now)
    except loop.LoopError as exc:
        print(json.dumps({"ok": False, "error": str(exc)}, indent=2, sort_keys=True))
        return 1
    text = json.dumps(packet, indent=2, sort_keys=True)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text + "\n", encoding="utf-8")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
