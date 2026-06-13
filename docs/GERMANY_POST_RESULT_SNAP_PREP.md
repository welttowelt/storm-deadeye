# Germany Post-Result Snap Prep

Storm Deadeye runbook for the first Germany World Cup result window.

## Current Template

- Template: `germany-post-result-snap-template-20260612`
- Status: `disabled`, `draft_only_not_queue_active`
- Market: `0x1e7b71e2e9f26c9f37d9419a8e542049194053eb9534455306518a98746f803`
- Family: `lognormal`
- Direction: Germany higher
- Budget seed: `100 XP`
- Minimum quote EV: `10 XP`
- Result window opens: `2026-06-14T20:00:00Z`
- Fixture source: `https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464`
- Current blocker: missing official post-result evidence

## Hard Gate

No pre-result World Cup queue, dry-run, or execution is allowed for this
candidate. The template stays disabled until all of these are true:

- FIFA or another official match page shows the Germany match as completed.
- Final score and final-whistle/completed marker are captured with source URL
  and capture timestamp.
- `world_cup_post_result` is set only after that official completed marker is
  present.
- At least two post-result evidence rows exist, including one
  `source_role: "official_match_result"` row whose URL is not `TO_FILL`.
- Fresh active-portfolio scout is run after the result lands.
- Fresh market state, doctor, quote, dry-run, gas, XP, concentration, trade-cap,
  and leaderboard gap gates all pass.
- Stored quote EV remains above `10 XP`, and the fresh scout upgrades the
  opportunity beyond `weak_watch` if leaderboard impact is still too small.

## Evidence Checklist

Capture these before promoting or queueing:

- Official result, `source_role: "official_match_result"`: final score,
  completed/final marker, source URL, capture UTC.
- Lineups, `source_role: "official_lineups"`: starting XI, substitutions,
  notable absences, source URL, capture UTC.
- Injuries/suspensions, `source_role: "team_news"`: pre-match and in-match
  changes that affect Germany's later tournament path.
- Odds move, `source_role: "odds_snapshot"`: at least one pre-result snapshot
  and one post-result snapshot for Germany outright odds or path odds.
- Ratings move, `source_role: "ratings_snapshot"`: model/ratings source before
  and after, if available.
- Market state, `source_role: "deadeye_market_state"`: `deadeye markets show
  <market> --output json` after result.
- Quote, `source_role: "deadeye_quote_scout"`: quote JSON after result using
  fresh belief and sigma.
- Dry-run: exact intended execute path with `--dry-run` before any confirm path.

Required packet claims must be specific, not generic. The validator checks for
these claim markers:

- `official_result`: completion marker plus score.
- `confirmed_lineups`: lineup or starting XI.
- `injuries_suspensions`: injury, suspension, booking, or absence.
- `odds_move`: odds.
- `ratings_move`: rating or model.
- `market_state`: market/Deadeye plus state, mu, sigma, or distribution.
- `quote_scout`: quote plus scout, EV, or expected value.

The test suite includes a synthetic realistic filled Germany packet proving
these strict gates can clear when all required post-window evidence rows are
specific and correctly slotted. This is only a regression test; it is not live
evidence and does not approve queueing or execution.

## Pre-Result Baseline

Captured for comparison on `2026-06-13T05:37Z`. Treat these as scout context,
not as execution approval.

Official fixture/result source:

- FIFA match page:
  `https://www.fifa.com/en/match-centre/match/17/285023/289273/400021464`
- Current use: primary page to check for the completed-match marker, final
  score, official lineups, substitutions, bookings, and post-match event feed.
- Post-result delta to capture: final score, completion/final-whistle marker,
  official lineups, match events, and source capture UTC.

Team news and lineup baseline:

- Bulinews preview:
  `https://bulinews.com/germany-curacao-preview-team-news-and-predicted-lineups`
- Sports Mole preview:
  `https://www.sportsmole.co.uk/football/germany/world-cup-2026/preview/germany-vs-curacao-prediction-team-news-lineups_599044.html`
- The Standard preview:
  `https://www.standard.co.uk/sport/football/germany-vs-curacao-prediction-kick-off-time-tv-live-stream-team-news-latest-h2h-results-odds-world-cup-2026-preview-b1285707.html`
- Current baseline: Germany injury context is Lennart Karl out of the squad,
  Assan Ouedraogo called in, Neuer expected/targeted to start after calf
  management, and no fresh Curacao injury issue reported by the preview set.
