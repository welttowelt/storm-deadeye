#!/usr/bin/env python3
"""Storm Deadeye leaderboard loop.

This runner is deliberately narrow. It monitors Deadeye leaderboards and can
execute queued, evidence-backed leaderboard trades, but it never invents a
forecast by itself and never touches LP/admin/deploy/grant/settlement paths.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_STATE_DIR = Path.home() / ".local" / "state" / "storm-deadeye"
DEFAULT_SHARE_DIR = Path.home() / ".local" / "share" / "storm-deadeye"
DEFAULT_CANDIDATES = DEFAULT_STATE_DIR / "candidates.jsonl"
DEFAULT_STATE = DEFAULT_STATE_DIR / "state.json"
DEFAULT_EVENTS = DEFAULT_STATE_DIR / "events.jsonl"
DEFAULT_TRADE_JOURNAL = DEFAULT_SHARE_DIR / "trade-journal.jsonl"
DEFAULT_MAILBOX = REPO_ROOT.parent / "DEADEYE_AGENT_HANDOFF.md"
DEFAULT_SMOKE_SCRIPT = REPO_ROOT.parent / "deadeye-claude-smoke" / "smoke.sh"
DEFAULT_SMOKE_MARKET = "0x5e678bd092173e9ef0945f348d09e6c1c22f78e06c0ef380441444359193500"
DEFAULT_TEMPLATES = DEFAULT_STATE_DIR / "templates"
DEFAULT_ACTIVE_PORTFOLIO_SCOUT_MAX_AGE_MINUTES = 60.0
ACTIVE_PORTFOLIO_SCOUT_BUDGET = 4000.0

XP_RESERVE = 1000.0
XP_LADDER = [100.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0]
MAX_TRADES_PER_LOOP = 3
MAX_TRADES_PER_HOUR = 12
GAS_WARN = 100.0
GAS_STRONG_WARN = 50.0
GAS_HARD_STOP = 25.0
MIN_RATIONALE_CHARS = 40
MIN_EXECUTE_EV = 10.0
CAMPAIGN_LOSS_HALT_XP = 1500.0
MAX_LOTS_PER_MARKET = 2
MAX_LOTS_PER_SETTLEMENT = 2
RANKING_TIME_WINDOWS = (
    ("last-1h", 3600),
    ("last-24h", 86400),
    ("last-7d", 604800),
)
SMOKE_SCRIPT_ATTEMPTS = 2
SMOKE_SCRIPT_RETRY_DELAY_SECONDS = 5.0
MARKET_STATE_FINGERPRINT_VERSION = 2
WATCHED_TEMPLATE_MARKET_STATE_FINGERPRINT_VERSION = 1
MARKET_FINGERPRINT_KEYS = {
    "address",
    "backing",
    "category",
    "collateral",
    "distribution",
    "domain",
    "family",
    "feeConfig",
    "isActive",
    "isPaused",
    "liquidity",
    "marketType",
    "minTradeCollateral",
    "oracle",
    "paused",
    "resolution",
    "settled",
    "settlementValue",
    "state",
    "status",
    "title",
}
VOLATILE_MARKET_KEY_PARTS = (
    "block",
    "created",
    "fetched",
    "hash",
    "timestamp",
    "transaction",
    "tx",
    "updated",
)

SECRET_RE = re.compile(
    r"mnemonic|private[_ -]?key|recovery phrase|secret[_ -]?key|BEGIN [A-Z ]*PRIVATE",
    re.IGNORECASE,
)

OFFICIAL_ECON_HINTS = (
    "bls.gov",
    "bea.gov",
    "fred.stlouisfed.org",
    "federalreserve.gov",
    "clevelandfed.org",
    "eia.gov",
    "census.gov",
)

WORLD_CUP_POST_RESULT_ROLES = {
    "post_result",
    "match_result",
    "official_match_result",
    "official_result",
}
QUEUEABLE_TEMPLATE_OPPORTUNITIES = {"runner_candidate", "durable_watch"}
NEXT_DURABLE_WINDOW_SKIP_BLOCKERS = {
    "template_ev_below_floor",
    "template_opportunity_not_durable",
    "template_result_window_invalid",
}
CANDIDATE_TEMPLATE_FIELDS = (
    "id",
    "market",
    "family",
    "belief",
    "belief_sigma",
    "budget",
    "min_expected_value",
    "max_market_lots",
    "max_settlement_lots",
    "world_cup_post_result",
    "rationale",
    "evidence",
)


class LoopError(RuntimeError):
    """Expected operator-loop failure."""


@dataclass
class CmdResult:
    cmd: list[str]
    returncode: int
    stdout: str
    stderr: str


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def now_ts() -> int:
    return int(time.time())


def ensure_dirs(*paths: Path) -> None:
    for path in paths:
        path.mkdir(parents=True, exist_ok=True)


def canonical_address(address: str | None) -> str:
    if not address:
        return ""
    value = address.strip().lower()
    if value.startswith("0x"):
        value = value[2:]
    value = value.lstrip("0") or "0"
    return f"0x{value}"


def slugify(value: str) -> str:
    value = value.strip().lower()
    value = re.sub(r"[^a-z0-9]+", "-", value)
    value = re.sub(r"-+", "-", value).strip("-")
    return value


def parse_json_object(text: str, label: str) -> Any:
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        raise LoopError(f"{label} did not return valid JSON: {exc}") from exc


def check_no_secret(text: str, label: str) -> None:
    if SECRET_RE.search(text):
        raise LoopError(f"{label} output matched the secret-scan pattern")


def run_cmd(
    args: list[str],
    *,
    timeout: int = 60,
    check: bool = True,
    attempts: int = 1,
    retry_delay_seconds: float = 2.0,
) -> CmdResult:
    env = os.environ.copy()
    env["PATH"] = f"{Path.home() / '.local' / 'bin'}:{env.get('PATH', '')}"
    max_attempts = max(1, int(attempts))
    result = CmdResult(args, 1, "", "command did not run")
    for attempt in range(1, max_attempts + 1):
        proc = subprocess.run(
            args,
            cwd=REPO_ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        result = CmdResult(args, proc.returncode, proc.stdout, proc.stderr)
        check_no_secret(proc.stdout + "\n" + proc.stderr, "command")
        if proc.returncode == 0 or not check:
            return result
        if attempt < max_attempts:
            time.sleep(retry_delay_seconds)
    joined = " ".join(args)
    tail = (result.stderr or result.stdout).strip().splitlines()[-3:]
    raise LoopError(f"{joined} failed rc={result.returncode}: {' | '.join(tail)}")


def deadeye_json(args: list[str], *, timeout: int = 60, attempts: int = 1) -> Any:
    result = run_cmd(["deadeye", *args, "--output", "json"], timeout=timeout, attempts=attempts)
    return parse_json_object(result.stdout, "deadeye " + " ".join(args))


def http_get_json(base_url: str, path: str, *, timeout: int = 15) -> tuple[int, Any]:
    url = urllib.parse.urljoin(base_url.rstrip("/") + "/", path.lstrip("/"))
    request = urllib.request.Request(url, headers={"User-Agent": "storm-deadeye-loop/1"})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read().decode("utf-8")
            check_no_secret(body, url)
            return response.status, json.loads(body)
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        check_no_secret(body, url)
        try:
            payload: Any = json.loads(body)
        except json.JSONDecodeError:
            payload = {"body": body[:256]}
        return exc.code, payload
    except urllib.error.URLError as exc:
        return 0, {"error": str(exc)}


def load_json(path: Path, default: Any) -> Any:
    if not path.exists():
        return default
    with path.open("r", encoding="utf-8") as fh:
        return json.load(fh)


def save_json(path: Path, payload: Any) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    with tmp.open("w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=2, sort_keys=True)
        fh.write("\n")
    tmp.replace(path)


def append_jsonl(path: Path, payload: dict[str, Any]) -> None:
    with path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(payload, sort_keys=True))
        fh.write("\n")


def load_candidates(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    candidates: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as fh:
        for line_no, line in enumerate(fh, 1):
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            try:
                candidate = json.loads(stripped)
            except json.JSONDecodeError as exc:
                raise LoopError(f"candidate file line {line_no} is invalid JSON: {exc}") from exc
            if not isinstance(candidate, dict):
                raise LoopError(f"candidate file line {line_no} is not an object")
            candidates.append(candidate)
    return candidates


def evidence_has_post_result_marker(item: dict[str, Any]) -> bool:
    if str(item.get("url") or "").strip().upper() == "TO_FILL":
        return False
    if item.get("post_result") is False:
        return False
    source_role = str(item.get("source_role") or "").lower()
    event_stage = str(item.get("event_stage") or "").lower()
    return (
        item.get("post_result") is True
        or source_role in WORLD_CUP_POST_RESULT_ROLES
        or event_stage in {"post_result", "match_completed"}
    )


def template_expected_value(template: dict[str, Any]) -> float | None:
    prepared_from = template.get("prepared_from") or {}
    raw_value = template.get("quote_expected_value_xp", prepared_from.get("quote_expected_value_xp"))
    try:
        return float(raw_value)
    except (TypeError, ValueError):
        return None


def template_opportunity_status(template: dict[str, Any]) -> str:
    prepared_from = template.get("prepared_from") or {}
    return str(template.get("opportunity_status") or prepared_from.get("status") or "").lower()


def parse_utc_timestamp(raw_value: Any) -> datetime:
    raw = str(raw_value or "").strip()
    if not raw:
        raise ValueError("empty timestamp")
    value = raw[:-1] + "+00:00" if raw.endswith("Z") else raw
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def template_blockers(template: dict[str, Any]) -> list[str]:
    blockers: list[str] = []
    if template.get("disabled", False):
        blockers.append("disabled")
    status = str(template.get("template_status") or "").lower()
    if "draft" in status or "not_queue" in status:
        blockers.append("template_not_queue_active")
    if not template.get("world_cup_post_result", False):
        blockers.append("missing_world_cup_post_result")
    expected_value = template_expected_value(template)
    min_expected_value = float(template.get("min_expected_value", MIN_EXECUTE_EV))
    if expected_value is not None and expected_value < min_expected_value:
        blockers.append("template_ev_below_floor")
    opportunity_status = template_opportunity_status(template)
    if opportunity_status and opportunity_status not in QUEUEABLE_TEMPLATE_OPPORTUNITIES:
        blockers.append("template_opportunity_not_durable")
    result_not_before = template.get("result_not_before_utc") or template.get("result_not_before")
    if result_not_before:
        try:
            not_before = parse_utc_timestamp(result_not_before)
        except (TypeError, ValueError):
            blockers.append("template_result_window_invalid")
        else:
            if datetime.now(timezone.utc) < not_before:
                blockers.append("template_result_window_not_reached")
    evidence = template.get("evidence") or []
    has_official_result = False
    for item in evidence:
        if not isinstance(item, dict):
            continue
        if str(item.get("url") or "").strip().upper() == "TO_FILL":
            blockers.append("evidence_url_to_fill")
        if evidence_has_post_result_marker(item):
            has_official_result = True
    if not has_official_result:
        blockers.append("missing_official_result_evidence")
    return sorted(set(blockers))


def load_template_status(templates_dir: Path) -> list[dict[str, Any]]:
    if not templates_dir.exists():
        return []
    statuses: list[dict[str, Any]] = []
    for path in sorted(templates_dir.glob("*.json")):
        template = load_json(path, {})
        if not isinstance(template, dict):
            continue
        prepared_from = template.get("prepared_from") or {}
        blockers = template_blockers(template)
        statuses.append({
            "file": path.name,
            "id": template.get("id"),
            "disabled": bool(template.get("disabled", False)),
            "template_status": template.get("template_status"),
            "prepared_at": template.get("prepared_at"),
            "market": template.get("market"),
            "family": template.get("family"),
            "budget": template.get("budget"),
            "min_expected_value": template.get("min_expected_value"),
            "result_not_before_utc": template.get("result_not_before_utc"),
            "world_cup_post_result": bool(template.get("world_cup_post_result", False)),
            "label": prepared_from.get("label"),
            "opportunity_status": prepared_from.get("status"),
            "quote_expected_value_xp": prepared_from.get("quote_expected_value_xp"),
            "belief_gap_improvement_xp": prepared_from.get("belief_gap_improvement_xp"),
            "current_blocker": prepared_from.get("current_blocker"),
            "queue_ready": not blockers,
            "blockers": blockers,
        })
    return statuses


def next_template_window(
    templates: list[dict[str, Any]],
    now: datetime | None = None,
    *,
    queueable_opportunities_only: bool = False,
) -> dict[str, Any] | None:
    now = now or datetime.now(timezone.utc)
    candidates: list[tuple[datetime, dict[str, Any]]] = []
    for template in templates:
        if queueable_opportunities_only:
            status = str(template.get("opportunity_status") or "").lower()
            if status not in QUEUEABLE_TEMPLATE_OPPORTUNITIES:
                continue
            blockers = set(template.get("blockers") or [])
            if blockers & NEXT_DURABLE_WINDOW_SKIP_BLOCKERS:
                continue
        raw_window = template.get("result_not_before_utc")
        if not raw_window:
            continue
        blockers = set(template.get("blockers") or [])
        if "template_result_window_not_reached" not in blockers:
            continue
        try:
            window = parse_utc_timestamp(raw_window)
        except (TypeError, ValueError):
            continue
        if window > now:
            candidates.append((window, template))
    if not candidates:
        return None
    window, template = min(candidates, key=lambda item: item[0])
    return {
        "id": template.get("id"),
        "label": template.get("label"),
        "result_not_before_utc": window.isoformat().replace("+00:00", "Z"),
        "opportunity_status": template.get("opportunity_status"),
        "quote_expected_value_xp": template.get("quote_expected_value_xp"),
        "belief_gap_improvement_xp": template.get("belief_gap_improvement_xp"),
    }


def post_result_evidence_due(
    templates: list[dict[str, Any]],
    now: datetime | None = None,
) -> list[dict[str, Any]]:
    now = now or datetime.now(timezone.utc)
    due: list[dict[str, Any]] = []
    evidence_blockers = {
        "missing_official_result_evidence",
        "missing_world_cup_post_result",
        "evidence_url_to_fill",
    }
    for template in templates:
        raw_window = template.get("result_not_before_utc")
        if not raw_window:
            continue
        try:
            window = parse_utc_timestamp(raw_window)
        except (TypeError, ValueError):
            continue
        if window > now:
            continue
        blockers = set(template.get("blockers") or [])
        if "template_result_window_not_reached" in blockers:
            continue
        if not (blockers & evidence_blockers):
            continue
        due.append({
            "id": template.get("id"),
            "label": template.get("label"),
            "result_not_before_utc": window.isoformat().replace("+00:00", "Z"),
            "opportunity_status": template.get("opportunity_status"),
            "quote_expected_value_xp": template.get("quote_expected_value_xp"),
            "belief_gap_improvement_xp": template.get("belief_gap_improvement_xp"),
            "blockers": sorted(blockers),
        })
    return sorted(due, key=lambda item: (str(item.get("result_not_before_utc")), str(item.get("id"))))


def candidate_from_template(template: dict[str, Any]) -> dict[str, Any]:
    candidate = {key: template[key] for key in CANDIDATE_TEMPLATE_FIELDS if key in template}
    candidate["source_template_id"] = template.get("id")
    prepared_from = template.get("prepared_from")
    if prepared_from:
        candidate["prepared_from"] = prepared_from
    return candidate


def existing_candidate_ids(candidates_path: Path) -> set[str]:
    return {str(candidate.get("id")) for candidate in load_candidates(candidates_path) if candidate.get("id")}


def promote_ready_templates(templates_dir: Path, candidates_path: Path, *, append: bool = False) -> dict[str, Any]:
    existing_ids = existing_candidate_ids(candidates_path)
    promoted: list[dict[str, Any]] = []
    skipped: list[dict[str, Any]] = []
    if not templates_dir.exists():
        return {"append": append, "promoted": promoted, "skipped": skipped}

    for path in sorted(templates_dir.glob("*.json")):
        template = load_json(path, {})
        if not isinstance(template, dict):
            skipped.append({"file": path.name, "reason": "template is not a JSON object"})
            continue
        template_id = str(template.get("id") or path.stem)
        blockers = template_blockers(template)
        if blockers:
            skipped.append({"id": template_id, "file": path.name, "reason": "blocked", "blockers": blockers})
            continue
        if template_id in existing_ids:
            skipped.append({"id": template_id, "file": path.name, "reason": "duplicate_candidate_id"})
            continue
        candidate = candidate_from_template(template)
        candidate.setdefault("id", template_id)
        check_no_secret(json.dumps(candidate, sort_keys=True), f"template {template_id}")
        if append:
            candidates_path.parent.mkdir(parents=True, exist_ok=True)
            append_jsonl(candidates_path, candidate)
            existing_ids.add(str(candidate["id"]))
        promoted.append({
            "id": candidate["id"],
            "file": path.name,
            "market": candidate.get("market"),
            "appended": append,
        })

    return {"append": append, "promoted": promoted, "skipped": skipped}


def account_snapshot() -> dict[str, Any]:
    account = deadeye_json(["account", "show"], timeout=45, attempts=3)
    collateral = deadeye_json(["collateral", "balance"], timeout=45, attempts=3)
    return {"account": account, "collateral": collateral}


def market_is_tradeable(market: dict[str, Any]) -> bool:
    state = market.get("state") or {}
    settled = bool(market.get("settled") or state.get("isSettled"))
    paused = bool(state.get("isPaused"))
    initialized = state.get("isInitialized")
    if initialized is None:
        initialized = True
    return bool(market.get("isActive", market.get("is_active", False))) and not settled and not paused and bool(initialized)


def discover_filter_slugs(markets: list[dict[str, Any]]) -> list[str]:
    slugs: set[str] = set()
    for market in markets:
        for key in ("domain", "category"):
            value = market.get(key)
            if value:
                slug = slugify(str(value))
                if slug:
                    slugs.add(slug)
        for key in ("topics", "tags"):
            values = market.get(key) or []
            if not isinstance(values, list):
                values = [values]
            for value in values:
                slug = slugify(str(value))
                if slug:
                    slugs.add(slug)
    return sorted(slugs)


def market_search_text(market: dict[str, Any]) -> str:
    resolution = market.get("resolution") or {}
    parts: list[str] = [
        str(market.get("title") or ""),
        str(market.get("category") or ""),
        str(market.get("domain") or ""),
        str(market.get("resolutionSource") or ""),
        str(market.get("resolutionCriteria") or ""),
        str(resolution.get("source") or ""),
        str(resolution.get("criteria") or ""),
        str(resolution.get("metric") or ""),
    ]
    for key in ("topics", "tags"):
        values = market.get(key) or []
        if isinstance(values, list):
            parts.extend(str(item) for item in values)
        else:
            parts.append(str(values))
    return " ".join(parts).lower()


def is_world_cup_market(market: dict[str, Any]) -> bool:
    text = market_search_text(market)
    slug = slugify(text)
    return "world cup" in text or "fifa world cup" in text or "world-cup" in slug


def compute_rank(rows: list[dict[str, Any]], trader: str, own_pnl: float | None = None) -> dict[str, Any]:
    own = canonical_address(trader)
    top_pnl = float(rows[0].get("totalPnl", 0.0)) if rows else 0.0
    own_row = None
    rank = None
    for index, row in enumerate(rows, 1):
        if canonical_address(row.get("trader")) == own:
            own_row = row
            rank = index
            break
    pnl = float(own_row.get("totalPnl", own_pnl or 0.0)) if own_row else float(own_pnl or 0.0)
    gap = max(0.0, top_pnl - pnl) if rank != 1 else 0.0
    return {
        "rank": rank,
        "pnl": pnl,
        "gap_to_first": gap,
        "top_pnl": top_pnl,
        "top_trader": rows[0].get("trader") if rows else None,
        "markets_traded": own_row.get("marketsTraded") if own_row else None,
        "total_trades": own_row.get("totalTrades") if own_row else None,
    }


def ranking_rows_signature(rows: list[dict[str, Any]], *, limit: int = 50) -> list[tuple[str, float, int, int]]:
    signature: list[tuple[str, float, int, int]] = []
    for row in rows[:limit]:
        signature.append((
            canonical_address(row.get("trader")),
            round(float(row.get("totalPnl", 0.0)), 12),
            int(row.get("marketsTraded") or 0),
            int(row.get("totalTrades") or 0),
        ))
    return signature


def classify_ranking_view(view: dict[str, Any], overall_signature: list[tuple[str, float, int, int]] | None) -> dict[str, Any]:
    signature = view.pop("_rows_signature", None)
    if view.get("healthy") and overall_signature and signature == overall_signature:
        view["healthy"] = False
        view["status"] = "mirrored"
        view["error"] = "ranking view mirrors overall board"
        view["mirror_of"] = "overall"
    return view


def build_rankings_path(
    *,
    limit: int = 100,
    domain: str | None = None,
    from_ts: int | None = None,
    to_ts: int | None = None,
) -> str:
    params: dict[str, str] = {"limit": str(limit)}
    if domain:
        params["domain"] = domain
    if from_ts is not None:
        params["from"] = str(from_ts)
    if to_ts is not None:
        params["to"] = str(to_ts)
    return "/api/rankings?" + urllib.parse.urlencode(params)


def build_stats_path(
    trader: str,
    *,
    domain: str | None = None,
    from_ts: int | None = None,
    to_ts: int | None = None,
) -> str:
    params: dict[str, str] = {}
    if domain:
        params["domain"] = domain
    if from_ts is not None:
        params["from"] = str(from_ts)
    if to_ts is not None:
        params["to"] = str(to_ts)
    path = f"/api/positions/{canonical_address(trader)}/stats"
    if params:
        path += "?" + urllib.parse.urlencode(params)
    return path


def fetch_rankings_view(
    indexer_url: str,
    trader: str,
    *,
    domain: str | None = None,
    from_ts: int | None = None,
    to_ts: int | None = None,
) -> dict[str, Any]:
    code, payload = http_get_json(
        indexer_url,
        build_rankings_path(limit=100, domain=domain, from_ts=from_ts, to_ts=to_ts),
    )
    if code != 200 or not isinstance(payload, list):
        error = payload.get("error") if isinstance(payload, dict) else str(payload)[:120]
        return {"healthy": False, "status": code, "error": error}

    stats_code, stats = http_get_json(
        indexer_url,
        build_stats_path(trader, domain=domain, from_ts=from_ts, to_ts=to_ts),
    )
    own_pnl = 0.0
    if stats_code == 200 and isinstance(stats, dict):
        own_pnl = float(stats.get("totalPnl", 0.0))

    result = {
        "healthy": True,
        **compute_rank(payload, trader, own_pnl),
        "_rows_signature": ranking_rows_signature(payload),
    }
    if stats_code != 200:
        result["stats_status"] = stats_code
        if isinstance(stats, dict) and stats.get("error"):
            result["stats_error"] = stats.get("error")
    return result


def gas_tier(strk_balance: float) -> str:
    if strk_balance < GAS_HARD_STOP:
        return "hard_stop"
    if strk_balance < GAS_STRONG_WARN:
        return "strong_warn"
    if strk_balance < GAS_WARN:
        return "warn"
    return "ok"


def available_xp(balance_xp: float) -> float:
    return max(0.0, balance_xp - XP_RESERVE)


def select_ladder_budget(
    requested: float | None,
    balance_xp: float,
    *,
    require_requested: bool = False,
) -> float:
    cap = available_xp(balance_xp)
    if requested is None:
        if require_requested:
            raise LoopError("candidate budget is required for live execution")
        cap = min(cap, XP_LADDER[0])
    else:
        cap = min(cap, float(requested))
    allowed = [value for value in XP_LADDER if value <= cap]
    if not allowed:
        raise LoopError(f"no XP ladder budget fits available XP after {XP_RESERVE:g} XP reserve")
    return max(allowed)


def trade_history_last_hour(state: dict[str, Any], ts: int | None = None) -> list[dict[str, Any]]:
    ts = ts or now_ts()
    history = state.get("trade_history") or []
    return [entry for entry in history if ts - int(entry.get("timestamp", 0)) <= 3600]


def update_campaign_loss_guard(state: dict[str, Any], stats: dict[str, Any]) -> dict[str, Any]:
    current_pnl = float(stats.get("totalPnl", 0.0) if stats else 0.0)
    campaign = state.setdefault("campaign", {})
    campaign.setdefault("start_pnl", current_pnl)
    campaign["high_water_pnl"] = max(float(campaign.get("high_water_pnl", current_pnl)), current_pnl)
    campaign["current_pnl"] = current_pnl
    campaign["loss_from_start"] = max(0.0, float(campaign["start_pnl"]) - current_pnl)
    campaign["drawdown_from_high_water"] = max(0.0, float(campaign["high_water_pnl"]) - current_pnl)
    campaign["loss_halt_xp"] = CAMPAIGN_LOSS_HALT_XP
    campaign["loss_halt"] = (
        campaign["loss_from_start"] >= CAMPAIGN_LOSS_HALT_XP
        or campaign["drawdown_from_high_water"] >= CAMPAIGN_LOSS_HALT_XP
    )
    return campaign


def validate_candidate(candidate: dict[str, Any], market_meta: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    for key in ("id", "market", "belief"):
        if key not in candidate:
            errors.append(f"missing {key}")
    family = str(candidate.get("family") or market_meta.get("marketType") or market_meta.get("family") or "").lower()
    if family not in {"normal", "lognormal"}:
        errors.append(f"unsupported family {family or '-'}")
    if not candidate.get("belief_sigma"):
        errors.append("missing belief_sigma")
    rationale = str(candidate.get("rationale") or "")
    if len(rationale.strip()) < MIN_RATIONALE_CHARS:
        errors.append("rationale is too short")
    evidence = candidate.get("evidence") or []
    if not isinstance(evidence, list) or not evidence:
        errors.append("missing evidence list")
    else:
        for index, item in enumerate(evidence, 1):
            if not isinstance(item, dict):
                errors.append(f"evidence {index} is not an object")
                continue
            if not (item.get("url") or item.get("source")):
                errors.append(f"evidence {index} needs url or source")
            if not item.get("claim"):
                errors.append(f"evidence {index} needs claim")
    category = str(market_meta.get("category") or "").lower()
    title = str(market_meta.get("title") or "").lower()
    if "economics" in category or "inflation" in title or "cpi" in title:
        urls = " ".join(str(item.get("url") or item.get("source") or "").lower() for item in evidence if isinstance(item, dict))
        role_ok = any(str(item.get("source_role") or "").lower() in {"official_measurement", "quantitative_input", "leading_indicator", "market_prior"} for item in evidence if isinstance(item, dict))
        if not (role_ok or any(hint in urls for hint in OFFICIAL_ECON_HINTS)):
            errors.append("economics candidate needs official/primary evidence hint")
    if is_world_cup_market(market_meta):
        if len(evidence) < 2:
            errors.append("World Cup candidate needs at least two evidence items")
        post_result_ok = bool(candidate.get("world_cup_post_result"))
        for item in evidence:
            if not isinstance(item, dict):
                continue
            if evidence_has_post_result_marker(item):
                post_result_ok = True
        if not post_result_ok:
            errors.append("World Cup candidate needs post-result evidence marker")
    return errors


def position_lot_count(position: dict[str, Any]) -> int:
    if not position.get("hasPosition", True):
        return 0
    for key in ("deltaCount", "lots", "lotCount"):
        value = position.get(key)
        if value is not None:
            try:
                return max(0, int(value))
            except (TypeError, ValueError):
                pass
    return 1


def settlement_key(market: dict[str, Any]) -> str:
    resolution = market.get("resolution") or {}
    parts = [
        market.get("title"),
        market.get("category"),
        resolution.get("metric"),
        resolution.get("units"),
        resolution.get("source") or market.get("resolutionSource"),
        (resolution.get("criteria") or market.get("resolutionCriteria") or "")[:160],
    ]
    return slugify(" ".join(str(part) for part in parts if part))


def candidate_lot_limit(candidate: dict[str, Any], key: str, default: int) -> int:
    value = candidate.get(key, default)
    try:
        requested = int(value)
    except (TypeError, ValueError):
        requested = default
    return max(1, min(default, requested))


def concentration_errors(
    candidate: dict[str, Any],
    market_meta: dict[str, Any],
    positions: list[dict[str, Any]],
    markets: list[dict[str, Any]],
) -> list[str]:
    errors: list[str] = []
    market_limit = candidate_lot_limit(candidate, "max_market_lots", MAX_LOTS_PER_MARKET)
    settlement_limit = candidate_lot_limit(candidate, "max_settlement_lots", MAX_LOTS_PER_SETTLEMENT)
    candidate_market = canonical_address(candidate.get("market"))
    candidate_settlement = settlement_key(market_meta)
    market_lots = 0
    settlement_lots = 0
    for position in positions:
        lots = position_lot_count(position)
        if lots <= 0:
            continue
        position_market = canonical_address(position.get("marketAddress") or position.get("market"))
        if position_market == candidate_market:
            market_lots += lots
        position_meta = market_meta_by_address(markets, position_market)
        if position_meta and settlement_key(position_meta) == candidate_settlement:
            settlement_lots += lots
    post_market_lots = market_lots + 1
    post_settlement_lots = settlement_lots + 1
    if post_market_lots > market_limit:
        errors.append(
            f"market concentration cap: existing_lots={market_lots}, post_trade_lots={post_market_lots}, cap={market_limit}"
        )
    if post_settlement_lots > settlement_limit:
        errors.append(
            "settlement concentration cap: "
            f"existing_lots={settlement_lots}, post_trade_lots={post_settlement_lots}, cap={settlement_limit}"
        )
    return errors


def run_smoke(smoke_script: Path, smoke_market: str) -> dict[str, Any]:
    started = utc_now()
    if smoke_script.exists():
        attempts: list[dict[str, Any]] = []
        for attempt in range(1, SMOKE_SCRIPT_ATTEMPTS + 1):
            result = run_cmd(["zsh", str(smoke_script), smoke_market], timeout=90, check=False)
            summary = (result.stdout or result.stderr).strip().splitlines()[-3:]
            attempts.append({"attempt": attempt, "returncode": result.returncode, "summary": summary})
            if result.returncode == 0:
                return {
                    "ok": True,
                    "started_at": started,
                    "script": str(smoke_script),
                    "version": smoke_output_version(result.stdout + "\n" + result.stderr),
                    "attempts": attempts,
                    "summary": result.stdout.strip().splitlines()[-1:],
                }
            if attempt < SMOKE_SCRIPT_ATTEMPTS:
                time.sleep(SMOKE_SCRIPT_RETRY_DELAY_SECONDS)
        last = attempts[-1]
        raise LoopError(
            f"smoke script failed after {SMOKE_SCRIPT_ATTEMPTS} attempts rc={last['returncode']}: "
            + " | ".join(str(line) for line in last["summary"])
        )
    version = run_cmd(["deadeye", "--version"], timeout=15, attempts=3)
    run_cmd(["deadeye", "markets", "list", "--limit", "3", "--output", "plain"], timeout=45, attempts=3)
    deadeye_json(["markets", "show", smoke_market], timeout=45, attempts=3)
    doctor = deadeye_json(["doctor", "--market", smoke_market], timeout=60, attempts=3)
    if not doctor.get("all_ok"):
        raise LoopError("built-in smoke doctor was not all_ok")
    return {"ok": True, "started_at": started, "script": "built-in", "version": version.stdout.strip()}


def smoke_output_version(text: str) -> str | None:
    match = re.search(r"\bdeadeye\s+[0-9][^\s]*", text)
    return match.group(0) if match else None


def monitor(indexer_url: str, trader: str) -> dict[str, Any]:
    status_health, health = http_get_json(indexer_url, "/health")
    status_markets, markets = http_get_json(indexer_url, "/api/markets")
    if status_markets != 200 or not isinstance(markets, list):
        raise LoopError(f"indexer markets unavailable status={status_markets}")
    active_markets = [market for market in markets if market_is_tradeable(market)]
    status_stats, stats = http_get_json(indexer_url, f"/api/positions/{canonical_address(trader)}/stats")
    if status_stats != 200 or not isinstance(stats, dict):
        stats = {}
    status_positions, positions = http_get_json(indexer_url, f"/api/positions/{canonical_address(trader)}")
    if status_positions != 200 or not isinstance(positions, list):
        positions = []
    overall = fetch_rankings_view(indexer_url, trader)
    if not overall.get("healthy"):
        raise LoopError(f"indexer rankings unavailable status={overall.get('status')}")
    overall_signature = overall.pop("_rows_signature", None)
    overall.pop("healthy", None)
    filter_status: dict[str, Any] = {}
    for slug in discover_filter_slugs(markets):
        filter_status[slug] = classify_ranking_view(
            fetch_rankings_view(indexer_url, trader, domain=slug),
            overall_signature,
        )
    current_ts = now_ts()
    time_window_status: dict[str, Any] = {}
    filter_time_window_status: dict[str, Any] = {}
    for label, seconds in RANKING_TIME_WINDOWS:
        from_ts = current_ts - seconds
        time_window_status[label] = classify_ranking_view(
            fetch_rankings_view(indexer_url, trader, from_ts=from_ts),
            overall_signature,
        )
    healthy_slugs = [slug for slug, item in filter_status.items() if item.get("healthy")]
    for slug in healthy_slugs:
        for label, seconds in RANKING_TIME_WINDOWS:
            from_ts = current_ts - seconds
            filter_time_window_status[f"{slug}/{label}"] = classify_ranking_view(
                fetch_rankings_view(
                    indexer_url,
                    trader,
                    domain=slug,
                    from_ts=from_ts,
                ),
                overall_signature,
            )
    return {
        "health": {"status": status_health, "payload": health},
        "markets": {"total": len(markets), "active_tradeable": len(active_markets), "items": markets},
        "rankings": {
            "overall": overall,
            "filters": filter_status,
            "time_windows": time_window_status,
            "filter_time_windows": filter_time_window_status,
        },
        "stats": stats,
        "positions": positions,
    }


def market_meta_by_address(markets: list[dict[str, Any]], address: str) -> dict[str, Any] | None:
    target = canonical_address(address)
    for market in markets:
        if canonical_address(market.get("address")) == target:
            return market
    return None


def quote_candidate(candidate: dict[str, Any], budget: float, balance_xp: float, market_meta: dict[str, Any]) -> dict[str, Any]:
    family = str(candidate.get("family") or market_meta.get("marketType") or market_meta.get("family")).lower()
    args = [
        "trade",
        "quote",
        str(candidate["market"]),
        "--family",
        family,
        "--belief",
        str(candidate["belief"]),
        "--belief-sigma",
        str(candidate["belief_sigma"]),
        "--budget",
        f"{budget:g}",
        "--bankroll",
        f"{balance_xp:g}",
        "--risk",
        "aggressive",
    ]
    return deadeye_json(args, timeout=90)


def execute_candidate(
    candidate: dict[str, Any],
    budget: float,
    balance_xp: float,
    quote: dict[str, Any],
    market_meta: dict[str, Any],
    *,
    execute: bool,
    journal: Path,
) -> dict[str, Any]:
    family = str(candidate.get("family") or market_meta.get("marketType") or market_meta.get("family")).lower()
    collateral = float(quote.get("padded_collateral") or quote.get("required_collateral") or 0.0)
    if collateral <= 0:
        raise LoopError("quote did not return positive collateral")
    max_collateral = collateral * 1.05
    base_args = [
        "trade",
        "execute",
        str(candidate["market"]),
        "--family",
        family,
        "--belief",
        str(candidate["belief"]),
        "--belief-sigma",
        str(candidate["belief_sigma"]),
        "--budget",
        f"{budget:g}",
        "--bankroll",
        f"{balance_xp:g}",
        "--risk",
        "aggressive",
        "--max-collateral",
        f"{max_collateral:.6f}",
        "--journal",
        str(journal),
        "--confirm",
    ]
    dry = deadeye_json([*base_args, "--dry-run"], timeout=120)
    if not execute:
        return {"dry_run": dry, "submitted": False, "max_collateral": max_collateral}
    submitted = deadeye_json(base_args, timeout=180)
    return {"dry_run": dry, "submitted": True, "receipt": submitted, "max_collateral": max_collateral}


def process_candidates(
    candidates: list[dict[str, Any]],
    markets: list[dict[str, Any]],
    positions: list[dict[str, Any]],
    account: dict[str, Any],
    collateral: dict[str, Any],
    stats: dict[str, Any],
    state: dict[str, Any],
    args: argparse.Namespace,
    events_path: Path,
) -> list[dict[str, Any]]:
    processed: list[dict[str, Any]] = []
    processed_ids = set(state.get("processed_candidate_ids") or [])
    balance_xp = float(collateral.get("balance_xp", 0.0))
    strk_balance = float(account.get("strk_balance_strk", 0.0))
    if gas_tier(strk_balance) == "hard_stop":
        return [{"id": None, "status": "write_stopped", "reason": f"STRK balance {strk_balance:.6f} below {GAS_HARD_STOP:g} hard stop"}]
    campaign = update_campaign_loss_guard(state, stats)
    if args.execute and campaign["loss_halt"]:
        return [{
            "id": None,
            "status": "write_stopped",
            "reason": (
                f"campaign XP loss halt tripped: loss_from_start={campaign['loss_from_start']:.6f}, "
                f"drawdown={campaign['drawdown_from_high_water']:.6f}, halt={CAMPAIGN_LOSS_HALT_XP:g}"
            ),
        }]
    hour_history = trade_history_last_hour(state)
    executed_this_loop = 0
    for candidate in candidates:
        candidate_id = str(candidate.get("id") or "")
        if not candidate_id or candidate_id in processed_ids or candidate.get("disabled"):
            continue
        if executed_this_loop >= MAX_TRADES_PER_LOOP:
            processed.append({"id": candidate_id, "status": "skipped", "reason": "per-loop trade cap reached"})
            continue
        if len(hour_history) + executed_this_loop >= MAX_TRADES_PER_HOUR:
            processed.append({"id": candidate_id, "status": "skipped", "reason": "hourly trade cap reached"})
            continue
        market_meta = market_meta_by_address(markets, str(candidate.get("market")))
        if not market_meta:
            processed.append({"id": candidate_id, "status": "failed", "reason": "market not found on indexer"})
            processed_ids.add(candidate_id)
            continue
        errors = validate_candidate(candidate, market_meta)
        errors.extend(concentration_errors(candidate, market_meta, positions, markets))
        if errors:
            processed.append({"id": candidate_id, "status": "failed", "reason": "; ".join(errors)})
            processed_ids.add(candidate_id)
            continue
        if not market_is_tradeable(market_meta):
            processed.append({"id": candidate_id, "status": "failed", "reason": "market not tradeable"})
            processed_ids.add(candidate_id)
            continue
        try:
            if args.execute:
                fresh_snap = account_snapshot()
                fresh_account = fresh_snap.get("account") or {}
                fresh_collateral = fresh_snap.get("collateral") or {}
                strk_balance = float(fresh_account.get("strk_balance_strk", strk_balance))
                balance_xp = float(fresh_collateral.get("balance_xp", balance_xp))
                tier = gas_tier(strk_balance)
                if tier == "hard_stop":
                    processed.append({
                        "id": candidate_id,
                        "status": "write_stopped",
                        "reason": f"fresh STRK balance {strk_balance:.6f} below {GAS_HARD_STOP:g} hard stop",
                    })
                    break
            budget = select_ladder_budget(
                candidate.get("budget"),
                balance_xp,
                require_requested=bool(args.execute),
            )
            doctor = deadeye_json(["doctor", "--market", str(candidate["market"])], timeout=60)
            if not doctor.get("all_ok"):
                raise LoopError("doctor was not all_ok for market")
            quote = quote_candidate(candidate, budget, balance_xp, market_meta)
            if not quote.get("on_chain_will_accept"):
                raise LoopError(f"quote rejected: {quote.get('rejection')}")
            min_ev = max(MIN_EXECUTE_EV, float(candidate.get("min_expected_value", MIN_EXECUTE_EV)))
            if float(quote.get("expected_value") or 0.0) < min_ev:
                raise LoopError(f"quote expected value did not clear {min_ev:g} XP threshold")
            receipt = execute_candidate(
                candidate,
                budget,
                balance_xp,
                quote,
                market_meta,
                execute=bool(args.execute),
                journal=args.trade_journal,
            )
            status = "executed" if receipt.get("submitted") else "dry_run_ok"
            event = {
                "type": f"trade.{status}",
                "timestamp": now_ts(),
                "candidate_id": candidate_id,
                "market": candidate["market"],
                "family": str(candidate.get("family") or market_meta.get("marketType")).lower(),
                "budget": budget,
                "expected_value": quote.get("expected_value"),
                "min_expected_value": min_ev,
                "max_collateral": receipt.get("max_collateral"),
                "submitted": receipt.get("submitted"),
            }
            append_jsonl(events_path, event)
            if receipt.get("submitted"):
                executed_this_loop += 1
                hour_history.append({"timestamp": event["timestamp"], "candidate_id": candidate_id})
                state.setdefault("trade_history", []).append({"timestamp": event["timestamp"], "candidate_id": candidate_id})
            processed.append({
                "id": candidate_id,
                "status": status,
                "budget": budget,
                "expected_value": quote.get("expected_value"),
                "min_expected_value": min_ev,
            })
            processed_ids.add(candidate_id)
        except Exception as exc:  # noqa: BLE001 - failures are recorded for the operator loop.
            processed.append({"id": candidate_id, "status": "failed", "reason": str(exc)})
            processed_ids.add(candidate_id)
    state["processed_candidate_ids"] = sorted(processed_ids)
    state["trade_history"] = trade_history_last_hour(state)
    return processed


def latest_active_portfolio_scout(state_dir: Path) -> dict[str, Any] | None:
    files = sorted(
        state_dir.glob("gap-analysis-active-portfolio-ladder-quote-*.json"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    for path in files:
        payload = load_json(path, {})
        if not isinstance(payload, dict):
            continue
        results = payload.get("results") or []
        if not isinstance(results, list):
            results = []
        ev_floor = [
            item for item in results
            if float(((item.get("quote") or {}).get("expected_value")) or 0.0) >= MIN_EXECUTE_EV
        ]
        runner_pass = [
            item for item in results
            if (item.get("runner_gate") or {}).get("would_pass_current_runner") is True
        ]
        top_signals: list[dict[str, Any]] = []
        for item in results[:5]:
            quote = item.get("quote") or {}
            gate = item.get("runner_gate") or {}
            top_signals.append({
                "label": item.get("label"),
                "budget": item.get("budget"),
                "expected_value": quote.get("expected_value"),
                "blockers": gate.get("blockers") or [],
                "would_pass_current_runner": bool(gate.get("would_pass_current_runner")),
            })
        return {
            "generated_at": payload.get("generated_at"),
            "coverage": payload.get("coverage"),
            "rows": len(results),
            "ev_floor_rows": len(ev_floor),
            "runner_pass_rows": len(runner_pass),
            "top_signals": top_signals,
        }
    return None


def scout_age_seconds(scout: dict[str, Any] | None, *, now: datetime | None = None) -> float | None:
    if not scout or not scout.get("generated_at"):
        return None
    now = now or datetime.now(timezone.utc)
    try:
        generated_at = parse_utc_timestamp(scout.get("generated_at"))
    except (TypeError, ValueError):
        return None
    return max(0.0, (now - generated_at).total_seconds())


def active_portfolio_scout_is_stale(
    scout: dict[str, Any] | None,
    *,
    max_age_minutes: float,
    now: datetime | None = None,
) -> bool:
    age = scout_age_seconds(scout, now=now)
    if age is None:
        return True
    return age >= max(0.0, float(max_age_minutes)) * 60.0


def active_portfolio_scout_output_path(state_dir: Path) -> Path:
    stamp = utc_now().replace("-", "").replace(":", "")
    return state_dir / f"gap-analysis-active-portfolio-ladder-quote-4000-{stamp}.json"


def refresh_active_portfolio_scout_if_stale(
    state_dir: Path,
    *,
    max_age_minutes: float,
    force: bool = False,
) -> dict[str, Any]:
    before = latest_active_portfolio_scout(state_dir)
    age = scout_age_seconds(before)
    stale = force or active_portfolio_scout_is_stale(before, max_age_minutes=max_age_minutes)
    result: dict[str, Any] = {
        "attempted": False,
        "refreshed": False,
        "stale": stale,
        "max_age_minutes": max_age_minutes,
        "previous_generated_at": before.get("generated_at") if before else None,
        "previous_age_seconds": age,
    }
    if not stale:
        result["status"] = "fresh"
        return result

    output_path = active_portfolio_scout_output_path(state_dir)
    args = [
        sys.executable,
        str(REPO_ROOT / "scripts" / "storm_gap_analyzer.py"),
        "--preset",
        "active-portfolio-20260612",
        "--budget",
        f"{ACTIVE_PORTFOLIO_SCOUT_BUDGET:g}",
        "--budget-ladder",
        "--quote-only",
        "--sort-by",
        "ev",
        "--output",
        str(output_path),
    ]
    result["attempted"] = True
    result["output_file"] = output_path.name
    cmd_result = run_cmd(args, timeout=300, check=False)
    if cmd_result.returncode != 0:
        tail = (cmd_result.stderr or cmd_result.stdout).strip().splitlines()[-3:]
        result["status"] = "failed"
        result["returncode"] = cmd_result.returncode
        result["error_tail"] = tail
        return result

    after = latest_active_portfolio_scout(state_dir)
    result["status"] = "refreshed"
    result["refreshed"] = True
    result["generated_at"] = after.get("generated_at") if after else None
    return result


def scrub_market_fingerprint_value(value: Any) -> Any:
    if isinstance(value, dict):
        scrubbed = {}
        for key in sorted(value):
            lowered = str(key).lower()
            if any(part in lowered for part in VOLATILE_MARKET_KEY_PARTS):
                continue
            scrubbed[key] = scrub_market_fingerprint_value(value[key])
        return scrubbed
    if isinstance(value, list):
        return [scrub_market_fingerprint_value(item) for item in value]
    if isinstance(value, float):
        return round(value, 12)
    return value


def market_state_scout_refresh_key(markets: list[dict[str, Any]]) -> dict[str, Any]:
    items: list[dict[str, Any]] = []
    for market in markets:
        if not isinstance(market, dict):
            continue
        item: dict[str, Any] = {
            "address": canonical_address(market.get("address")),
            "tradeable": market_is_tradeable(market),
        }
        for key in sorted(MARKET_FINGERPRINT_KEYS):
            if key == "address":
                continue
            if key in market:
                item[key] = scrub_market_fingerprint_value(market.get(key))
        items.append(item)
    items.sort(key=lambda item: str(item.get("address")))
    return {
        "version": MARKET_STATE_FINGERPRINT_VERSION,
        "markets": items,
        "total": len(items),
        "active_tradeable": sum(1 for item in items if item.get("tradeable")),
    }


def market_state_refresh_summary(
    previous_key: dict[str, Any] | None,
    current_key: dict[str, Any] | None,
) -> dict[str, Any]:
    previous_items = {
        str(item.get("address")): item
        for item in (previous_key or {}).get("markets", [])
        if isinstance(item, dict)
    }
    current_items = {
        str(item.get("address")): item
        for item in (current_key or {}).get("markets", [])
        if isinstance(item, dict)
    }
    changed = sorted(
        address
        for address in set(previous_items) | set(current_items)
        if previous_items.get(address) != current_items.get(address)
    )
    return {
        "previous_total": (previous_key or {}).get("total"),
        "total": (current_key or {}).get("total"),
        "previous_active_tradeable": (previous_key or {}).get("active_tradeable"),
        "active_tradeable": (current_key or {}).get("active_tradeable"),
        "changed_markets": changed[:20],
        "changed_markets_truncated": len(changed) > 20,
    }


def watched_template_market_groups(templates: list[dict[str, Any]]) -> dict[str, list[str]]:
    groups: dict[str, list[str]] = {}
    for template in templates:
        if not isinstance(template, dict):
            continue
        if not template.get("result_not_before_utc"):
            continue
        address = canonical_address(template.get("market"))
        if not address:
            continue
        template_id = str(template.get("id") or template.get("file") or address)
        groups.setdefault(address, []).append(template_id)
    return {address: sorted(set(ids)) for address, ids in sorted(groups.items())}


def fetch_watched_template_market_states(templates: list[dict[str, Any]]) -> dict[str, Any]:
    watched: list[dict[str, Any]] = []
    failures: list[dict[str, Any]] = []
    for address, template_ids in watched_template_market_groups(templates).items():
        try:
            market_state = deadeye_json(["markets", "show", address], timeout=45, attempts=3)
        except Exception as exc:  # noqa: BLE001 - read-only watch failures should not stop the loop.
            failures.append({
                "address": address,
                "template_ids": template_ids,
                "error": str(exc)[:240],
            })
            continue
        watched.append({
            "address": address,
            "template_ids": template_ids,
            "market": market_state,
        })
    return {"markets": watched, "failures": failures}


def watched_template_market_state_key(watched_markets: list[dict[str, Any]]) -> dict[str, Any]:
    items: list[dict[str, Any]] = []
    for watched in watched_markets:
        if not isinstance(watched, dict):
            continue
        address = canonical_address(watched.get("address"))
        if not address:
            continue
        items.append({
            "address": address,
            "template_ids": sorted(str(item) for item in (watched.get("template_ids") or [])),
            "market": scrub_market_fingerprint_value(watched.get("market") or {}),
        })
    items.sort(key=lambda item: str(item.get("address")))
    return {
        "version": WATCHED_TEMPLATE_MARKET_STATE_FINGERPRINT_VERSION,
        "markets": items,
        "total": len(items),
    }


def watched_template_market_state_refresh_summary(
    previous_key: dict[str, Any] | None,
    current_key: dict[str, Any] | None,
) -> dict[str, Any]:
    previous_items = {
        str(item.get("address")): item
        for item in (previous_key or {}).get("markets", [])
        if isinstance(item, dict)
    }
    current_items = {
        str(item.get("address")): item
        for item in (current_key or {}).get("markets", [])
        if isinstance(item, dict)
    }
    changed = sorted(
        address
        for address in set(previous_items) | set(current_items)
        if previous_items.get(address) != current_items.get(address)
    )
    return {
        "previous_total": (previous_key or {}).get("total"),
        "total": (current_key or {}).get("total"),
        "changed_markets": changed[:20],
        "changed_markets_truncated": len(changed) > 20,
    }


def post_result_scout_refresh_key(due_templates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    key: list[dict[str, Any]] = []
    for item in due_templates:
        key.append({
            "id": item.get("id"),
            "result_not_before_utc": item.get("result_not_before_utc"),
            "blockers": sorted(str(blocker) for blocker in (item.get("blockers") or [])),
        })
    return sorted(key, key=lambda item: (str(item.get("result_not_before_utc")), str(item.get("id"))))


def maybe_refresh_active_portfolio_scout(
    state: dict[str, Any],
    state_dir: Path,
    *,
    max_age_minutes: float,
    due_templates: list[dict[str, Any]],
    markets: list[dict[str, Any]] | None = None,
    watched_template_market_states: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    due_key = post_result_scout_refresh_key(due_templates)
    due_state_key = "last_post_result_scout_refresh_key"
    if not due_key:
        state.pop(due_state_key, None)
    force_due_refresh = bool(due_key) and state.get(due_state_key) != due_key
    market_key = market_state_scout_refresh_key(markets or []) if markets is not None else None
    market_state_key = "last_market_state_scout_refresh_key"
    previous_market_key = state.get(market_state_key)
    previous_market_version = (previous_market_key or {}).get("version") if isinstance(previous_market_key, dict) else None
    force_market_refresh = (
        bool(market_key)
        and previous_market_key is not None
        and previous_market_version == MARKET_STATE_FINGERPRINT_VERSION
        and previous_market_key != market_key
    )
    watched_key = (
        watched_template_market_state_key(watched_template_market_states)
        if watched_template_market_states is not None
        else None
    )
    watched_state_key = "last_watched_template_market_state_key"
    if watched_key is not None and not watched_key.get("markets"):
        state.pop(watched_state_key, None)
    previous_watched_key = state.get(watched_state_key)
    previous_watched_version = (
        (previous_watched_key or {}).get("version")
        if isinstance(previous_watched_key, dict)
        else None
    )
    force_watched_refresh = (
        bool(watched_key and watched_key.get("markets"))
        and previous_watched_key is not None
        and previous_watched_version == WATCHED_TEMPLATE_MARKET_STATE_FINGERPRINT_VERSION
        and previous_watched_key != watched_key
    )
    force_refresh = force_due_refresh or force_market_refresh or force_watched_refresh
    refresh = refresh_active_portfolio_scout_if_stale(
        state_dir,
        max_age_minutes=max_age_minutes,
        force=force_refresh,
    )
    reasons: list[str] = []
    if force_due_refresh:
        reasons.append("post_result_evidence_due")
        refresh["due_templates"] = due_key
    if force_market_refresh:
        reasons.append("market_state_shift")
        refresh["market_state_shift"] = market_state_refresh_summary(previous_market_key, market_key)
    if force_watched_refresh:
        reasons.append("watched_template_market_state_shift")
        refresh["watched_template_market_state_shift"] = watched_template_market_state_refresh_summary(
            previous_watched_key,
            watched_key,
        )
    if reasons:
        refresh["reasons"] = reasons
        refresh["reason"] = reasons[0] if len(reasons) == 1 else "+".join(reasons)
    if refresh.get("status") != "failed":
        if due_key:
            state[due_state_key] = due_key
        if market_key:
            state[market_state_key] = market_key
        if watched_key and watched_key.get("markets"):
            state[watched_state_key] = watched_key
    return refresh


def scout_key(summary: dict[str, Any]) -> dict[str, Any] | None:
    scout = summary.get("active_portfolio_scout")
    if not scout:
        return None
    coverage = scout.get("coverage") or {}
    top_signals = []
    for item in (scout.get("top_signals") or [])[:5]:
        expected_value = item.get("expected_value")
        if expected_value is not None:
            expected_value = round(float(expected_value), 4)
        top_signals.append({
            "label": item.get("label"),
            "budget": item.get("budget"),
            "expected_value": expected_value,
            "would_pass_current_runner": bool(item.get("would_pass_current_runner")),
            "blockers": item.get("blockers") or [],
        })
    return {
        "active_tradeable": coverage.get("active_tradeable_markets"),
        "covered": coverage.get("covered_active_tradeable_markets"),
        "coverage_complete": coverage.get("coverage_complete"),
        "rows": scout.get("rows"),
        "ev_floor_rows": scout.get("ev_floor_rows"),
        "runner_pass_rows": scout.get("runner_pass_rows"),
        "top_signals": top_signals,
    }


def scout_refresh_key(summary: dict[str, Any]) -> dict[str, Any] | None:
    refresh = summary.get("active_portfolio_scout_refresh") or {}
    reasons = refresh.get("reasons") or []
    signal_reasons = {"market_state_shift", "watched_template_market_state_shift"}
    if refresh.get("status") != "failed" and signal_reasons.intersection(reasons):
        return {
            "status": refresh.get("status"),
            "reasons": reasons,
        }
    if refresh.get("status") != "failed":
        return None
    failed = {
        "status": "failed",
        "returncode": refresh.get("returncode"),
        "error_tail": refresh.get("error_tail") or [],
    }
    if reasons:
        failed["reasons"] = reasons
    return failed


def ranking_view_stats(views: dict[str, Any], *, rounded: bool = False) -> dict[str, Any]:
    stats: dict[str, Any] = {}
    for slug in sorted(views):
        item = views.get(slug) or {}
        if not item.get("healthy"):
            continue
        pnl = item.get("pnl")
        gap = item.get("gap_to_first")
        top_pnl = item.get("top_pnl")
        if rounded:
            pnl = round(float(pnl or 0.0), 4)
            gap = round(float(gap or 0.0), 4)
            top_pnl = round(float(top_pnl or 0.0), 4)
        stats[slug] = {
            "rank": item.get("rank"),
            "pnl": pnl,
            "gap_to_first": gap,
            "top_pnl": top_pnl,
            "markets_traded": item.get("markets_traded"),
            "total_trades": item.get("total_trades"),
        }
    return stats


def view_slugs_by_status(views: dict[str, Any], status: str) -> list[str]:
    return sorted(
        slug
        for slug, item in views.items()
        if isinstance(item, dict) and item.get("status") == status
    )


def unavailable_view_slugs(views: dict[str, Any]) -> list[str]:
    return sorted(
        slug
        for slug, item in views.items()
        if not (item or {}).get("healthy") and (item or {}).get("status") != "mirrored"
    )


def summary_key(summary: dict[str, Any]) -> dict[str, Any]:
    rankings = summary.get("rankings", {})
    filters = rankings.get("filters", {})
    time_windows = rankings.get("time_windows", {})
    filter_time_windows = rankings.get("filter_time_windows", {})
    unhealthy = unavailable_view_slugs(filters)
    unhealthy_time_windows = unavailable_view_slugs(time_windows)
    unhealthy_filter_time_windows = unavailable_view_slugs(filter_time_windows)
    mirrored = view_slugs_by_status(filters, "mirrored")
    mirrored_time_windows = view_slugs_by_status(time_windows, "mirrored")
    mirrored_filter_time_windows = view_slugs_by_status(filter_time_windows, "mirrored")
    processed = [
        {"id": item.get("id"), "status": item.get("status")}
        for item in summary.get("processed_candidates", [])
        if item.get("status") not in {"skipped"}
    ]
    promotion = summary.get("template_promotion") or {}
    promoted_templates = [
        {"id": item.get("id"), "appended": item.get("appended")}
        for item in promotion.get("promoted", [])
    ]
    due_templates = [
        {"id": item.get("id"), "opportunity_status": item.get("opportunity_status")}
        for item in post_result_evidence_due(summary.get("templates") or [])
    ]
    overall = rankings.get("overall", {})
    return {
        "rank": overall.get("rank"),
        "gap": round(float(overall.get("gap_to_first") or 0.0), 4),
        "pnl": round(float(overall.get("pnl") or 0.0), 4),
        "gas_tier": summary.get("gas_tier"),
        "unhealthy_filters": unhealthy,
        "unhealthy_time_windows": unhealthy_time_windows,
        "unhealthy_filter_time_windows": unhealthy_filter_time_windows,
        "mirrored_filters": mirrored,
        "mirrored_time_windows": mirrored_time_windows,
        "mirrored_filter_time_windows": mirrored_filter_time_windows,
        "healthy_view_ranks": {
            "filters": ranking_view_stats(filters, rounded=True),
            "time_windows": ranking_view_stats(time_windows, rounded=True),
            "filter_time_windows": ranking_view_stats(filter_time_windows, rounded=True),
        },
        "active_portfolio_scout": scout_key(summary),
        "active_portfolio_scout_refresh": scout_refresh_key(summary),
        "processed": processed,
        "promoted_templates": promoted_templates,
        "post_result_evidence_due": due_templates,
    }


def mailbox_keys_equivalent(stored: Any, current: dict[str, Any]) -> bool:
    if not isinstance(stored, dict):
        return False

    stable_fields = (
        "rank",
        "gap",
        "pnl",
        "gas_tier",
        "unhealthy_filters",
        "unhealthy_time_windows",
        "unhealthy_filter_time_windows",
        "mirrored_filters",
        "mirrored_time_windows",
        "mirrored_filter_time_windows",
        "healthy_view_ranks",
        "processed",
        "promoted_templates",
        "post_result_evidence_due",
    )
    for field in stable_fields:
        if field.startswith("mirrored_"):
            if (stored.get(field) or []) != (current.get(field) or []):
                return False
            continue
        if stored.get(field) != current.get(field):
            return False

    stored_scout = stored.get("active_portfolio_scout") or {}
    current_scout = current.get("active_portfolio_scout") or {}
    scout_fields = (
        "active_tradeable",
        "covered",
        "coverage_complete",
        "rows",
        "ev_floor_rows",
        "runner_pass_rows",
    )
    for field in scout_fields:
        if stored_scout.get(field) != current_scout.get(field):
            return False
    if "top_signals" in stored_scout and stored_scout.get("top_signals") != current_scout.get("top_signals"):
        return False

    stored_refresh = stored.get("active_portfolio_scout_refresh") or {}
    current_refresh = current.get("active_portfolio_scout_refresh") or {}
    if stored_refresh.get("status") == "failed" or current_refresh.get("status") == "failed":
        return stored_refresh == current_refresh
    return True


def format_unhealthy_views(key: dict[str, Any]) -> str:
    parts: list[str] = []
    if key.get("unhealthy_filters"):
        parts.append("domains=" + ", ".join(key["unhealthy_filters"]))
    if key.get("unhealthy_time_windows"):
        parts.append("time_windows=" + ", ".join(key["unhealthy_time_windows"]))
    if key.get("unhealthy_filter_time_windows"):
        parts.append("domain_time_windows=" + ", ".join(key["unhealthy_filter_time_windows"]))
    return "; ".join(parts) if parts else "none"


def format_mirrored_views(key: dict[str, Any]) -> str:
    parts: list[str] = []
    if key.get("mirrored_filters"):
        parts.append("domains=" + ", ".join(key["mirrored_filters"]))
    if key.get("mirrored_time_windows"):
        parts.append("time_windows=" + ", ".join(key["mirrored_time_windows"]))
    if key.get("mirrored_filter_time_windows"):
        parts.append("domain_time_windows=" + ", ".join(key["mirrored_filter_time_windows"]))
    return "; ".join(parts) if parts else "none"


def format_healthy_view_stats(key: dict[str, Any]) -> str:
    view_ranks = key.get("healthy_view_ranks") or {}
    parts: list[str] = []
    for group in ("filters", "time_windows", "filter_time_windows"):
        items = view_ranks.get(group) or {}
        if not items:
            continue
        formatted = []
        for slug, stats in items.items():
            rank = stats.get("rank")
            gap = stats.get("gap_to_first")
            formatted.append(f"{slug}:rank={rank},gap={gap}")
        parts.append(f"{group}=" + ", ".join(formatted))
    return "; ".join(parts) if parts else "overall only"


def format_active_portfolio_scout(summary: dict[str, Any]) -> str:
    scout = summary.get("active_portfolio_scout")
    if not scout:
        return "none"
    coverage = scout.get("coverage") or {}
    return (
        f"generated_at={scout.get('generated_at')}, "
        f"coverage={coverage.get('covered_active_tradeable_markets')}/"
        f"{coverage.get('active_tradeable_markets')}, "
        f"ev_floor_rows={scout.get('ev_floor_rows')}, "
        f"runner_pass_rows={scout.get('runner_pass_rows')}"
    )


def format_active_portfolio_scout_refresh(summary: dict[str, Any]) -> str:
    refresh = summary.get("active_portfolio_scout_refresh") or {}
    if not refresh:
        return "not requested"
    status = refresh.get("status")
    reasons = set(refresh.get("reasons") or [])
    if not reasons and refresh.get("reason"):
        reasons.add(str(refresh.get("reason")))
    prefix_parts = []
    if "post_result_evidence_due" in reasons:
        prefix_parts.append("post-result evidence due")
    if "market_state_shift" in reasons:
        prefix_parts.append("market state shift")
    if "watched_template_market_state_shift" in reasons:
        prefix_parts.append("watched template market state shift")
    prefix = "; ".join(prefix_parts)
    prefix = f"{prefix}; " if prefix else ""
    if status == "fresh":
        age = refresh.get("previous_age_seconds")
        age_text = f", age_seconds={age:.0f}" if isinstance(age, (int, float)) else ""
        return f"{prefix}fresh{age_text}"
    if status == "refreshed":
        return f"{prefix}refreshed generated_at={refresh.get('generated_at')}"
    if status == "failed":
        return f"{prefix}failed"
    return prefix + str(status or "unknown")


def compact_last_summary(summary: dict[str, Any]) -> dict[str, Any]:
    rankings = summary.get("rankings") or {}
    filters = rankings.get("filters") or {}
    time_windows = rankings.get("time_windows") or {}
    filter_time_windows = rankings.get("filter_time_windows") or {}
    overall = rankings.get("overall") or {}
    account = summary.get("account") or {}
    collateral = summary.get("collateral") or {}
    smoke = summary.get("smoke") or {}
    key = summary_key(summary)
    templates = summary.get("templates") or []
    return {
        "generated_at": summary.get("generated_at"),
        "execute_mode": bool(summary.get("execute_mode")),
        "mailbox_updated": bool(summary.get("mailbox_updated", False)),
        "smoke_ok": smoke.get("ok") if smoke else None,
        "smoke_version": smoke.get("version") if smoke else None,
        "gas_tier": summary.get("gas_tier"),
        "strk_balance": account.get("strk_balance_strk"),
        "xp_balance": collateral.get("balance_xp"),
        "markets": summary.get("markets"),
        "positions_count": summary.get("positions_count"),
        "overall": {
            "rank": overall.get("rank"),
            "pnl": overall.get("pnl"),
            "gap_to_first": overall.get("gap_to_first"),
            "top_pnl": overall.get("top_pnl"),
            "top_trader": overall.get("top_trader"),
            "markets_traded": overall.get("markets_traded"),
            "total_trades": overall.get("total_trades"),
        },
        "healthy_views": {
            "overall": bool(overall),
            "filters": sorted(slug for slug, item in filters.items() if item.get("healthy")),
            "time_windows": sorted(slug for slug, item in time_windows.items() if item.get("healthy")),
            "filter_time_windows": sorted(slug for slug, item in filter_time_windows.items() if item.get("healthy")),
        },
        "unhealthy_views": {
            "filters": key["unhealthy_filters"],
            "time_windows": key["unhealthy_time_windows"],
            "filter_time_windows": key["unhealthy_filter_time_windows"],
        },
        "mirrored_views": {
            "filters": key["mirrored_filters"],
            "time_windows": key["mirrored_time_windows"],
            "filter_time_windows": key["mirrored_filter_time_windows"],
        },
        "healthy_view_stats": {
            "filters": ranking_view_stats(filters),
            "time_windows": ranking_view_stats(time_windows),
            "filter_time_windows": ranking_view_stats(filter_time_windows),
        },
        "processed_candidates": summary.get("processed_candidates") or [],
        "templates": templates,
        "next_template_window": next_template_window(templates),
        "next_durable_template_window": next_template_window(
            templates,
            queueable_opportunities_only=True,
        ),
        "post_result_evidence_due": post_result_evidence_due(templates),
        "template_promotion": summary.get("template_promotion"),
        "active_portfolio_scout": summary.get("active_portfolio_scout"),
        "active_portfolio_scout_refresh": summary.get("active_portfolio_scout_refresh"),
    }


def public_loop_summary(summary: dict[str, Any]) -> dict[str, Any]:
    return compact_last_summary(summary)


def append_mailbox_if_changed(mailbox: Path, state: dict[str, Any], summary: dict[str, Any]) -> bool:
    key = summary_key(summary)
    if state.get("last_mailbox_key") == key:
        return False
    if mailbox_keys_equivalent(state.get("last_mailbox_key"), key):
        state["last_mailbox_key"] = key
        return False
    overall = summary["rankings"]["overall"]
    processed = summary.get("processed_candidates") or []
    due_templates = post_result_evidence_due(summary.get("templates") or [])
    gas = summary.get("account", {}).get("strk_balance_strk")
    xp = summary.get("collateral", {}).get("balance_xp")
    lines = [
        "",
        f"## Codex_Storm Deadeye — {utc_now()}",
        "",
        "Status: LOOP_UPDATE",
        f"Worktree: `{REPO_ROOT}`",
        "Branch: `main`",
        "Candidate: Storm Deadeye leaderboard loop tick.",
        "What changed: Ran the monitor loop with the current local guardrails.",
        (
            "Result: "
            f"overall rank={overall.get('rank')}, pnl={overall.get('pnl'):.6f}, "
            f"gap_to_first={overall.get('gap_to_first'):.6f}, active_markets="
            f"{summary['markets']['active_tradeable']}, gas={gas:.6f} STRK, xp={xp:.4f}."
        ),
        f"Healthy leaderboard views: {format_healthy_view_stats(key)}.",
        f"Active portfolio scout: {format_active_portfolio_scout(summary)}.",
        f"Active portfolio scout refresh: {format_active_portfolio_scout_refresh(summary)}.",
        f"Mirrored leaderboard views ignored: {format_mirrored_views(key)}.",
        f"Failures: unhealthy_ranking_views={format_unhealthy_views(key)}.",
        (
            "RCI pass: RCI_Storm pass by Codex_Storm Deadeye. The loop only "
            "records a mailbox entry when rank, health, gas tier, template "
            "evidence readiness, or candidate processing changes."
        ),
        (
            "Sceptic pass: Sceptic_Storm pass by Codex_Storm Deadeye. No "
            "LP/admin/deploy/grant/approval-only/settlement/pause/unpause/"
            "runtime deploy path is enabled by this runner."
        ),
        f"Next gate: {'review processed candidates and post strategy result' if processed else 'continue monitoring and queue evidence-backed candidates'}.",
        "Reviewed-by:",
        "Message to other agent: Claude_Storm, review any new candidate/script changes in your separate worktree.",
    ]
    if processed:
        lines.insert(9, "Processed candidates: " + json.dumps(processed, sort_keys=True))
    if due_templates:
        due = [
            {
                "id": item.get("id"),
                "label": item.get("label"),
                "opportunity_status": item.get("opportunity_status"),
                "result_not_before_utc": item.get("result_not_before_utc"),
            }
            for item in due_templates
        ]
        lines.insert(9, "Post-result evidence due: " + json.dumps(due, sort_keys=True))
    with mailbox.open("a", encoding="utf-8") as fh:
        fh.write("\n".join(lines))
        fh.write("\n")
    state["last_mailbox_key"] = key
    return True


def clear_last_error_after_success(state: dict[str, Any]) -> None:
    state.pop("last_error", None)


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run one Storm Deadeye leaderboard-loop tick.")
    parser.add_argument("--execute", action="store_true", help="Submit queued trades that pass every guard.")
    parser.add_argument("--run-smoke", action="store_true", help="Run the read-only smoke gate before the loop.")
    parser.add_argument("--mailbox", action="store_true", help="Append a mailbox update when status changes.")
    parser.add_argument("--state", type=Path, default=DEFAULT_STATE)
    parser.add_argument("--events", type=Path, default=DEFAULT_EVENTS)
    parser.add_argument("--candidates", type=Path, default=DEFAULT_CANDIDATES)
    parser.add_argument("--trade-journal", type=Path, default=DEFAULT_TRADE_JOURNAL)
    parser.add_argument("--mailbox-path", type=Path, default=DEFAULT_MAILBOX)
    parser.add_argument("--smoke-script", type=Path, default=DEFAULT_SMOKE_SCRIPT)
    parser.add_argument("--smoke-market", default=DEFAULT_SMOKE_MARKET)
    parser.add_argument("--templates", type=Path, default=DEFAULT_TEMPLATES)
    parser.add_argument(
        "--promote-ready-templates",
        action="store_true",
        help="Append queue-ready templates to the local candidate queue before processing candidates.",
    )
    parser.add_argument(
        "--refresh-active-portfolio-scout",
        action="store_true",
        help="Refresh the read-only active-portfolio quote scout when the saved artifact is stale.",
    )
    parser.add_argument(
        "--active-portfolio-scout-max-age-minutes",
        type=float,
        default=DEFAULT_ACTIVE_PORTFOLIO_SCOUT_MAX_AGE_MINUTES,
        help="Maximum age for the saved active-portfolio scout before quote-only refresh.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_arg_parser()
    args = parser.parse_args(argv)
    ensure_dirs(args.state.parent, args.events.parent, args.candidates.parent, args.trade_journal.parent)
    state = load_json(args.state, {})
    events: list[dict[str, Any]] = []
    try:
        smoke_result = None
        if args.run_smoke:
            smoke_result = run_smoke(args.smoke_script, args.smoke_market)
            state["last_smoke"] = smoke_result
        scout_refresh = None
        templates = load_template_status(args.templates)
        due_templates = post_result_evidence_due(templates)
        snap = account_snapshot()
        account = snap["account"]
        collateral = snap["collateral"]
        trader = account.get("address") or collateral.get("account")
        indexer_url = account.get("indexer_url")
        if not trader or not indexer_url:
            raise LoopError("account show did not return trader and indexer URL")
        monitoring = monitor(indexer_url, trader)
        if args.refresh_active_portfolio_scout:
            watched_template_market_state_reads = fetch_watched_template_market_states(templates)
            watched_template_market_states = watched_template_market_state_reads["markets"]
            if not watched_template_market_states and watched_template_market_state_reads.get("failures"):
                watched_template_market_states = None
            scout_refresh = maybe_refresh_active_portfolio_scout(
                state,
                args.state.parent,
                max_age_minutes=args.active_portfolio_scout_max_age_minutes,
                due_templates=due_templates,
                markets=monitoring["markets"]["items"],
                watched_template_market_states=watched_template_market_states,
            )
            if watched_template_market_state_reads.get("failures"):
                scout_refresh["watched_template_market_state_failures"] = watched_template_market_state_reads["failures"]
        template_promotion = None
        if args.promote_ready_templates:
            template_promotion = promote_ready_templates(args.templates, args.candidates, append=True)
        candidates = load_candidates(args.candidates)
        processed = process_candidates(
            candidates,
            monitoring["markets"]["items"],
            monitoring["positions"],
            account,
            collateral,
            monitoring["stats"],
            state,
            args,
            args.events,
        )
        summary = {
            "generated_at": utc_now(),
            "execute_mode": bool(args.execute),
            "smoke": smoke_result,
            "account": account,
            "collateral": collateral,
            "gas_tier": gas_tier(float(account.get("strk_balance_strk", 0.0))),
            "campaign": state.get("campaign", {}),
            "markets": {k: v for k, v in monitoring["markets"].items() if k != "items"},
            "rankings": monitoring["rankings"],
            "stats": monitoring["stats"],
            "positions_count": len(monitoring["positions"]),
            "processed_candidates": processed,
            "template_promotion": template_promotion,
            "templates": templates,
            "active_portfolio_scout": latest_active_portfolio_scout(args.state.parent),
            "active_portfolio_scout_refresh": scout_refresh,
            "state_dir": str(args.state.parent),
            "candidate_file": str(args.candidates),
            "events_file": str(args.events),
            "trade_journal": str(args.trade_journal),
        }
        append_jsonl(args.events, {"type": "loop.tick", "timestamp": now_ts(), "summary": summary_key(summary)})
        clear_last_error_after_success(state)
        if args.mailbox:
            summary["mailbox_updated"] = append_mailbox_if_changed(args.mailbox_path, state, summary)
        public_summary = public_loop_summary(summary)
        state["last_summary"] = public_summary
        save_json(args.state, state)
        print(json.dumps(public_summary, indent=2, sort_keys=True))
        return 0
    except Exception as exc:  # noqa: BLE001 - operator summary must capture any failure.
        event = {"type": "loop.error", "timestamp": now_ts(), "error": str(exc)}
        append_jsonl(args.events, event)
        state["last_error"] = event
        save_json(args.state, state)
        print(json.dumps({"ok": False, "error": str(exc)}, indent=2, sort_keys=True), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
