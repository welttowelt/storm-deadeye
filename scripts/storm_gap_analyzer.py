#!/usr/bin/env python3
"""Read-only leaderboard gap-impact analyzer for Storm Deadeye.

The executor gates standalone trade EV. This companion answers a different
question before strategy review: if a quoted market move landed, how would the
current indexer leaderboard mark change for us and the current leaders?

This script never submits transactions. It only calls indexer read endpoints
and `deadeye trade quote`.
"""

from __future__ import annotations

import argparse
import json
import math
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import storm_deadeye_loop as loop


DEFAULT_INDEXER_LIMIT = 100
DEFAULT_BUDGET = 100.0
DEFAULT_BANKROLL = 19832.0

WORLD_CUP_POD_20260612 = [
    {"label": "France lower/wider", "title_contains": "France", "belief": 3.3346, "belief_sigma": 0.2787},
    {"label": "Spain lower/wider", "title_contains": "Spain", "belief": 3.3463, "belief_sigma": 0.2776},
    {"label": "Belgium higher", "title_contains": "Belgium", "belief": 3.2215, "belief_sigma": 0.2576},
    {"label": "Argentina lower/wider", "title_contains": "Argentina", "belief": 3.2861, "belief_sigma": 0.2663},
    {"label": "Portugal lower/wider", "title_contains": "Portugal", "belief": 3.2956, "belief_sigma": 0.2745},
    {"label": "Germany higher", "title_contains": "Germany", "belief": 3.2291, "belief_sigma": 0.2702},
    {"label": "England lower/wider", "title_contains": "England", "belief": 3.3153, "belief_sigma": 0.2672},
    {"label": "Morocco higher/wider", "title_contains": "Morocco", "belief": 3.0790, "belief_sigma": 0.2687},
    {"label": "Brazil wider", "title_contains": "Brazil", "belief": 3.2765, "belief_sigma": 0.2756},
    {"label": "Netherlands higher/wider", "title_contains": "Netherlands", "belief": 3.1939, "belief_sigma": 0.2909},
]

CPI_NOWCAST_20260612 = [
    {
        "label": "CPI June Cleveland nowcast wider",
        "title_contains": "US Inflation in June 2026",
        "family": "normal",
        "belief": 4.05,
        "belief_sigma": 0.24,
    },
]

PRESETS = {
    "world-cup-pod-20260612": WORLD_CUP_POD_20260612,
    "cpi-nowcast-20260612": CPI_NOWCAST_20260612,
}


@dataclass
class Probe:
    label: str
    market: str
    family: str
    belief: float
    belief_sigma: float
    budget: float


def curve_lambda(sigma: float, k: float) -> float:
    if sigma <= 0.0 or k <= 0.0:
        return 0.0
    return k * math.sqrt(2.0 * sigma * math.sqrt(math.pi))


def normal_pdf(x: float, mean: float, sigma: float) -> float:
    if sigma <= 0.0:
        return 0.0
    z = (x - mean) / sigma
    return math.exp(-0.5 * z * z) / (sigma * math.sqrt(2.0 * math.pi))


def curve_score(x: float, mean: float, sigma: float, k: float) -> float:
    return curve_lambda(sigma, k) * normal_pdf(x, mean, sigma)


def row_spent(row: dict[str, Any]) -> float:
    expected_value = row.get("expectedValue")
    unrealized = row.get("unrealizedPnl")
    if expected_value is not None and unrealized is not None:
        return float(expected_value) - float(unrealized)
    return 0.0


def lognormal_row_pnl_at(row: dict[str, Any], live_mean: float, effective_k: float) -> float | None:
    required = ("mean", "sigma", "effectiveMean", "effectiveSigma")
    if any(row.get(key) is None for key in required):
        return None
    curve_delta = curve_score(
        live_mean,
        float(row["effectiveMean"]),
        float(row["effectiveSigma"]),
        effective_k,
    ) - curve_score(live_mean, float(row["mean"]), float(row["sigma"]), effective_k)
    spent = row_spent(row)
    gross = max(0.0, spent + curve_delta)
    return gross - spent


def new_lot_display_pnl(
    market_state: dict[str, Any],
    quote: dict[str, Any],
    spend_xp: float,
) -> float:
    effective_k = float(market_state.get("effectiveK") or market_state.get("k") or 0.0)
    post_mean = float(quote["candidate_mean"])
    curve_delta = curve_score(
        post_mean,
        float(market_state["mean"]),
        float(market_state["sigma"]),
        effective_k,
    ) - curve_score(post_mean, float(quote["candidate_mean"]), float(quote["candidate_sigma"]), effective_k)
    gross = max(0.0, spend_xp + curve_delta)
    return gross - spend_xp


