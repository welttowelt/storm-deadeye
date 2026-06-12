# Storm Deadeye Leaderboard Loop

Goal: reach and hold rank #1 on every healthy Deadeye indexer leaderboard while
preserving a 25 STRK hard gas reserve, warning below 100 STRK, and stopping
live writes after a 1500 XP campaign loss or drawdown.

## Number-One Goal

Storm Deadeye wins in three phases:

1. Beat the overall board first. Current target is the live #1 P&L plus a
   buffer, not just a rank tie. Every candidate should state its expected
   contribution to the current gap.
2. Hold every healthy filtered board once the indexer exposes it. A filtered
   board returning HTTP 503 is unhealthy and monitored, not counted as empty.
3. Add coverage trades only after P&L leadership is stable. Markets-traded or
   total-trades coverage must stay positive-EV and obey the same evidence gate.

Live execution is only for Storm-vetted leaderboard candidates. The runner does
not invent forecasts; pods or agents queue candidates with sources, rationale,
belief, sigma, budget, and minimum EV.

The loop is implemented by `scripts/storm_deadeye_loop.py`. It is an operator
runner, not a forecasting oracle: it monitors leaderboards every tick and can
execute only queued, evidence-backed leaderboard trades that pass the local
guardrails.

## Local Files

Default local state lives outside the repo:

- State: `~/.local/state/storm-deadeye/state.json`
- Events: `~/.local/state/storm-deadeye/events.jsonl`
- Candidate queue: `~/.local/state/storm-deadeye/candidates.jsonl`
- Trade journal: `~/.local/share/storm-deadeye/trade-journal.jsonl`

These files can contain public trading telemetry and should not be committed.
The runner never prints wallet config bodies, `.env`, private keys, mnemonics,
or journal contents.

## Monitor Tick

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke
```

This runs the read-only smoke gate, fetches indexer health, markets, rankings,
own stats, positions, STRK balance, XP balance, and filtered rankings for
discovered category/topic slugs. Filtered boards returning HTTP 503 are recorded
as unhealthy rather than empty.

To let the runner append a mailbox update only when rank, health, gas tier, or
candidate processing changes:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox
```

## Candidate Queue

Candidates are JSONL entries in
`~/.local/state/storm-deadeye/candidates.jsonl`. Example:

```json
{"id":"cpi-2026-06-v1","market":"0x5f4203d658eb7f0aff83fa3aaaf5274d096258d6a906f3e119940474fe76ad4","family":"normal","belief":3.8,"belief_sigma":0.16,"budget":1000,"rationale":"Official CPI and nowcast evidence imply a cooler June YoY print than the current curve.","evidence":[{"claim":"BLS CPI release calendar and methodology define the resolving source","source_role":"official_measurement","source":"BLS CPI","url":"https://www.bls.gov/cpi/"},{"claim":"Nowcast input points lower than the market mean","source_role":"leading_indicator","source":"Cleveland Fed Inflation Nowcasting","url":"https://www.clevelandfed.org/indicators-and-data/inflation-nowcasting"}]}
```

The runner rejects candidates without a rationale, evidence, a supported family,
or category-specific evidence hints. Economics candidates need an official or
primary-source hint. World Cup candidates need at least two evidence items and
a post-result marker such as `world_cup_post_result: true`, evidence
`post_result: true`, `source_role: "official_match_result"`, or
`event_stage: "match_completed"`.

## Gap-Impact Screen

Use the read-only analyzer before proposing any rank-gap strategy:

```bash
python3 scripts/storm_gap_analyzer.py --preset world-cup-pod-20260612
```

For CPI/economics:

```bash
python3 scripts/storm_gap_analyzer.py --preset cpi-nowcast-20260612
```

The analyzer quotes probe beliefs and estimates how the indexer leaderboard
mark would move for current leaders and for a new Storm Deadeye lot. The
`runner_gate` section reports whether the probe would pass current runner
guards and lists blockers such as low standalone EV, concentration, or missing
World Cup post-result evidence. It is a review signal only. A positive
gap-improvement estimate does not bypass the execution runner's evidence,
quote, dry-run, 10 XP EV floor, concentration, gas, XP, or trade-cap guards.

## Execution Mode

Dry-run queued candidates:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox
```

Submit queued candidates that pass every guard:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox --execute
```

Autonomous execution is limited to leaderboard trades via:

- `deadeye trade quote`
- `deadeye trade execute --dry-run`
- capped `deadeye trade execute --confirm`

The runner does not call LP, admin, deploy, grant, approval-only, settlement,
pause, unpause, or runtime-deploy commands.

## Guardrails

- 25 STRK hard write stop.
- 100 STRK warning, 50 STRK strong warning.
- 1000 XP reserve.
- XP quote ladder: 100, 250, 500, 1000, 2000, 4000.
- Missing dry-run budgets default to the smallest rung, 100 XP.
- Live `--execute` requires an explicit candidate budget.
- Minimum expected value floor: 10 XP. Candidates may raise this, never lower it.
- Campaign loss halt: stop live writes after 1500 XP loss from campaign start or
  1500 XP drawdown from high-water P&L.
- Max 3 executed trades per 10-minute loop.
- Max 12 executed trades per hour.
- `doctor --market` must pass before each candidate.
- Quote must be accepted on-chain and clear the candidate EV threshold.
- Dry-run is attempted for the intended execute path before submission.
- `--max-collateral` is set to quoted collateral plus 5 percent.

## Tests

```bash
python3 -m unittest scripts/test_storm_deadeye_loop.py scripts/test_storm_gap_analyzer.py
```

Live monitor smoke:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke
```
