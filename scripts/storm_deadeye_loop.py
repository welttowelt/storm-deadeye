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


def run_cmd(args: list[str], *, timeout: int = 60, check: bool = True) -> CmdResult:
    env = os.environ.copy()
    env["PATH"] = f"{Path.home() / '.local' / 'bin'}:{env.get('PATH', '')}"
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
    if check and proc.returncode != 0:
        joined = " ".join(args)
        tail = (proc.stderr or proc.stdout).strip().splitlines()[-3:]
        raise LoopError(f"{joined} failed rc={proc.returncode}: {' | '.join(tail)}")
    return result


def deadeye_json(args: list[str], *, timeout: int = 60) -> Any:
    result = run_cmd(["deadeye", *args, "--output", "json"], timeout=timeout)
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


def account_snapshot() -> dict[str, Any]:
    account = deadeye_json(["account", "show"], timeout=45)
    collateral = deadeye_json(["collateral", "balance"], timeout=45)
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
        category = market.get("category")
        if category:
            slug = slugify(str(category))
            if slug:
                slugs.add(slug)
        for topic in market.get("topics") or []:
            slug = slugify(str(topic))
            if slug:
                slugs.add(slug)
    return sorted(slugs)


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
    if "world cup" in title and len(evidence) < 2:
        errors.append("World Cup candidate needs at least two evidence items")
    return errors


def run_smoke(smoke_script: Path, smoke_market: str) -> dict[str, Any]:
    started = utc_now()
    if smoke_script.exists():
        result = run_cmd(["zsh", str(smoke_script), smoke_market], timeout=90)
        return {"ok": True, "started_at": started, "script": str(smoke_script), "summary": result.stdout.strip().splitlines()[-1:]}
    version = run_cmd(["deadeye", "--version"], timeout=15)
    run_cmd(["deadeye", "markets", "list", "--limit", "3", "--output", "plain"], timeout=45)
    deadeye_json(["markets", "show", smoke_market], timeout=45)
    doctor = deadeye_json(["doctor", "--market", smoke_market], timeout=60)
    if not doctor.get("all_ok"):
        raise LoopError("built-in smoke doctor was not all_ok")
    return {"ok": True, "started_at": started, "script": "built-in", "version": version.stdout.strip()}


def monitor(indexer_url: str, trader: str) -> dict[str, Any]:
    status_health, health = http_get_json(indexer_url, "/health")
    status_markets, markets = http_get_json(indexer_url, "/api/markets")
    if status_markets != 200 or not isinstance(markets, list):
        raise LoopError(f"indexer markets unavailable status={status_markets}")
    active_markets = [market for market in markets if market_is_tradeable(market)]
    status_rankings, rankings = http_get_json(indexer_url, "/api/rankings?limit=100")
    if status_rankings != 200 or not isinstance(rankings, list):
        raise LoopError(f"indexer rankings unavailable status={status_rankings}")
    status_stats, stats = http_get_json(indexer_url, f"/api/positions/{canonical_address(trader)}/stats")
    if status_stats != 200 or not isinstance(stats, dict):
        stats = {}
    status_positions, positions = http_get_json(indexer_url, f"/api/positions/{canonical_address(trader)}")
    if status_positions != 200 or not isinstance(positions, list):
        positions = []
    own_pnl = float(stats.get("totalPnl", 0.0)) if stats else 0.0
    overall = compute_rank(rankings, trader, own_pnl)
    filter_status: dict[str, Any] = {}
    for slug in discover_filter_slugs(markets):
        code, payload = http_get_json(indexer_url, f"/api/rankings?limit=100&domain={urllib.parse.quote(slug)}")
        if code == 200 and isinstance(payload, list):
            filter_status[slug] = {"healthy": True, **compute_rank(payload, trader, own_pnl)}
        else:
            error = payload.get("error") if isinstance(payload, dict) else str(payload)[:120]
            filter_status[slug] = {"healthy": False, "status": code, "error": error}
    return {
        "health": {"status": status_health, "payload": health},
        "markets": {"total": len(markets), "active_tradeable": len(active_markets), "items": markets},
        "rankings": {"overall": overall, "filters": filter_status},
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
        if errors:
            processed.append({"id": candidate_id, "status": "failed", "reason": "; ".join(errors)})
            processed_ids.add(candidate_id)
            continue
        if not market_is_tradeable(market_meta):
            processed.append({"id": candidate_id, "status": "failed", "reason": "market not tradeable"})
            processed_ids.add(candidate_id)
            continue
        try:
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


def summary_key(summary: dict[str, Any]) -> dict[str, Any]:
    filters = summary.get("rankings", {}).get("filters", {})
    unhealthy = sorted(slug for slug, item in filters.items() if not item.get("healthy"))
    processed = [
        {"id": item.get("id"), "status": item.get("status")}
        for item in summary.get("processed_candidates", [])
        if item.get("status") not in {"skipped"}
    ]
    overall = summary.get("rankings", {}).get("overall", {})
    return {
        "rank": overall.get("rank"),
        "gap": round(float(overall.get("gap_to_first") or 0.0), 4),
        "pnl": round(float(overall.get("pnl") or 0.0), 4),
        "gas_tier": summary.get("gas_tier"),
        "unhealthy_filters": unhealthy,
        "processed": processed,
    }


def append_mailbox_if_changed(mailbox: Path, state: dict[str, Any], summary: dict[str, Any]) -> bool:
    key = summary_key(summary)
    if state.get("last_mailbox_key") == key:
        return False
    overall = summary["rankings"]["overall"]
    unhealthy = key["unhealthy_filters"]
    processed = summary.get("processed_candidates") or []
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
        f"Failures: unhealthy_filtered_boards={', '.join(unhealthy) if unhealthy else 'none'}.",
        (
            "RCI pass: RCI_Storm pass by Codex_Storm Deadeye. The loop only "
            "records a mailbox entry when rank, health, gas tier, or candidate "
            "processing changes."
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
    with mailbox.open("a", encoding="utf-8") as fh:
        fh.write("\n".join(lines))
        fh.write("\n")
    state["last_mailbox_key"] = key
    return True


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
        snap = account_snapshot()
        account = snap["account"]
        collateral = snap["collateral"]
        trader = account.get("address") or collateral.get("account")
        indexer_url = account.get("indexer_url")
        if not trader or not indexer_url:
            raise LoopError("account show did not return trader and indexer URL")
        monitoring = monitor(indexer_url, trader)
        candidates = load_candidates(args.candidates)
        processed = process_candidates(
            candidates,
            monitoring["markets"]["items"],
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
            "state_dir": str(args.state.parent),
            "candidate_file": str(args.candidates),
            "events_file": str(args.events),
            "trade_journal": str(args.trade_journal),
        }
        append_jsonl(args.events, {"type": "loop.tick", "timestamp": now_ts(), "summary": summary_key(summary)})
        if args.mailbox:
            summary["mailbox_updated"] = append_mailbox_if_changed(args.mailbox_path, state, summary)
        save_json(args.state, state)
        print(json.dumps(summary, indent=2, sort_keys=True))
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