def find_market(markets: list[dict[str, Any]], probe: dict[str, Any]) -> dict[str, Any]:
    if probe.get("market"):
        target = loop.canonical_address(str(probe["market"]))
        for market in markets:
            if loop.canonical_address(market.get("address")) == target:
                return market
        raise loop.LoopError(f"probe market not found: {probe['market']}")
    title_contains = str(probe.get("title_contains") or "").lower()
    for market in markets:
        if title_contains and title_contains in str(market.get("title") or "").lower():
            return market
    raise loop.LoopError(f"probe title not found: {probe.get('title_contains')}")


def load_probe_specs(path: Path | None, preset: str | None) -> list[dict[str, Any]]:
    specs: list[dict[str, Any]] = []
    if preset and preset != "none":
        specs.extend(PRESETS[preset])
    if path:
        with path.open("r", encoding="utf-8") as fh:
            for line_no, line in enumerate(fh, 1):
                stripped = line.strip()
                if not stripped or stripped.startswith("#"):
                    continue
                try:
                    item = json.loads(stripped)
                except json.JSONDecodeError as exc:
                    raise loop.LoopError(f"probe file line {line_no} invalid JSON: {exc}") from exc
                if not isinstance(item, dict):
                    raise loop.LoopError(f"probe file line {line_no} is not an object")
                specs.append(item)
    return specs


def build_probes(markets: list[dict[str, Any]], specs: list[dict[str, Any]], default_budget: float) -> list[Probe]:
    probes: list[Probe] = []
    for spec in specs:
        market = find_market(markets, spec)
        family = str(spec.get("family") or market.get("marketType") or "").lower()
        if family not in {"normal", "lognormal"}:
            continue
        probes.append(
            Probe(
                label=str(spec.get("label") or market.get("title") or market["address"]),
                market=str(market["address"]),
                family=family,
                belief=float(spec["belief"]),
                belief_sigma=float(spec["belief_sigma"]),
                budget=float(spec.get("budget", default_budget)),
            )
        )
    return probes


def quote_probe(probe: Probe, bankroll: float) -> dict[str, Any]:
    return loop.deadeye_json(
        [
            "trade",
            "quote",
            probe.market,
            "--family",
            probe.family,
            "--belief",
            f"{probe.belief:g}",
            "--belief-sigma",
            f"{probe.belief_sigma:g}",
            "--budget",
            f"{probe.budget:g}",
            "--bankroll",
            f"{bankroll:g}",
            "--risk",
            "aggressive",
        ],
        timeout=90,
    )


def fetch_indexer(base_url: str, path: str) -> Any:
    code, payload = loop.http_get_json(base_url, path)
    if code != 200:
        raise loop.LoopError(f"{path} returned HTTP {code}")
    return payload


def position_value_at(market: str, family: str, trader: str, at: float) -> float | None:
    try:
        payload = loop.deadeye_json(
            [
                "position",
                "value",
                market,
                "--family",
                family,
                "--trader",
                trader,
                "--at",
                f"{at:.12g}",
            ],
            timeout=60,
        )
    except Exception:
        return None
    value = payload.get("total_position_value")
    return None if value is None else float(value)


def position_expected_under_belief(
    market: str,
    family: str,
    trader: str,
    belief: float,
    belief_sigma: float,
) -> float | None:
    try:
        payload = loop.deadeye_json(
            [
                "position",
                "value",
                market,
                "--family",
                family,
                "--trader",
                trader,
                "--belief",
                f"{belief:.12g}",
                "--belief-sigma",
                f"{belief_sigma:.12g}",
            ],
            timeout=90,
        )
    except Exception:
        return None
    value = payload.get("expected_pnl")
    return None if value is None else float(value)


def existing_row_pnl_at(
    row: dict[str, Any],
    probe: Probe,
    live_mean: float,
    effective_k: float,
) -> float | None:
    if probe.family == "lognormal":
        return lognormal_row_pnl_at(row, live_mean, effective_k)
    if probe.family == "normal":
        trader = str(row.get("trader") or "")
        if not trader:
            return None
        return position_value_at(probe.market, probe.family, trader, live_mean)
    return None


def rank_summary(totals: dict[str, float], own: str) -> dict[str, Any]:
    sorted_rows = sorted(totals.items(), key=lambda item: item[1], reverse=True)
    top = sorted_rows[0] if sorted_rows else ("", 0.0)
    own_pnl = totals.get(own, 0.0)
    return {
        "rank": next((idx for idx, item in enumerate(sorted_rows, 1) if item[0] == own), None),
        "gap": 0.0 if top[0] == own else max(0.0, top[1] - own_pnl),
        "own_pnl": own_pnl,
        "top_trader": top[0],
        "top_pnl": top[1],
    }


