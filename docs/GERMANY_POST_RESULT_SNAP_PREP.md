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

- Official result: final score, completed/final marker, source URL, capture UTC.
- Lineups: starting XI, substitutions, notable absences, source URL, capture UTC.
- Injuries/suspensions: pre-match and in-match changes that affect Germany's
  later tournament path.
- Odds move: at least one pre-result snapshot and one post-result snapshot for
  Germany outright odds or path odds.
- Ratings move: model/ratings source before and after, if available.
- Market state: `deadeye markets show <market> --output json` after result.
- Quote: quote JSON after result using fresh belief and sigma.
- Dry-run: exact intended execute path with `--dry-run` before any confirm path.

## Scout Request

Use the mailbox scout lane before the window:

```text
scout_claude: prepare Germany SNAP-PREP for 2026-06-14T20:00:00Z.
Need official result URL/marker, lineups, injuries/suspensions, odds move, and
ratings move. Do not propose pre-result execution. Please ACK.
```

## Post-Result Commands

Read-only refresh sequence:

```bash
deadeye markets show 0x1e7b71e2e9f26c9f37d9419a8e542049194053eb9534455306518a98746f803 --output json
deadeye doctor --market 0x1e7b71e2e9f26c9f37d9419a8e542049194053eb9534455306518a98746f803 --output plain
python3 scripts/storm_gap_analyzer.py --preset active-portfolio-20260612 --budget 4000 --budget-ladder --quote-only --sort-by ev
python3 scripts/storm_deadeye_loop.py --run-smoke --mailbox --refresh-active-portfolio-scout --active-portfolio-scout-max-age-minutes 0
```

Execution stays under the runner only. No manual `--confirm` command belongs in
this runbook.

