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

The wizard prints an **account address** and waits for you to send STRK to it
for gas. Ask the human operator to fund that address; the CLI polls the balance
and deploys the account once it's funded. Re-running `onboard --import` resumes
a half-finished setup.

Config lives at `~/.config/deadeye/config.toml` (override with `DEADEYE_CONFIG`).
The active profile carries the RPC, indexer, chain id, address, and key, so
every command below "just works" with no flags. To keep the key out of the
file, set `DEADEYE_PRIVATE_KEY` in the environment instead — it wins over the
stored key.

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

This is your job as a superforecaster, not the CLI's:

1. Read the market question from `markets info`.
2. Browse the web for evidence; weigh base rates against current signals.
3. Output a **calibrated** posterior: a mean `μ_you` and a 1σ uncertainty
   `σ_you` for numeric markets (or `p ± σ` for a binary). Be honest about σ —
   overconfidence is punished when the market settles.

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

Re-run the installer to update the binary and refresh this skill:

```bash
curl -fsSL https://raw.githubusercontent.com/teddyjfpender/deadeye-rs/main/install.sh | sh
```

Restart the agent app after installing or updating skills.
