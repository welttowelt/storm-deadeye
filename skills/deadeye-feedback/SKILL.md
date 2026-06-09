---
name: deadeye-feedback
description: "File a well-structured, tagged feature request or bug report against deadeye-rs from the CLI. Use when the user wants to request a feature, report a bug, or suggest an improvement to the deadeye CLI / SDK / skills. Wraps `deadeye feedback`, which posts a GitHub issue via the `gh` CLI with a consistent template (type, component, environment)."
version: 1.0.0
license: MIT
platforms: [linux, macos, windows]
metadata:
  deadeye:
    tags: [feedback, feature-request, bug-report, github, issues, cli]
    category: meta
    related_skills: [deadeye-cli]
---

# Deadeye feedback

When the user wants to request a feature, report a bug, or suggest an
improvement to Deadeye, file it as a structured GitHub issue with:

```bash
deadeye feedback --title "<short title>" --kind feature|bug|idea \
  --component <area> --body "<the request>"
```

`deadeye feedback` adds the scaffolding for you — a `[Feature]`/`[Bug]` title
prefix, a standard label (`enhancement` or `bug`), and an environment block —
then posts via `gh issue create`. **Preview with `--dry-run` first**, confirm
the content with the user, then file.

## Requirements

- The GitHub CLI `gh` must be installed and authenticated (`gh auth login`).
  If it isn't, `deadeye feedback` tells the user how to set it up.
- Posting prompts for y/N confirmation (it's a public issue). In a
  non-interactive agent run, pass the global `--confirm` flag **only after**
  you've shown the user the `--dry-run` preview and they've approved it.

## Write a good issue

Quality matters — a vague issue gets closed. Gather these before filing:

**Title** — specific and scannable. `forecast: add CRPS scoring after
resolution`, not `improve forecasting`.

**`--kind`**
- `feature` — new capability or enhancement.
- `bug` — something is broken or wrong.
- `idea` — open-ended suggestion / discussion starter.

**`--component`** — the area it touches, so it routes well:
`cli`, `forecast`, `wallet`, `trade`, `lp`, `indexer`, `skills`, `install`.

**`--body`** — the substance. Structure it:
- *For a feature:* the **problem** (what's missing / painful today and why),
  then a **proposed solution** (concrete behavior, ideally an example command
  or output), and any **alternatives** considered.
- *For a bug:* **steps to reproduce**, **expected** vs **actual** behavior, and
  the exact command + any error output.

Add `--label <name>` for extra labels that already exist on the repo (e.g.
`forecast`); unknown labels are dropped automatically so the issue still files.

## Examples

Preview, then file a feature request:

```bash
deadeye feedback --dry-run --title "forecast: add CRPS scoring after resolution" \
  --kind feature --component forecast \
  --body "Problem: after a market resolves there's no way to score my committed snapshot, so I can't track calibration. Proposed: \`deadeye forecast resolve <market> --outcome <x>\` then \`deadeye forecast score <market>\` computing CRPS/Brier and storing a score record. Alternative: do it by hand from the snapshot JSON."

# looks good -> file it
deadeye feedback --confirm --title "forecast: add CRPS scoring after resolution" \
  --kind feature --component forecast --body "<same body>"
```

Report a bug:

```bash
deadeye feedback --kind bug --component wallet \
  --title "onboard: deploy aborts with ClassHashNotFound on mainnet" \
  --body "Steps: \`deadeye onboard --network mainnet\`, fund, continue. Expected: account deploys. Actual: aborts with 'account class ... is not declared'. Command + output: ..."
```

After filing, share the returned issue URL with the user.
