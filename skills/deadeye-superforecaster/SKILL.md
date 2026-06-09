---
name: deadeye-superforecaster
description: "The guided superforecasting loop for Deadeye distribution markets: read the market's outcome space, set reference-class base rates, gather and weight timestamped evidence, decompose into components, aggregate into a calibrated (mean, σ), commit a snapshot, and turn it into the highest-EV trade. Use when forecasting a continuous Deadeye market end-to-end. Backed by `deadeye forecast` + `deadeye trade`."
version: 1.0.0
license: MIT
platforms: [linux, macos, windows]
metadata:
  deadeye:
    tags: [forecasting, superforecasting, distribution-markets, bayesian, calibration, trading]
    category: forecasting
    related_skills: [bayes-forecast-scratchpad, evidence-ledger, deadeye-cli]
---

# Deadeye superforecaster loop

A Deadeye market holds a **probability distribution** (e.g. a Normal with mean μ
and sigma σ), not a yes/no contract. You profit by moving the market's curve
toward a **better-calibrated** distribution. Your whole job is to produce an
honest `(mean, σ)` and let `deadeye trade quote` size the EV-maximizing move.

The loop is auditable: every forecast is built from evidence and reference
classes recorded in a per-market **workspace** (`deadeye forecast …`), so you
can re-run it, defend it, and score it after resolution.

## The loop

```
parse → base-rate → evidence → weight → decompose → aggregate → commit → trade → monitor → score
```

### 1. Parse the market (outcome space)

```bash
deadeye markets show  <MARKET>   # on-chain distribution (μ_mkt, σ_mkt), family, LP backing
deadeye markets info  <MARKET>   # the human question + resolution criteria
deadeye forecast new  <MARKET> --title "<question>" --lower <lo> --upper <hi> --resolution "<how it resolves>"
```

Write down exactly what resolves the market and the numeric range. If it isn't
scoreable, stop and clarify before forecasting.

### 2. Reference classes → base-rate prior

Anchor on the outside view first. Add 2–4 reference classes (numeric anchors for
a continuous market), each with an applicability weight and within-class sd:

```bash
deadeye forecast base-rate <MARKET> --name "CPI YoY, last 12 prints" --rate 3.2 --applicability 0.8 --uncertainty 0.3
deadeye forecast base-rate <MARKET> --name "Cleveland Fed nowcast"   --rate 3.0 --applicability 0.9 --uncertainty 0.2
```

The CLI blends them in **value space** into a prior `mean ± sd`. This is your
starting distribution before any inside-view evidence.

### 3. Gather + record evidence

Use the **evidence-ledger** skill for the source taxonomy and scoring. Record
every item, timestamped and source-linked, with a stance and quality ratings:

```bash
deadeye forecast evidence <MARKET> --claim "Gasoline fell 3% in May" --stance down \
    --source "EIA weekly" --url https://... --reliability 0.9 --relevance 0.7
```

`--stance up|down|context|mixed`. Prefer primary/official sources; note when two
sources are not independent (you'll collapse them in step 4).

### 4. Weight evidence (and avoid double-counting)

Turn qualitative evidence into a likelihood ratio, and collapse correlated
clusters so you don't over-update. Use the **bayes-forecast-scratchpad** skill:

```bash
deadeye forecast bayes evidence-weight --input '{"reliability":0.9,"relevance":0.7,"independence":0.6,"recency":1,"bias_risk":0.1,"direction":"against","strength":"medium"}'
deadeye forecast bayes effective-count --input '{"n":4,"rho":0.7}'   # 4 correlated takes ≈ ? independent
```

### 5. Decompose into components

Express your belief as a few independent component `(μ, σ)` estimates — e.g.
the base-rate prior, an inside-view mechanism, and the **de-vigged market
itself** as one component (never ignore the market; it's information). Give each
a weight and a side (long/short).

### 6. Aggregate into a calibrated (mean, σ)

```bash
deadeye forecast bayes aggregate-normal --input '{
  "rho": 0.3,
  "components": [
    {"mu": 3.10, "sigma": 0.30, "weight": 1.0},   # base-rate prior
    {"mu": 2.95, "sigma": 0.20, "weight": 1.2},   # inside view (disinflation)
    {"mu": 3.05, "sigma": 0.25, "weight": 0.8}    # de-vigged market
  ]
}'
```

`rho` is the shared correlation across components (0 = independent). The output
`mean`, `sd`, and `variance` are your forecast. **Keep σ honest** — if you can't
name a credible path for the tail mass, don't fabricate spread, but don't
collapse it either.

### 7. Commit the snapshot

```bash
deadeye forecast snapshot <MARKET> --mean 3.02 --sd 0.22 --method aggregate-normal \
    --rationale "Outside view ~3.1; energy disinflation pulls it down; market roughly agrees." \
    --reason-up "shelter stays sticky" --reason-down "energy keeps falling" \
    --change-my-mind "a hot core services print"
deadeye forecast show <MARKET>     # review the full workspace + the trade command
```

### 8. Turn the forecast into the highest-EV trade

Do **not** jump the market to your mean. `trade quote` runs the EV-max grid
search between the market's curve and yours under your budget:

```bash
deadeye trade quote   <MARKET> --mean 3.02 --variance 0.0484
deadeye trade execute <MARKET> --mean 3.02 --variance 0.0484 --max-collateral <XP_CAP>
```

(Or drive it from your belief directly: `trade quote <MARKET> --belief <mean> --budget <xp> --belief-sigma <sd>`.)

### 9. Monitor, re-run, score

Re-run when watched inputs move materially (re-record evidence, re-aggregate,
re-commit). After the market resolves, score your forecast (Brier/CRPS-style),
write a one-paragraph postmortem — what you missed, what you over-weighted — and
carry the lesson into the next market's base rates.

## Calibration discipline (do not skip)

- **Outside view before inside view.** Anchor on base rates; let evidence move
  you off them, not replace them.
- **Don't double-count.** Correlated sources share signal — collapse them with
  `effective-count` before pooling.
- **Stress-test the disconfirming case.** Before a confident, narrow σ, name the
  credible path where you're wrong and widen toward it.
- **The market is evidence.** Include the de-vigged market as a component;
  disagreeing with it is fine, ignoring it is not.
- **XP is non-transferable and worthless off-platform** — only gas STRK has
  value. Always `trade quote` before `execute`; respect `--max-collateral`.
