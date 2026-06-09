---
name: deadeye-cli
description: "Use when an agent should participate in Deadeye distribution markets via the `deadeye` CLI: onboard a wallet, claim the XP grant, fetch a market and its state from the indexer/chain, gather evidence and produce a Bayesian mean ± σ forecast, then size and submit the highest-EV trade. Covers onboard, account, collateral, markets, trade quote/execute, position, and the recover-every-run config."
---

# Deadeye CLI — agent participation

The `deadeye` CLI lets you trade Deadeye **distribution markets** on Starknet.
Unlike yes/no markets, each market holds a probability *distribution* (e.g. a
Normal with mean μ and sigma σ). You profit by moving the market's curve toward
a better-calibrated forecast: if reality lands closer to your shape than the
market's, the market pays you for the information. Collateral is **XP** — a
restricted, non-transferable token with no monetary value; only gas is paid in
STRK.

The whole loop is: **onboard → fund → claim XP → pick a market → forecast →
quote the EV-max trade → execute.**

## 0. One-time setup (wallet)

`deadeye onboard` creates or recovers a local wallet, saves the key to the
deadeye config (cleartext, `0600`) so you recover the same wallet every run,
and deploys the account contract once it's funded:

```bash
deadeye onboard --network mainnet          # generates a recovery phrase
deadeye onboard --network mainnet --import # recover from an existing phrase
```

The wizard first asks for the **RPC endpoint** (defaults to ZAN's public node;
it suggests getting a free Alchemy key for better reliability — pass your own
with `--rpc-url`), then prints an **account address** and waits for you to send
STRK to it for gas. Ask the human operator to fund that address; the CLI polls
the balance and deploys the account once it's funded. Re-running
`onboard --import` resumes a half-finished setup.

Onboarding **never overwrites an existing wallet** by accident: re-running it on
a profile that already holds a key is refused unless you pass `--force`
(importing the same phrase is allowed — it's idempotent).

**Multiple wallets / accounts.** Each named profile is its own wallet. Create
more with `deadeye onboard --profile <name>` and list them so you can pick which
account to trade from:

```bash
deadeye account list --output json    # every wallet: profile, address, network, deployed
deadeye trade quote <MARKET> --profile alice ...   # trade from a specific wallet
```

Any command takes `--profile <name>`; omit it to use the default (marked `*`).

Config lives at `~/.config/deadeye/config.toml` (override with `DEADEYE_CONFIG`).
Each profile carries the RPC, indexer, chain id, address, and key, so commands
"just work" with no flags. To keep the key out of the file, set
`DEADEYE_PRIVATE_KEY` in the environment instead — it wins over the stored key.

Confirm the wallet and claim your starting XP:

```bash
deadeye account show                       # address, STRK balance, chain id
deadeye collateral balance                 # XP balance + grant status
deadeye collateral claim-grant --execute   # mint the one-shot XP grant
```

## 1. Pick a market and read its state

```bash
deadeye markets list --limit 20                 # browse open markets
deadeye markets list --family normal --output json
deadeye markets show <MARKET_ADDR>              # on-chain distribution (μ, σ), LP backing
deadeye markets info  <MARKET_ADDR>             # indexer metadata: title, description, tags
```

`markets show` gives the market's current curve `(μ_mkt, σ_mkt)`. `markets info`
gives the human question you are forecasting. Use `--output json` for anything
you need to parse.

## 2. Produce a Bayesian forecast

This is your job as a superforecaster. The CLI now gives you a structured,
auditable workspace for it — use the **deadeye-superforecaster** skill for the
full loop, **evidence-ledger** for research discipline, and
**bayes-forecast-scratchpad** for the math. In short:

```bash
deadeye forecast new <MARKET_ADDR> --title "<question>" --lower <lo> --upper <hi>
deadeye forecast base-rate <MARKET_ADDR> --name "<class>" --rate <r> --applicability <a> --uncertainty <u>
deadeye forecast evidence  <MARKET_ADDR> --claim "<...>" --stance up|down --source "<...>" --reliability <0..1> --relevance <0..1>
deadeye forecast bayes aggregate-normal --input '{"rho":0.3,"components":[{"mu":..,"sigma":..,"weight":..}, ...]}'
deadeye forecast snapshot  <MARKET_ADDR> --mean <μ> --sd <σ> --rationale "<...>"
deadeye forecast show      <MARKET_ADDR>
```

Output a **calibrated** posterior: a mean `μ_you` and 1σ uncertainty `σ_you`
(or `p ± σ` for a binary). Be honest about σ — overconfidence is punished when
the market settles. The snapshot prints the exact `trade quote` command to run.

## 3. Turn the forecast into the highest-EV trade

Do **not** jump the market all the way to `μ_you`. The optimal post-trade curve
`(μ′, σ′)` maximizes expected payoff under *your* forecast minus the collateral
it costs, capped by your budget. The CLI runs that search for you — pass your
belief and budget and it returns the EV-max candidate plus an on-chain preflight
verdict:

```bash
# Optimizer picks (μ′, σ′): belief mean + budget (XP) + optional belief sigma.
deadeye trade quote <MARKET_ADDR> --belief <MU_YOU> --budget <XP_BUDGET> --belief-sigma <SIGMA_YOU>

# Or quote a specific candidate distribution directly:
deadeye trade quote <MARKET_ADDR> --mean <MU'> --variance <VAR'>
```

`trade quote` is read-only: it reads market state, computes the candidate, and
runs the chain's `check_trade_view` so you see the quoted collateral and whether
the chain will accept *before* spending anything. Some families need a math
runtime address — pass `--runtime 0x...` or set `DEADEYE_<FAMILY>_RUNTIME_ADDR`
if the quote asks for it.

## 4. Execute

```bash
deadeye trade execute <MARKET_ADDR> --belief <MU_YOU> --budget <XP_BUDGET> \
    --max-collateral <XP_CAP> --journal ~/.local/share/deadeye/journal.jsonl
```

`trade execute` re-runs the quote (state may have moved), prompts for
confirmation (skip with the global `--confirm`), submits the trade, and appends
to the journal on success. `--max-collateral` is a hard ceiling in XP — the
trade aborts if the fresh quote exceeds it. Keep it ≤ your budget.

Afterwards:

```bash
deadeye position show <MARKET_ADDR>   # your position on this market
deadeye position list                 # all open positions
deadeye trade journal --tail 20       # recent trade log
```

## Notes and safety

- **XP is non-transferable and worthless off-platform.** It cannot be drained;
  only the gas STRK on the address has value. Still, treat the config file as
  secret — it holds the spendable gas key.
- Always `trade quote` before `trade execute`. Respect `--max-collateral`.
- Use `--output json` for machine parsing; `-v` sends tracing to stderr without
  polluting stdout JSON.
- `deadeye <command> --help` documents every flag.

## Updating

Keep the CLI current so you keep getting new commands, skills, and fixes:

```bash
deadeye update --check    # report whether a newer release exists
deadeye update            # check, then update the binary + refresh skills
```

`deadeye update` re-runs the installer under the hood; you can also do it
manually with `curl -fsSL https://project-deadeye.vercel.app/install.sh | sh`.
Restart your agent app after updating to pick up refreshed skills.

Restart the agent app after installing or updating skills.
