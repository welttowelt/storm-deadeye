---
name: evidence-ledger
description: "How to gather, score, de-duplicate, and record evidence for a Deadeye market forecast: a source-role taxonomy, reliability/relevance/independence/recency/bias scoring, primary-vs-secondary and double-counting discipline, and the timestamped evidence-item schema. Use when researching a market before forecasting. Backed by `deadeye forecast evidence`."
version: 1.0.0
license: MIT
platforms: [linux, macos, windows]
metadata:
  deadeye:
    tags: [forecasting, evidence, research, sources, curation, calibration, superforecasting]
    category: forecasting
    related_skills: [deadeye-superforecaster, bayes-forecast-scratchpad]
---

# Evidence ledger

Good forecasts are built from a curated, timestamped evidence trail — not a
single web search. Record every material item in the market's workspace so the
forecast is auditable and re-runnable.

```bash
deadeye forecast evidence <MARKET> --claim "<headline>" --stance up|down|context|mixed \
    --source "<label>" --url <url> --reliability <0..1> --relevance <0..1>
```

Items append to `evidence.jsonl` in the workspace and show up in
`deadeye forecast show <MARKET>`.

## 1. Plan your sources by role

Cover these roles before forming a view — each answers a different question:

| Role | What it gives you | Examples |
| --- | --- | --- |
| **official measurement** | the thing that *resolves* the market | BLS, FRED series, SEC filings, the stated resolution source |
| **quantitative input** | hard series that drive the outcome | FRED, EIA, BLS, Census, World Bank |
| **leading indicator** | fast early signal | nowcasts, high-frequency prints, supplier data |
| **market prior** | liquid, de-vigged implied view | Kalshi, Polymarket, Metaculus, Manifold |
| **news early warning** | broad text monitoring for surprises | reputable wires, GDELT-style sweeps |
| **scientific / regulatory** | primary evidence | PubMed, arXiv, Federal Register, court records |

Anchor on the **official measurement** (it defines truth) and always pull a
**market prior** (de-vig it with `bayes devig` and treat it as a component).

## 2. Score each item (these become likelihood ratios)

For every item, judge five 0..1 dimensions — they feed `bayes evidence-weight`:

- **reliability** — source track record / editorial standards.
- **relevance** — how directly it bears on *this* question.
- **independence** — is it correlated with evidence you already have?
- **recency** — fresh, or stale relative to the question's clock?
- **bias_risk** — partisan / sponsored / systematic distortion (higher = worse).

Record `--reliability` and `--relevance` on the item; keep independence, recency,
and bias_risk in your reasoning and apply them when you weight the item.

## 3. Curate: primary over secondary, and de-duplicate

- **Primary beats secondary.** An official print or filing outranks a news
  article *about* it. Tag the primary as the official-measurement source.
- **De-duplicate.** Three outlets reporting the same wire is **one** piece of
  evidence. Don't record it three times, and don't let it triple-count.
- **Collapse correlated clusters.** When several items share a driver, estimate
  their shared `rho` and use `bayes effective-count --input '{"n":N,"rho":...}'`
  to get the effective independent count before you pool/update. This is the #1
  way forecasts become overconfident.
- **Respect the clock.** For anything you might backtest, only use evidence that
  was *available* at the forecast time — note `available_at` in the claim if it
  differs from when you found it.

## 4. Stance, not spin

`--stance` records the **direction** an item pushes the outcome:

- `up` / `down` — moves the forecast mean up or down (or supports/opposes a
  binary).
- `context` — background that frames the question but doesn't move the number.
- `mixed` — genuinely two-sided; split it into separate items if you can.

Be honest about disconfirming evidence — log the items that cut against your
lean, then weight them. A ledger that only contains supporting evidence is a
red flag for motivated reasoning.

## Evidence-item schema (what gets stored)

```json
{
  "id": "e3",
  "captured_at": 1780000000,
  "claim": "Gasoline fell 3% in May",
  "source": "EIA weekly",
  "url": "https://...",
  "stance": "down",
  "reliability": 0.9,
  "relevance": 0.7
}
```

Once you've gathered and scored evidence, hand off to the
**deadeye-superforecaster** loop (decompose → `bayes aggregate-normal` →
`forecast snapshot` → `trade quote`).
