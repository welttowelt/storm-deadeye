# Storm Deadeye Leaderboard Loop

Goal: reach and hold rank #1 on every healthy Deadeye indexer leaderboard while
preserving a 25 STRK hard gas reserve, warning below 100 STRK, and stopping
live writes after a 1500 XP campaign loss or drawdown.

## Clear Goal

Storm Deadeye's goal is to become and stay rank #1 on every healthy Deadeye
leaderboard by converting evidence-backed mispricing into realized leaderboard
P&L, without risking the wallet's gas floor or spending the last 1000 XP.

The climb runs in this order:

1. Beat the overall board first. The target is live #1 P&L plus a buffer, not a
   rank tie. Every candidate states its expected contribution to the current
   gap.
2. Hold each healthy filtered board once the indexer exposes it. A filtered
   board returning HTTP 503 is unhealthy and monitored, not counted as empty.
3. Turn durable watchlist items into trades only when evidence, quote EV, and
   runner gates clear. World Cup pre-result pressure stays watch-only.
4. Add coverage trades only after P&L leadership is stable. Markets-traded or
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

`state.json` stores the latest smoke result, mailbox-change key, trade caps,
campaign guard, and a sanitized `last_summary` with rank, gap, gas tier, XP,
healthy/unhealthy ranking views, processed candidates, and disabled template
readiness. It omits RPC URLs, indexer URLs, raw token balances, smoke script
paths, journal paths, and wallet configuration bodies. Template readiness is
operator-only telemetry; a `queue_ready` template still has to be copied into
the candidate queue and pass fresh doctor, quote, dry-run, gas, XP,
concentration, and EV gates before any execute path.
Normal loop stdout uses the same sanitized public summary.
Templates whose stored quote EV is below their `min_expected_value` stay blocked
as `template_ev_below_floor` until a fresh analysis supports promotion.
Templates also need a durable promotion status, currently `runner_candidate` or
`durable_watch`; `weak_watch`, `paint_trap`, and `avoid` stay parked until a
fresh analysis upgrades them.
World Cup templates may set `result_not_before_utc`; before that timestamp the
promoter adds `template_result_window_not_reached`, even if post-result fields
are filled.
The sanitized `last_summary` also records `next_template_window`, the earliest
future template result window still blocking promotion, and
`next_durable_template_window`, the earliest blocked window among
`runner_candidate` or `durable_watch` templates that also clear stored EV and
durability blockers. An EV-blocked durable watch remains visible in
`templates`, but it is not shown as the next durable strike clock.
After a template's `result_not_before_utc` has passed, `last_summary` also
reports `post_result_evidence_due` when official result evidence is still
missing. That field is part of the mailbox-change key, so the hourly loop can
surface result-evidence work as soon as the configured result window opens.
Before the window, `last_summary.pre_window_evidence_readiness` reports whether
the local evidence packet is actually ready for final-whistle capture. A ready
packet stays quiet. For the next result window only, packet loss, unreadable
JSON, or `pre_window_readiness.ready_for_result_window=false` is treated as a
readiness regression and enters the mailbox-change key. Source reachability
also has a 24-hour freshness gate, so an old pre-window URL check is surfaced
before final whistle. Later template backlog does not create routine mailbox
noise. The loop automatically refreshes public source reachability for the next
pre-window packet when that check is missing or stale; this only updates the
local evidence packet and never captures post-result evidence, promotes a
template, queues a candidate, or touches on-chain state. Refresh failures are
mailbox-visible.

## Monitor Tick

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke
```

This runs the read-only smoke gate, fetches indexer health, markets, rankings,
own stats, positions, STRK balance, XP balance, and filtered rankings for
discovered domain/category/topic/tag slugs plus standard overall time windows
(`last-1h`, `last-24h`, `last-7d`). When a domain board is healthy, the runner
also checks domain+time-window views. Filtered boards returning HTTP 503 are
recorded as unhealthy rather than empty, and filtered rank/gap calculations use
matching filtered trader stats instead of overall wallet P&L.
Overall is probed every tick. Non-overall views that are healthy are also
probed every tick. Non-overall views that are unhealthy or mirror the overall
board are cached and re-probed on a one-hour cooldown, so the loop still detects
recovered distinct boards without spending every tick on fake or unavailable
views. The sanitized summary exposes `ranking_probe_cooldown` skipped counts;
cooldown skips do not create mailbox updates.

When the external read-only smoke script is present, the runner allows one
retry before failing the tick. External and built-in smoke must report
`deadeye >= 0.1.20`; stale or missing smoke versions fail closed before any
candidate can reach quote, dry-run, or submit.

To let the runner append a mailbox update only when rank, health, gas tier,
candidate processing, template readiness, scout signal counts, or scout-refresh
failures change:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox
```

Active-portfolio scout refresh timestamps and fresh/refreshed status are not
mailbox-change triggers on their own. The mailbox stays quiet when a routine
hourly refresh produces the same coverage, pass count, and top signal shape.
When active-portfolio scouting is enabled, the runner also records sanitized
market-state fingerprints and forces a full quote-only scout after a real
market-state shift. For World Cup markets, it performs explicit read-only
`deadeye markets show` checks across all active tradeable World Cup markets
discovered from the indexer list plus prepared post-result templates, so a
post-result repricing can trigger a scout even if filtered leaderboard views are
still mirrored or unavailable. A World Cup market-state read failure is treated
as a health regression for the mailbox-change key; routine fresh/refreshed scout
timestamps still stay quiet.

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
Placeholders do not count: evidence with `url: "TO_FILL"` or
`post_result: false` is explicitly blocked even if the source role is
`official_match_result`.

