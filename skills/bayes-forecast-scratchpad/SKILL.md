---
name: bayes-forecast-scratchpad
description: "Auditable Bayesian + distributional scratchpad for Deadeye markets: aggregate component (μ,σ) beliefs into a calibrated mean/variance, blend reference classes, pool probabilities (log-odds, with extremization), weight qualitative evidence into likelihood ratios, collapse correlated double-counting, de-vig markets, apply LR updates, and compute tail probabilities. Use whenever you need a defensible number, not a vibe. Backed by `deadeye forecast bayes`."
version: 1.0.0
license: MIT
platforms: [linux, macos, windows]
metadata:
  deadeye:
    tags: [forecasting, bayesian, probability, likelihood-ratio, pooling, calibration, distributions, superforecasting]
    category: forecasting
    related_skills: [deadeye-superforecaster, evidence-ledger]
---

# Bayesian forecast scratchpad

`deadeye forecast bayes <routine>` runs a pure toolkit routine: JSON in (via
`--input '<json>'` or stdin), JSON out on stdout, plus a one-line rationale on
stderr (suppress with `--json`). Everything is total and side-effect free, so
chain them freely. Every number you commit should trace back to one of these.

## Routines

### `aggregate-normal` — the one that produces your trade

Combine independent component `(μ, σ)` beliefs into a single distribution. This
is the bridge to `trade quote`: feed the output `mean` and `variance`.

```bash
deadeye forecast bayes aggregate-normal --input '{
  "rho": 0.3,
  "components": [
    {"mu": 3.10, "sigma": 0.30, "weight": 1.0, "side": "long"},
    {"mu": 2.95, "sigma": 0.20, "weight": 1.2}
  ]
}'
# -> {mean, sd, variance, q05, q50, q95, cvar05, n_eff}
```

- `side`: `"long"` (default) or `"short"` (negates the mean).
- `rho` ∈ [0, 0.95]: shared correlation. Higher rho **widens** variance (less
  diversification) and lowers `n_eff`. Zero-σ components add no spread — we never
  fabricate variance.
- `cvar05` is the 5% expected shortfall of the lower tail; `n_eff` is the Kish
  effective sample size of your weights.

### `blend-base-rates` — outside-view prior

```bash
deadeye forecast bayes blend-base-rates --input '{"classes":[
  {"base_rate":3.2,"applicability":0.8,"uncertainty":0.3},
  {"base_rate":3.0,"applicability":0.9,"uncertainty":0.2}
]}'
# value space (default, for continuous markets) -> {mean, sd}
```

Add `"space":"probability"` for **binary** sub-questions (blends in log-odds and
returns `{blended, uncertainty_sd}`). `uncertainty_sd` carries both between-class
disagreement and within-class spread.

### `pool` — combine independent probability estimates

```bash
deadeye forecast bayes pool --input '{"items":[{"p":0.7,"weight":1},{"p":0.8,"weight":1}],"extremize":1.0}'
# -> {probability, method, extremize}
```

- `method`: `"log_odds"` (default, the superforecaster pool — geometric mean of
  odds, respects confident minorities) or `"linear"`.
- `extremize` > 1 sharpens away from 0.5; only justified for genuinely
  **independent** sources that agree. Leave at 1.0 if they share signal.

### `evidence-weight` — qualitative evidence → likelihood ratio

```bash
deadeye forecast bayes evidence-weight --input '{
  "reliability":0.9,"relevance":0.7,"independence":0.6,"recency":1.0,"bias_risk":0.1,
  "direction":"against","strength":"medium"
}'
# -> {likelihood_ratio, log_odds, quality}
```

`quality = reliability·relevance·independence·recency·(1−bias_risk)` discounts
the raw `strength` (negligible|weak|modest|medium|strong|very_strong|decisive).
`direction`: `for` | `against` | `neutral`.

### `effective-count` — kill double-counting

```bash
deadeye forecast bayes effective-count --input '{"n":4,"rho":0.7}'   # -> {effective}
```

Four correlated takes (rho 0.7) are worth ~1.6 independent ones. Collapse a
cluster to its effective count **before** pooling or you'll over-update.

### `lr-update` — apply likelihood ratios to a prior

```bash
deadeye forecast bayes lr-update --input '{"prior":0.62,"lrs":[0.85,0.70,1.25]}'  # -> {prior, posterior}
```

### `devig` — turn a binary market price into a fair probability

```bash
deadeye forecast bayes devig --input '{"yes":0.55,"no":0.50}'   # -> {fair}
```

Strips the overround. Use the result as a **market** component / prior — never
ignore liquid market pricing.

### `prob-below` — tail probability under your forecast

```bash
deadeye forecast bayes prob-below --input '{"x":3.0,"mean":3.02,"sd":0.22}'  # -> {prob_below, prob_above}
```

Sanity-check where your committed `(mean, σ)` puts mass relative to thresholds
the market cares about.

## Discipline

- Show your work: keep the JSON inputs/outputs in the forecast rationale.
- Pool in log-odds, not arithmetic, unless sources are truly exchangeable.
- Collapse correlated evidence before combining; don't extremize shared signal.
- Keep σ honest — `aggregate-normal` won't invent spread, and neither should you.