def analyze_probe(
    probe: Probe,
    market: dict[str, Any],
    markets: list[dict[str, Any]],
    rankings: list[dict[str, Any]],
    traders: list[dict[str, Any]],
    own_positions: list[dict[str, Any]],
    own_trader: str,
    quote: dict[str, Any],
) -> dict[str, Any]:
    state = market.get("state") or {}
    effective_k = float(state.get("effectiveK") or state.get("k") or 0.0)
    current_live_mean = float(state["mean"])
    post_live_mean = float(quote["candidate_mean"])
    current_totals = {
        loop.canonical_address(row.get("trader")): float(row.get("totalPnl", 0.0))
        for row in rankings
    }
    predicted = dict(current_totals)
    belief_predicted = dict(current_totals)
    trader_deltas: list[dict[str, Any]] = []
    belief_deltas: list[dict[str, Any]] = []
    for row in traders:
        if not row.get("hasPosition"):
            continue
        trader = loop.canonical_address(row.get("trader"))
        current_actual = row.get("unrealizedPnl")
        predicted_pnl = existing_row_pnl_at(row, probe, post_live_mean, effective_k)
        current_model = existing_row_pnl_at(row, probe, current_live_mean, effective_k)
        if current_actual is None or predicted_pnl is None:
            continue
        delta = predicted_pnl - float(current_actual)
        predicted[trader] = predicted.get(trader, 0.0) + delta
        trader_deltas.append(
            {
                "trader": trader,
                "current_indexer_pnl": float(current_actual),
                "current_model_pnl": current_model,
                "predicted_pnl": predicted_pnl,
                "delta": delta,
            }
        )
        belief_pnl = position_expected_under_belief(
            probe.market,
            probe.family,
            str(row.get("trader")),
            probe.belief,
            probe.belief_sigma,
        )
        if belief_pnl is not None:
            belief_delta = belief_pnl - float(current_actual)
            belief_predicted[trader] = belief_predicted.get(trader, 0.0) + belief_delta
            belief_deltas.append(
                {
                    "trader": trader,
                    "current_indexer_pnl": float(current_actual),
                    "belief_expected_pnl": belief_pnl,
                    "delta": belief_delta,
                }
            )

    spend_xp = float(quote.get("padded_collateral") or quote.get("required_collateral") or probe.budget)
    own_new_lot_pnl = new_lot_display_pnl(state, quote, spend_xp)
    own = loop.canonical_address(own_trader)
    predicted[own] = predicted.get(own, 0.0) + own_new_lot_pnl
    own_new_lot_ev = float(quote.get("expected_value") or 0.0)
    belief_predicted[own] = belief_predicted.get(own, 0.0) + own_new_lot_ev

    before = rank_summary(current_totals, own)
    after = rank_summary(predicted, own)
    belief_after = rank_summary(belief_predicted, own)

    top_trader = before["top_trader"]
    top_delta = predicted.get(top_trader, before["top_pnl"]) - before["top_pnl"]
    belief_top_delta = belief_predicted.get(top_trader, before["top_pnl"]) - before["top_pnl"]
    runner_blockers = runner_blockers_for_probe(probe, market, markets, own_positions, quote)
    return {
        "label": probe.label,
        "market": probe.market,
        "market_title": market.get("title"),
        "family": probe.family,
        "belief": probe.belief,
        "belief_sigma": probe.belief_sigma,
        "quote": {
            "on_chain_will_accept": quote.get("on_chain_will_accept"),
            "expected_value": quote.get("expected_value"),
            "required_collateral": quote.get("required_collateral"),
            "candidate_mean": quote.get("candidate_mean"),
            "candidate_sigma": quote.get("candidate_sigma"),
        },
        "scoreboard": {
            "before_rank": before["rank"],
            "after_rank": after["rank"],
            "before_gap": before["gap"],
            "after_gap": after["gap"],
            "gap_improvement": before["gap"] - after["gap"],
            "own_before_pnl": before["own_pnl"],
            "own_new_lot_pnl": own_new_lot_pnl,
            "own_after_pnl": after["own_pnl"],
            "top_trader": top_trader,
            "top_delta": top_delta,
            "after_top_trader": after["top_trader"],
            "after_top_pnl": after["top_pnl"],
        },
        "belief_scoreboard": {
            "after_rank": belief_after["rank"],
            "before_gap": before["gap"],
            "after_gap": belief_after["gap"],
            "gap_improvement": before["gap"] - belief_after["gap"],
            "own_new_lot_ev": own_new_lot_ev,
            "own_after_pnl": belief_after["own_pnl"],
            "top_trader": top_trader,
            "top_delta": belief_top_delta,
            "after_top_trader": belief_after["top_trader"],
            "after_top_pnl": belief_after["top_pnl"],
            "trader_deltas": sorted(belief_deltas, key=lambda item: abs(float(item["delta"])), reverse=True)[:8],
        },
        "runner_gate": {
            "would_pass_current_runner": not runner_blockers,
            "blockers": runner_blockers,
        },
        "model": {
            "type": (
                "lognormal_indexer_curve_floor"
                if probe.family == "lognormal"
                else "normal_position_value_existing_curve_estimated_new_lot"
            ),
            "current_live_mean": current_live_mean,
            "post_live_mean": post_live_mean,
            "effective_k": effective_k,
            "trader_deltas": sorted(trader_deltas, key=lambda item: abs(float(item["delta"])), reverse=True)[:8],
        },
    }


