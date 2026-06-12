# Storm Deadeye Leaderboard Loop

Goal: reach and hold rank #1 on every healthy Deadeye indexer leaderboard while
preserving a 25 STRK hard gas reserve and warning below 100 STRK.

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
primary-source hint; World Cup candidates need at least two evidence items.

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
- Max 3 executed trades per 10-minute loop.
- Max 12 executed trades per hour.
- `doctor --market` must pass before each candidate.
- Quote must be accepted on-chain and clear the candidate EV threshold.
- Dry-run is attempted for the intended execute path before submission.
- `--max-collateral` is set to quoted collateral plus 5 percent.

## Tests

```bash
python3 -m unittest scripts/test_storm_deadeye_loop.py
```

Live monitor smoke:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke
```