In execute mode, World Cup candidates also require a local Claude execute-review
marker before any confirmed submit. Without it, the runner still performs the
fresh doctor, quote, and exact `--dry-run` path, then records
`status=review_required` and stops before the non-dry-run submit. The approval
shape is:

```json
{"claude_review":{"reviewed_by":"Claude_Storm","status":"approved","approved_for_execute":true,"reviewed_at":"2026-06-14T20:15:00Z"}}
```

This marker does not bypass evidence, quote, dry-run, concentration, gas, XP,
trade-cap, or operator-policy gates.

When the marker is missing, the `review_required` event and processed-candidate
mailbox line include a sanitized `review_package`: candidate id/template id,
market, belief, budget, leaderboard context, evidence packet reference,
evidence-row summaries, selected quote fields, selected dry-run verdict fields,
and max collateral. The `leaderboard_context` block carries stored opportunity
status, stored quote EV, belief-gap impact, current blocker, and an explicit
flag that quote EV alone is not sufficient for review approval.
Raw dry-run output, calldata, wallet config, journal path, and secrets are not
included.

## Gap-Impact Screen

Use the read-only analyzer before proposing any rank-gap strategy:

```bash
python3 scripts/storm_gap_analyzer.py --preset world-cup-pod-20260612
```

For CPI/economics:

```bash
python3 scripts/storm_gap_analyzer.py --preset cpi-nowcast-20260612
```

For a cheap XP-ladder quote screen before expensive leaderboard valuation:

```bash
python3 scripts/storm_gap_analyzer.py --preset world-cup-pod-20260612 --budget 1000 --budget-ladder --quote-only --sort-by ev
```

The analyzer quotes probe beliefs and reports two views:

- `scoreboard`: estimated current mark-to-market display movement.
- `belief_scoreboard`: expected movement under the probe belief, using
  read-only position valuation for existing positions and quote EV for the new
  lot.
- `opportunity`: operator label for the probe:
  - `runner_candidate`: quote/concentration/preflight screen passes; draft a
    queued candidate only if the record has full evidence and budget. The
    execution runner still re-checks all live guards.
  - `durable_watch`: being right would close meaningful gap, but current gates
    still block execution.
  - `paint_trap`: display gap improves far more than belief gap; skip unless
    Oli separately approves a known snapshot sprint.
  - `weak_watch`: positive but too small for the current climb.
  - `avoid`: belief view does not help the rank gap.
  - `scout_error`: a read-only quote or indexer fetch failed for that probe;
    retry the scout and do not queue from that row.
  - `quote_screen`: quote-only ladder output; run full leaderboard valuation
    before queueing or executing.

The `runner_gate` section reports whether the probe would pass current runner
guards and lists blockers such as low standalone EV, concentration, or missing
World Cup post-result evidence. It is a review signal only. A positive
gap-improvement estimate does not bypass the execution runner's evidence,
quote, dry-run, 10 XP EV floor, concentration, gas, XP, or trade-cap guards.
By default results are sorted by `belief_gap`, so durable rank pressure is shown
before temporary display movement.
One flaky market read is recorded as a blocked `scout_error` row; the analyzer
continues ranking the rest of the preset.
With `--budget-ladder`, the analyzer expands each probe over the same XP rungs
used by the runner, capped by `--budget` and the 1000 XP reserve. With
`--quote-only`, it skips existing-position valuation and should be used as the
fast first pass; only EV-interesting rows should be sent through full valuation.

## Execution Mode

Review ready templates without touching the queue:

```bash
python3 scripts/storm_template_promoter.py
```

Append only templates that have no readiness blockers:

```bash
python3 scripts/storm_template_promoter.py --append
```

The promoter is local queue plumbing only. It does not call Deadeye or submit a
trade; the execution runner still re-checks all live guards. It also refuses
stale templates whose stored quote EV is below the configured minimum, even if
the post-result evidence fields have been filled. Weak-watch or paint-trap
templates are also refused until a fresh analyzer run upgrades their status.
Templates with `result_not_before_utc` cannot be promoted before that timestamp,
which keeps early fixture evidence from masquerading as a completed match.
For the next Germany result window, use
[`GERMANY_POST_RESULT_SNAP_PREP.md`](GERMANY_POST_RESULT_SNAP_PREP.md) as the
operator evidence checklist and do not queue from the draft template before the
official completed-match marker is captured.
To create a fillable local evidence packet from that disabled template:

```bash
python3 scripts/storm_worldcup_evidence_packet.py --template ~/.local/state/storm-deadeye/templates/germany-post-result-snap-template-20260612.json --output ~/.local/state/storm-deadeye/germany-post-result-evidence-packet.json
```

To let the loop perform that same local queue promotion before candidate
processing:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox --promote-ready-templates
```

With `--execute`, promoted candidates can be processed in the same tick, but
they still need fresh smoke, doctor, quote, dry-run, gas, XP, EV,
concentration, and trade-cap checks before any live submit.

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
python3 -m unittest scripts/test_storm_deadeye_loop.py scripts/test_storm_gap_analyzer.py scripts/test_storm_template_promoter.py
```

Live monitor smoke:

```bash
python3 scripts/storm_deadeye_loop.py --run-smoke
```