def runner_blockers_for_probe(
    probe: Probe,
    market: dict[str, Any],
    markets: list[dict[str, Any]],
    own_positions: list[dict[str, Any]],
    quote: dict[str, Any],
) -> list[str]:
    blockers: list[str] = []
    if not quote.get("on_chain_will_accept"):
        blockers.append(f"quote rejected: {quote.get('rejection')}")
    expected_value = float(quote.get("expected_value") or 0.0)
    if expected_value < loop.MIN_EXECUTE_EV:
        blockers.append(f"standalone EV {expected_value:.6f} below {loop.MIN_EXECUTE_EV:g} XP floor")
    candidate = {
        "id": probe.label,
        "market": probe.market,
        "family": probe.family,
    }
    blockers.extend(loop.concentration_errors(candidate, market, own_positions, markets))
    title = str(market.get("title") or "").lower()
    if "world cup" in title:
        blockers.append("World Cup probe has no post-result evidence marker")
    return blockers


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Read-only Storm Deadeye leaderboard gap analyzer.")
    parser.add_argument("--preset", choices=[*PRESETS.keys(), "none"], default="world-cup-pod-20260612")
    parser.add_argument("--probes", type=Path, help="Optional JSONL probe file.")
    parser.add_argument("--indexer-url", help="Override Deadeye indexer URL.")
    parser.add_argument("--trader", help="Override own trader address.")
    parser.add_argument("--budget", type=float, default=DEFAULT_BUDGET)
    parser.add_argument("--bankroll", type=float, default=DEFAULT_BANKROLL)
    parser.add_argument("--limit", type=int, default=DEFAULT_INDEXER_LIMIT)
    parser.add_argument("--output", type=Path, help="Optional JSON output path.")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    snap = loop.account_snapshot()
    account = snap["account"]
    indexer_url = args.indexer_url or account.get("indexer_url")
    trader = args.trader or account.get("address")
    if not indexer_url or not trader:
        raise loop.LoopError("missing indexer URL or trader")
    markets = fetch_indexer(indexer_url, "/api/markets")
    rankings = fetch_indexer(indexer_url, f"/api/rankings?limit={int(args.limit)}")
    own_positions = fetch_indexer(indexer_url, f"/api/positions/{loop.canonical_address(trader)}")
    specs = load_probe_specs(args.probes, args.preset)
    probes = build_probes(markets, specs, args.budget)
    by_address = {loop.canonical_address(market.get("address")): market for market in markets}
    results = []
    for probe in probes:
        market = by_address[loop.canonical_address(probe.market)]
        quote = quote_probe(probe, args.bankroll)
        if not quote.get("on_chain_will_accept"):
            results.append({"label": probe.label, "market": probe.market, "quote": quote, "error": "quote rejected"})
            continue
        traders = fetch_indexer(indexer_url, f"/api/markets/{probe.market}/traders")
        results.append(analyze_probe(probe, market, markets, rankings, traders, own_positions, trader, quote))
    results.sort(
        key=lambda item: (
            float(item.get("scoreboard", {}).get("gap_improvement") or -1e9),
            float(item.get("quote", {}).get("expected_value") or -1e9),
        ),
        reverse=True,
    )
    payload = {
        "generated_at": loop.utc_now(),
        "trader": loop.canonical_address(trader),
        "indexer_url": indexer_url,
        "preset": args.preset,
        "results": results,
    }
    text = json.dumps(payload, indent=2, sort_keys=True)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