- Current lineup watch: Germany likely Neuer; Kimmich, Tah, Schlotterbeck,
  left-back still Brown/Raum-sensitive; central midfield Nmecha/Pavlovic or
  Goretzka/Pavlovic; Sane, Musiala, Wirtz; Havertz. Curacao likely built around
  Room, Leandro Bacuna, Juninho Bacuna, Tahith Chong, Gorre, and Antonisse.
- Post-result delta to capture: confirmed XI, late absences, role changes
  versus the baseline, in-match injuries, suspensions/bookings affecting future
  matches, and substitution pattern.

Ratings/form baseline:

- Bundesliga lineup context:
  `https://www.bundesliga.com/en/bundesliga/news/how-will-germany-line-up-havertz-musiala-wirtz-nagelsmann-world-cup-2026-28807`
- Current baseline: Germany are treated as a heavy favorite with Neuer's return,
  Kimmich/Tah/Schlotterbeck/Raum or Brown defensive structure, Wirtz/Musiala
  creativity, and uncertainty concentrated at left-back and central midfield.
- Post-result delta to capture: any ratings/model movement after the final
  score, especially if the result or goal margin changes Germany's path odds.

Odds baseline:

- SportyTrader odds comparison:
  `https://www.sportytrader.com/en/odds/germany-curacao-7937446/`
- SportsLine/FanDuel market context:
  `https://www.sportsline.com/insiders/germany-vs-curacao-odds-predictions-2026-world-cup-picks-from-proven-soccer-expert/`
- Current baseline: odds screens show Germany as an overwhelming match
  favorite, draw as a longshot, Curacao as a very longshot, and the main market
  discussion centered on goal margin and total goals rather than Germany
  outright.
- Post-result delta to capture: Germany outright/path odds, Group E odds,
  round-of-32/quarterfinal path odds, and any goal-difference driven repricing.

## Scout Request

Use the mailbox scout lane before the window:

```text
scout_claude: prepare Germany SNAP-PREP for 2026-06-14T20:00:00Z.
Need official result URL/marker, lineups, injuries/suspensions, odds move, and
ratings move. Do not propose pre-result execution. Please ACK.
```

## Post-Result Commands

Build the fillable evidence packet first:

```bash
python3 scripts/storm_worldcup_evidence_packet.py \
  --template ~/.local/state/storm-deadeye/templates/germany-post-result-snap-template-20260612.json \
  --output ~/.local/state/storm-deadeye/germany-post-result-evidence-packet.json
```

After filling every required evidence row, validate the packet before copying
anything into the template:

```bash
python3 scripts/storm_worldcup_evidence_packet.py \
  --validate-packet ~/.local/state/storm-deadeye/germany-post-result-evidence-packet.json \
  --output ~/.local/state/storm-deadeye/germany-post-result-evidence-packet.json
```

`capture_readiness.ready_for_template_update` must be `true` before any template
edit. Every required evidence row must have a `capture_utc` at or after
`result_not_before_utc`; pre-window captures are rejected even if validation is
run after the window opens. Check `capture_status.next_action`,
`capture_status.missing_ids`, and the row-level
`capture_status.rows[].blockers` to see what is still missing. This still does
not approve queueing or execution.

After validation passes, copy the captured evidence into the local template:

```bash
python3 scripts/storm_worldcup_evidence_packet.py \
  --apply-to-template ~/.local/state/storm-deadeye/germany-post-result-evidence-packet.json \
  --template ~/.local/state/storm-deadeye/templates/germany-post-result-snap-template-20260612.json
```

This sets `world_cup_post_result=true` and removes placeholder evidence, but it
keeps the template disabled. Queueing still requires fresh quote, dry-run,
concentration, gas, XP, trade-cap, and promotion gates.

Read-only refresh sequence:

```bash
deadeye markets show 0x1e7b71e2e9f26c9f37d9419a8e542049194053eb9534455306518a98746f803 --output json
deadeye doctor --market 0x1e7b71e2e9f26c9f37d9419a8e542049194053eb9534455306518a98746f803 --output plain
python3 scripts/storm_gap_analyzer.py --preset active-portfolio-20260612 --budget 4000 --budget-ladder --quote-only --sort-by ev
python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox --refresh-active-portfolio-scout --active-portfolio-scout-max-age-minutes 0
```

Execution stays under the runner only. No manual `--confirm` command belongs in
this runbook.
