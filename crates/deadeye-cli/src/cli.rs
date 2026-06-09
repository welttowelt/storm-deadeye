//! Top-level `clap` command definitions.

use clap::{Parser, Subcommand, ValueEnum};

use crate::output::OutputMode;

/// Deadeye — market-maker-grade CLI for the Deadeye Rust SDK.
///
/// Read-path commands wrap the SDK's read surface (markets, positions,
/// LP info, indexer metadata). Output mode auto-detects: a TTY renders
/// colored / tabular output; a pipe renders plain `key: value` lines.
/// Use `--output json` for machine-readable output.
///
/// # Examples
///
/// ```text
/// # One-shot read against the default Sepolia profile:
/// deadeye markets list --limit 5
///
/// # Inspect one market (family auto-detected):
/// deadeye markets show 0x53e5…0fcf4
///
/// # Pipe-friendly for jq / awk:
/// deadeye markets list --output json | jq '.[] | .address'
/// ```
#[derive(Debug, Parser)]
#[command(
    name = "deadeye",
    version,
    about = "Market-maker-grade CLI for the Deadeye Rust SDK",
    long_about = None,
    propagate_version = true,
    arg_required_else_help = true,
)]
pub(crate) struct Cli {
    /// Override the Starknet JSON-RPC URL.
    ///
    /// Falls back to `DEADEYE_RPC_URL`, then to the active profile's
    /// `rpc_url`, then to a public Sepolia endpoint.
    #[arg(long, global = true, value_name = "URL", env = "DEADEYE_RPC_URL")]
    pub(crate) rpc_url: Option<String>,

    /// Override the indexer base URL.
    #[arg(long, global = true, value_name = "URL", env = "DEADEYE_INDEXER_URL")]
    pub(crate) indexer_url: Option<String>,

    /// Trader / account address (hex felt) used as the default for
    /// account-bound subcommands.
    #[arg(long, global = true, value_name = "0x...", env = "DEADEYE_ADDRESS")]
    pub(crate) address: Option<String>,

    /// Use a named profile from `~/.config/deadeye/config.toml`.
    #[arg(long, global = true, value_name = "NAME", env = "DEADEYE_PROFILE")]
    pub(crate) profile: Option<String>,

    /// Output mode. Defaults to `pretty` on a TTY and `plain` in a pipe.
    #[arg(long, global = true, value_name = "MODE")]
    pub(crate) output: Option<OutputModeArg>,

    /// Disable ANSI colors.
    ///
    /// Also honored when the `NO_COLOR` environment variable is set or
    /// stdout is not a terminal.
    #[arg(long, global = true)]
    pub(crate) no_color: bool,

    /// Enable verbose tracing (writes to stderr; does not contaminate JSON).
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,

    /// Skip interactive confirmation prompts on destructive commands.
    #[arg(long, global = true)]
    pub(crate) confirm: bool,

    #[command(subcommand)]
    pub(crate) command: Command,
}

impl Cli {
    /// Whether destructive commands should skip the y/N prompt.
    pub(crate) const fn confirm(&self) -> bool {
        self.confirm
    }
}

/// `--output` shadow so clap can derive `ValueEnum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputModeArg {
    /// Colored, tabular, human-friendly output (default on a TTY).
    Pretty,
    /// `key: value` lines, no colors, no boxes (default in a pipe).
    Plain,
    /// Pretty-printed JSON dumped to stdout.
    Json,
}

impl OutputModeArg {
    pub(crate) fn into_mode(self) -> OutputMode {
        match self {
            Self::Pretty => OutputMode::Pretty,
            Self::Plain => OutputMode::Plain,
            Self::Json => OutputMode::Json,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Create or recover a local wallet and deploy its account (start here).
    ///
    /// Interactive wizard: generate or import a BIP-39 phrase, derive the
    /// Starknet account, print the address to fund with STRK, poll the
    /// balance, then deploy the account contract. The key is saved to the
    /// active profile so every later command (and any agent) recovers the
    /// same wallet.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye onboard --network mainnet
    /// deadeye onboard --import            # recover from an existing phrase
    /// ```
    Onboard(OnboardArgs),
    /// Superforecasting workspace: gather evidence, blend base rates, and
    /// curate a calibrated (mean, σ) forecast that feeds `trade quote`.
    Forecast {
        #[command(subcommand)]
        action: ForecastCmd,
    },
    /// Submit a feature request / bug report as a structured GitHub issue.
    ///
    /// Builds a well-tagged issue (type, component, environment) and posts it
    /// to the repo via the `gh` CLI. Run with `--dry-run` to preview first.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye feedback --title "forecast: add CRPS scoring" \
    ///   --kind feature --component forecast \
    ///   --body "After a market resolves I want `deadeye forecast score` to ..."
    /// ```
    Feedback(FeedbackArgs),
    /// Check for a newer release and update the CLI (and skills) in place.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye update --check   # just report whether a newer version exists
    /// deadeye update           # check, then re-run the installer to update
    /// ```
    Update(UpdateArgs),
    /// Inspect the active account / profile.
    Account {
        #[command(subcommand)]
        action: AccountCmd,
    },
    /// Browse markets from the indexer and on-chain.
    Markets {
        #[command(subcommand)]
        action: MarketsCmd,
    },
    /// Read trader positions and LP shares.
    Position {
        #[command(subcommand)]
        action: PositionCmd,
    },
    /// Manage the on-disk configuration file.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },

    // ─── Driver B subcommands ─────────────────────────────────────────
    /// Trade preflight / execute / journal (Driver B).
    Trade {
        #[command(subcommand)]
        action: TradeCmd,
    },
    /// LP add / remove (Driver B write paths).
    Lp {
        #[command(subcommand)]
        action: LpCmd,
    },
    /// Claim a (post-settlement) position.
    Claim(ClaimArgs),
    /// Admin (factory-owner) operations: settle, pause, unpause, collect-fees.
    Admin {
        #[command(subcommand)]
        action: AdminCmd,
    },
    /// Block-driven live stream for one market.
    Watch(WatchArgs),
    /// Restricted-collateral-token (XP) operations.
    ///
    /// Wraps the deployed `restricted_collateral_token` contract — the
    /// ERC-20 that the Deadeye AMMs accept as `transfer_from` source. The
    /// `claim-grant` subcommand calls `claim_initial_grant()` on the
    /// token, which mints a fixed amount to a fresh wallet so it can
    /// start trading.
    Collateral {
        #[command(subcommand)]
        action: CollateralCmd,
    },
}

/// Target network for `deadeye onboard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum NetworkArg {
    /// Starknet mainnet (real STRK for gas).
    Mainnet,
    /// Starknet Sepolia testnet (faucet STRK).
    Sepolia,
}

/// `deadeye onboard …`
#[derive(Debug, clap::Args)]
pub(crate) struct OnboardArgs {
    /// Network to onboard against. Sets RPC, indexer, and chain id.
    #[arg(long, value_name = "NETWORK", default_value = "mainnet")]
    pub(crate) network: NetworkArg,
    /// Profile name to write the wallet into (defaults to the network name).
    #[arg(long, value_name = "NAME")]
    pub(crate) profile: Option<String>,
    /// Recover from an existing recovery phrase instead of generating one.
    #[arg(long)]
    pub(crate) import: bool,
    /// Account-contract class hash to deploy. Defaults to the bundled
    /// OpenZeppelin class; must be declared on the target network.
    #[arg(long, value_name = "0x...")]
    pub(crate) account_class_hash: Option<String>,
    /// Minimum STRK the address must hold before deploying the account.
    #[arg(long, default_value_t = 0.001)]
    pub(crate) min_strk: f64,
    /// Skip the balance/fund wait and deploy step (wallet is saved only).
    #[arg(long)]
    pub(crate) skip_deploy: bool,
    /// Override the RPC URL (otherwise derived from `--network`).
    #[arg(long, value_name = "URL")]
    pub(crate) rpc_url: Option<String>,
    /// Override the indexer URL (otherwise derived from `--network`).
    #[arg(long, value_name = "URL")]
    pub(crate) indexer_url: Option<String>,
    /// Overwrite an existing saved wallet on this profile. Without it,
    /// onboarding refuses to clobber a key you already have.
    #[arg(long)]
    pub(crate) force: bool,
}

// ─── Driver B argument types ─────────────────────────────────────────

/// Shared "trader override" flag.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct TraderOpt {
    /// Trader / account address; defaults to the active profile's address.
    #[arg(long, value_name = "0x...")]
    pub(crate) trader: Option<String>,
}

/// `deadeye trade …`
#[derive(Debug, Subcommand)]
pub(crate) enum TradeCmd {
    /// Off-chain + chain preflight for a candidate distribution.
    ///
    /// If `--belief` and `--budget` are both supplied, the optimizer
    /// picks the EV-maximizing candidate. Otherwise the caller supplies
    /// `--mean` + `--variance` (+ `--rho` / `--mu2` for bivariate).
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye trade quote 0xMARKET --mean 43.0 --variance 81.0
    /// ```
    Quote(TradeQuoteArgs),
    /// Submit a trade after a fresh preflight.
    Execute(TradeExecuteArgs),
    /// Show / open / replay a trade journal.
    Journal(TradeJournalArgs),
}

#[derive(Debug, clap::Args)]
pub(crate) struct TradeQuoteArgs {
    /// Market contract address.
    #[arg(value_name = "ADDRESS")]
    pub(crate) market: String,
    /// Force a specific family (otherwise auto-detected).
    #[arg(long, value_name = "FAMILY")]
    pub(crate) family: Option<FamilyArg>,
    /// Candidate mean μ.
    #[arg(long)]
    pub(crate) mean: Option<f64>,
    /// Candidate variance σ².
    #[arg(long)]
    pub(crate) variance: Option<f64>,
    /// Bivariate ρ (correlation).
    #[arg(long)]
    pub(crate) rho: Option<f64>,
    /// Bivariate μ₂ (second mean).
    #[arg(long)]
    pub(crate) mu2: Option<f64>,
    /// Optimizer: trader's directional belief about the true mean.
    #[arg(long)]
    pub(crate) belief: Option<f64>,
    /// Optimizer: budget (max collateral the trader will risk).
    #[arg(long)]
    pub(crate) budget: Option<f64>,
    /// Optimizer: belief sigma (defaults to current market sigma).
    #[arg(long)]
    pub(crate) belief_sigma: Option<f64>,
    /// Math-runtime contract address. Defaults to env
    /// `DEADEYE_NORMAL_RUNTIME_ADDR` (etc., per family).
    #[arg(long)]
    pub(crate) runtime: Option<String>,
    /// Collateral pad (in STRK) applied to the chain-computed amount.
    #[arg(long, default_value_t = 0.0)]
    pub(crate) pad: f64,
}

#[derive(Debug, clap::Args)]
pub(crate) struct TradeExecuteArgs {
    /// Market contract address.
    #[arg(value_name = "ADDRESS")]
    pub(crate) market: String,
    /// Force a specific family (otherwise auto-detected).
    #[arg(long, value_name = "FAMILY")]
    pub(crate) family: Option<FamilyArg>,
    /// Candidate mean μ.
    #[arg(long)]
    pub(crate) mean: Option<f64>,
    /// Candidate variance σ².
    #[arg(long)]
    pub(crate) variance: Option<f64>,
    /// Bivariate ρ.
    #[arg(long)]
    pub(crate) rho: Option<f64>,
    /// Bivariate μ₂.
    #[arg(long)]
    pub(crate) mu2: Option<f64>,
    /// Optimizer belief mean.
    #[arg(long)]
    pub(crate) belief: Option<f64>,
    /// Optimizer budget.
    #[arg(long)]
    pub(crate) budget: Option<f64>,
    /// Maximum collateral the caller is willing to supply (STRK).
    #[arg(long)]
    pub(crate) max_collateral: f64,
    /// Math runtime address.
    #[arg(long)]
    pub(crate) runtime: Option<String>,
    /// Journal path — appends a `Trade` entry on success.
    #[arg(long)]
    pub(crate) journal: Option<std::path::PathBuf>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct TradeJournalArgs {
    /// Path to a journal file. If absent, falls back to
    /// `~/.local/share/deadeye/journal.jsonl`.
    #[arg(long)]
    pub(crate) path: Option<std::path::PathBuf>,
    /// Show the most recent N entries.
    #[arg(long, default_value_t = 20)]
    pub(crate) tail: usize,
}

/// `deadeye position sell …` — extend Driver A's PositionCmd.
///
/// We keep PositionCmd's read variants intact and add `Sell` here.
#[derive(Debug, clap::Args)]
pub(crate) struct PositionSellArgs {
    /// Market contract address.
    #[arg(value_name = "ADDRESS")]
    pub(crate) market: String,
    /// Force family.
    #[arg(long)]
    pub(crate) family: Option<FamilyArg>,
    /// Minimum token-out (slippage floor, u128 base units).
    #[arg(long, default_value_t = 0)]
    pub(crate) min_out: u128,
    /// Math runtime address (normal only — others ignore).
    #[arg(long)]
    pub(crate) runtime: Option<String>,
    /// Journal path.
    #[arg(long)]
    pub(crate) journal: Option<std::path::PathBuf>,
}

/// `deadeye lp …`
#[derive(Debug, Subcommand)]
pub(crate) enum LpCmd {
    /// Add liquidity to a market.
    Add(LpAddArgs),
    /// Remove a fraction of LP shares from a market.
    Remove(LpRemoveArgs),
}

#[derive(Debug, clap::Args)]
pub(crate) struct LpAddArgs {
    /// Market contract address.
    #[arg(value_name = "ADDRESS")]
    pub(crate) market: String,
    /// Force family.
    #[arg(long)]
    pub(crate) family: Option<FamilyArg>,
    /// Amount of LP shares to add (in STRK-equivalent units).
    #[arg(long)]
    pub(crate) amount: f64,
}

#[derive(Debug, clap::Args)]
pub(crate) struct LpRemoveArgs {
    /// Market contract address.
    #[arg(value_name = "ADDRESS")]
    pub(crate) market: String,
    /// Force family.
    #[arg(long)]
    pub(crate) family: Option<FamilyArg>,
    /// Fraction of LP shares to remove (0 < f ≤ 1).
    #[arg(long)]
    pub(crate) fraction: f64,
}

/// `deadeye collateral …`
#[derive(Debug, Subcommand)]
pub(crate) enum CollateralCmd {
    /// Mint the one-shot initial grant of XP into the active wallet.
    ///
    /// The XP token's `claim_initial_grant()` mints `initial_grant()`
    /// tokens to the caller iff `has_claimed_initial_grant(caller)` is
    /// still `false`. Re-running on an already-claimed wallet is a clean
    /// no-op — the command short-circuits before submitting.
    ///
    /// # Example
    ///
    /// ```text
    /// # Dry-run (default): show what would happen.
    /// deadeye collateral claim-grant
    ///
    /// # Real submission.
    /// deadeye collateral claim-grant --execute
    ///
    /// # Custom token address (sepolia / devnet).
    /// deadeye collateral claim-grant --token 0x4583… --execute
    /// ```
    ClaimGrant(CollateralClaimGrantArgs),
    /// Show the wallet's XP balance + grant-claim status.
    Balance(CollateralBalanceArgs),
}

#[derive(Debug, clap::Args)]
pub(crate) struct CollateralClaimGrantArgs {
    /// Override the collateral-token address. Defaults to the bundled
    /// mainnet XP address (`MAINNET_XP_TOKEN_ADDRESS`). Required on
    /// sepolia / devnet.
    #[arg(long, value_name = "0x...")]
    pub(crate) token: Option<String>,
    /// Submit the transaction. Without this flag, the command performs
    /// the pre-flight reads and prints the plan but never signs.
    #[arg(long)]
    pub(crate) execute: bool,
}

#[derive(Debug, clap::Args)]
pub(crate) struct CollateralBalanceArgs {
    /// Override the collateral-token address. Defaults to the bundled
    /// mainnet XP address.
    #[arg(long, value_name = "0x...")]
    pub(crate) token: Option<String>,
    /// Account to inspect. Defaults to `--address` / `DEADEYE_ADDRESS`.
    #[arg(long, value_name = "0x...")]
    pub(crate) account: Option<String>,
}

/// `deadeye claim …`
#[derive(Debug, clap::Args)]
pub(crate) struct ClaimArgs {
    /// Market contract address.
    #[arg(value_name = "ADDRESS")]
    pub(crate) market: String,
    /// Trader to claim for (defaults to self).
    #[arg(long, value_name = "0x...")]
    pub(crate) trader: Option<String>,
    /// Force family.
    #[arg(long)]
    pub(crate) family: Option<FamilyArg>,
}

/// `deadeye admin …`
#[derive(Debug, Subcommand)]
pub(crate) enum AdminCmd {
    /// Settle a single market.
    Settle(AdminSettleArgs),
    /// Pause a market.
    Pause(AdminPauseArgs),
    /// Unpause a market.
    Unpause(AdminPauseArgs),
    /// Collect protocol fees.
    CollectFees(AdminCollectFeesArgs),
    /// Deploy an instance of a math-runtime class via the legacy UDC.
    ///
    /// Math runtime classes are pre-declared on mainnet, but no instances
    /// exist — consumers (e.g. cpi-arb) need an instance to do
    /// chain-faithful preflight. This command is idempotent: it caches
    /// successful deploys in `~/.config/deadeye/runtimes.toml` keyed by
    /// `(chain_id, family)` and re-uses them on subsequent invocations.
    ///
    /// # Examples
    ///
    /// ```text
    /// # Dry-run on mainnet — projects the deploy address without spending gas.
    /// deadeye admin deploy-math-runtime --family normal
    ///
    /// # Check that previously-cached runtimes are still alive on-chain.
    /// deadeye admin deploy-math-runtime --status
    ///
    /// # Real deploy (requires --confirm + DEADEYE_PRIVATE_KEY).
    /// deadeye admin deploy-math-runtime --family normal --confirm
    /// ```
    DeployMathRuntime(AdminDeployMathRuntimeArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DeployFamilyArg {
    /// Normal (Gaussian) market family.
    Normal,
    /// Lognormal market family.
    Lognormal,
    /// Multinoulli (categorical) market family.
    Multinoulli,
    /// Bivariate normal market family.
    Bivariate,
}

#[derive(Debug, clap::Args)]
pub(crate) struct AdminDeployMathRuntimeArgs {
    /// Market family whose math runtime should be deployed.
    /// Required unless `--status` is set.
    #[arg(long, value_name = "FAMILY")]
    pub(crate) family: Option<DeployFamilyArg>,
    /// Optional deterministic salt (hex felt). Defaults to a fresh random
    /// felt; pass the same salt across runs for a content-addressed deploy.
    #[arg(long, value_name = "FELT")]
    pub(crate) salt: Option<String>,
    /// Override the math-runtime class hash. Defaults to the canonical
    /// class hash for `(chain_id, family)` from the bundled deployment
    /// manifest (sepolia) or the pinned mainnet constants. Required on
    /// chains other than mainnet / sepolia.
    #[arg(long, value_name = "0x...")]
    pub(crate) class_hash: Option<String>,
    /// Required for a real on-chain deploy. Without it, the command is a
    /// dry-run: it prints the projected address + class hash and exits 0.
    #[arg(long)]
    pub(crate) confirm: bool,
    /// Query the local cache + verify each entry against the chain via
    /// `getClassHashAt`. Implies `--family` is optional.
    #[arg(long)]
    pub(crate) status: bool,
}

#[derive(Debug, clap::Args)]
pub(crate) struct AdminSettleArgs {
    /// Factory contract address.
    #[arg(long, value_name = "0x...")]
    pub(crate) factory: Option<String>,
    /// Market address.
    #[arg(value_name = "MARKET")]
    pub(crate) market: String,
    /// Market family (normal | lognormal | multinoulli | bivariate).
    #[arg(long)]
    pub(crate) family: FamilyArg,
    /// x* value for normal / lognormal markets (f64; settled at this point).
    #[arg(long)]
    pub(crate) x_star: Option<f64>,
    /// Outcome index for multinoulli (u32).
    #[arg(long)]
    pub(crate) outcome: Option<u32>,
    /// Comma-separated `x1,x2` point for bivariate.
    #[arg(long, value_name = "X1,X2")]
    pub(crate) point: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct AdminPauseArgs {
    /// Factory contract address.
    #[arg(long, value_name = "0x...")]
    pub(crate) factory: Option<String>,
    /// Market address.
    #[arg(value_name = "MARKET")]
    pub(crate) market: String,
}

#[derive(Debug, clap::Args)]
pub(crate) struct AdminCollectFeesArgs {
    /// Factory contract address.
    #[arg(long, value_name = "0x...")]
    pub(crate) factory: Option<String>,
    /// Market address.
    #[arg(value_name = "MARKET")]
    pub(crate) market: String,
    /// Recipient address.
    #[arg(long, value_name = "0x...")]
    pub(crate) recipient: String,
}

/// `deadeye watch …`
#[derive(Debug, clap::Args)]
pub(crate) struct WatchArgs {
    /// Market contract address.
    #[arg(value_name = "ADDRESS")]
    pub(crate) market: String,
    /// Force family.
    #[arg(long)]
    pub(crate) family: Option<FamilyArg>,
    /// Polling interval, milliseconds.
    #[arg(long, default_value_t = 1000)]
    pub(crate) poll_interval_ms: u64,
    /// Comma-separated `mean=...,variance=...` candidate to re-quote on
    /// each block; format depends on family.
    #[arg(long, value_name = "SPEC")]
    pub(crate) show_quote_for: Option<String>,
    /// Math runtime address (required for `--show-quote-for`).
    #[arg(long)]
    pub(crate) runtime: Option<String>,
    /// Stop after this many updates (used by tests; default: unlimited).
    #[arg(long, hide = true)]
    pub(crate) max_updates: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AccountCmd {
    /// Print the resolved profile, address, balance, and chain id.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye account show
    /// ```
    Show,
    /// List every saved wallet profile (name, address, network, deployed),
    /// so an agent can pick which account to trade from with `--profile`.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye account list --output json
    /// ```
    List,
    /// Deploy the active profile's account contract on-chain.
    ///
    /// A freshly-created wallet must be **funded with STRK and deployed**
    /// before it can send any transaction (claim-grant, trade, …). Run this
    /// after funding if onboarding's deploy step was skipped or failed. It is
    /// idempotent — a no-op if the account is already deployed.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye account deploy            # deploy the default wallet
    /// deadeye account deploy --profile alice
    /// ```
    Deploy,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MarketsCmd {
    /// List markets from the indexer.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye markets list --family normal --limit 10
    /// ```
    List {
        /// Filter by market family.
        #[arg(long, value_name = "FAMILY")]
        family: Option<FamilyArg>,
        /// Max rows to display.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Read a single market's on-chain state.
    ///
    /// Family is auto-detected by trying each family's `params()` read.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye markets show 0x53e5…0fcf4
    /// ```
    Show {
        /// Market contract address (`0x…`).
        #[arg(value_name = "ADDRESS")]
        address: String,
        /// Force a specific family (skip auto-detect).
        #[arg(long, value_name = "FAMILY")]
        family: Option<FamilyArg>,
    },
    /// Read indexer-side metadata (title, description, category).
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye markets info 0x53e5…0fcf4
    /// ```
    Info {
        /// Market contract address (`0x…`).
        #[arg(value_name = "ADDRESS")]
        address: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum PositionCmd {
    /// List a trader's open positions across the configured markets.
    ///
    /// Markets are sourced from the indexer; pass `--trader` to inspect
    /// someone else's book.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye position list --trader 0xabc…
    /// ```
    List {
        /// Trader address; defaults to the active profile's address.
        #[arg(long, value_name = "0x...")]
        trader: Option<String>,
        /// Restrict to a single family.
        #[arg(long, value_name = "FAMILY")]
        family: Option<FamilyArg>,
        /// Max markets to scan.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Read one trader's full position on a specific market.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye position show 0x53e5…0fcf4
    /// ```
    Show {
        /// Market contract address.
        #[arg(value_name = "ADDRESS")]
        market: String,
        /// Trader address; defaults to the active profile's address.
        #[arg(long, value_name = "0x...")]
        trader: Option<String>,
        /// Force a specific family.
        #[arg(long, value_name = "FAMILY")]
        family: Option<FamilyArg>,
    },
    /// Close a position via `sell_position` (Driver B write path).
    Sell(PositionSellArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCmd {
    /// Create or update `~/.config/deadeye/config.toml`.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye config init --profile sepolia
    /// ```
    Init {
        /// Name of the profile to create / update.
        #[arg(long, default_value = "sepolia")]
        profile: String,
        /// Address to associate with the profile (hex felt).
        #[arg(long, value_name = "0x...")]
        address: Option<String>,
        /// Override the Starknet RPC URL.
        #[arg(long, value_name = "URL")]
        rpc_url: Option<String>,
        /// Override the indexer URL.
        #[arg(long, value_name = "URL")]
        indexer_url: Option<String>,
        /// Make this the default profile.
        #[arg(long)]
        set_default: bool,
    },
    /// Print the resolved configuration (private key redacted).
    Show,
    /// List configured profiles.
    ProfileList,
    /// Set the default profile.
    ///
    /// # Example
    ///
    /// ```text
    /// deadeye config profile-use sepolia
    /// ```
    ProfileUse {
        /// Profile name to mark as default.
        #[arg(value_name = "NAME")]
        name: String,
    },
}

// ─── Update (self-update) ────────────────────────────────────────────────

/// `deadeye update …`
#[derive(Debug, clap::Args)]
pub(crate) struct UpdateArgs {
    /// Only check whether a newer release exists; don't install anything.
    #[arg(long)]
    pub(crate) check: bool,
    /// Installer URL to run when updating. Defaults to the branded endpoint.
    #[arg(long, value_name = "URL")]
    pub(crate) url: Option<String>,
}

// ─── Feedback (GitHub issue submission) ──────────────────────────────────

/// Kind of feedback — drives the title prefix and the standard label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FeedbackKind {
    /// A feature request / enhancement.
    Feature,
    /// A bug report.
    Bug,
    /// An idea / open-ended suggestion.
    Idea,
}

/// `deadeye feedback …`
#[derive(Debug, clap::Args)]
pub(crate) struct FeedbackArgs {
    /// Short issue title (a `[Feature]` / `[Bug]` prefix is added).
    #[arg(long)]
    pub(crate) title: String,
    /// The body: what you want and why. For a feature, describe the problem
    /// and a proposed solution; for a bug, steps + expected vs actual.
    #[arg(long)]
    pub(crate) body: String,
    /// Kind of feedback.
    #[arg(long, value_name = "KIND", default_value = "feature")]
    pub(crate) kind: FeedbackKind,
    /// Component this concerns (e.g. cli, forecast, wallet, trade, indexer).
    #[arg(long)]
    pub(crate) component: Option<String>,
    /// Extra GitHub label to apply (repeatable). Must already exist on the repo.
    #[arg(long = "label")]
    pub(crate) labels: Vec<String>,
    /// Target repository (`owner/name`). Defaults to the deadeye-rs repo or
    /// `DEADEYE_FEEDBACK_REPO`.
    #[arg(long, value_name = "OWNER/NAME", env = "DEADEYE_FEEDBACK_REPO")]
    pub(crate) repo: Option<String>,
    /// Print the issue title/labels/body that would be posted, without posting.
    #[arg(long)]
    pub(crate) dry_run: bool,
}

// ─── Forecast (superforecasting workspace) ───────────────────────────────

/// `deadeye forecast …`
#[derive(Debug, Subcommand)]
pub(crate) enum ForecastCmd {
    /// Create a forecast workspace for a market.
    New(ForecastNewArgs),
    /// List markets that have a forecast workspace.
    List,
    /// Show a workspace: question, evidence, base rates, snapshot, next step.
    Show {
        /// Market contract address.
        #[arg(value_name = "MARKET")]
        market: String,
    },
    /// Append a timestamped evidence item.
    Evidence(ForecastEvidenceAddArgs),
    /// Add a reference class (base rate) to the prior.
    BaseRate(ForecastBaseRateAddArgs),
    /// Blend the recorded reference classes into a prior (prints the result).
    BlendBaseRates {
        /// Market contract address.
        #[arg(value_name = "MARKET")]
        market: String,
    },
    /// Commit the curated `(mean, σ)` snapshot for the market.
    Snapshot(ForecastSnapshotArgs),
    /// Run a Bayesian / aggregation routine (JSON in, JSON + rationale out).
    Bayes(ForecastBayesArgs),
}

/// Stance of an evidence item (CLI shadow of `ledger::Stance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum StanceArg {
    /// Pushes the outcome up / supports.
    Up,
    /// Pushes the outcome down / against.
    Down,
    /// Background context.
    Context,
    /// Mixed signal.
    Mixed,
}

#[derive(Debug, clap::Args)]
pub(crate) struct ForecastNewArgs {
    /// Market contract address (the workspace key).
    #[arg(value_name = "MARKET")]
    pub(crate) market: String,
    /// Human question / market title. If omitted, pulled from the indexer.
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// What outcome resolves the market.
    #[arg(long)]
    pub(crate) resolution: Option<String>,
    /// Lower bound of the outcome range.
    #[arg(long)]
    pub(crate) lower: Option<f64>,
    /// Upper bound of the outcome range.
    #[arg(long)]
    pub(crate) upper: Option<f64>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct ForecastEvidenceAddArgs {
    /// Market contract address.
    #[arg(value_name = "MARKET")]
    pub(crate) market: String,
    /// The headline claim.
    #[arg(long)]
    pub(crate) claim: String,
    /// Which way it points.
    #[arg(long, value_name = "STANCE")]
    pub(crate) stance: StanceArg,
    /// Source label (e.g. `FRED CPIAUCSL`).
    #[arg(long)]
    pub(crate) source: Option<String>,
    /// Source URL.
    #[arg(long)]
    pub(crate) url: Option<String>,
    /// Source reliability `[0, 1]`.
    #[arg(long)]
    pub(crate) reliability: Option<f64>,
    /// Relevance to the question `[0, 1]`.
    #[arg(long)]
    pub(crate) relevance: Option<f64>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct ForecastBaseRateAddArgs {
    /// Market contract address.
    #[arg(value_name = "MARKET")]
    pub(crate) market: String,
    /// Name / description of the reference class.
    #[arg(long)]
    pub(crate) name: String,
    /// Class base rate (probability or numeric anchor).
    #[arg(long)]
    pub(crate) rate: f64,
    /// Applicability to this question `[0, 1]`.
    #[arg(long, default_value_t = 1.0)]
    pub(crate) applicability: f64,
    /// Within-class uncertainty `>= 0`.
    #[arg(long, default_value_t = 0.0)]
    pub(crate) uncertainty: f64,
}

#[derive(Debug, clap::Args)]
pub(crate) struct ForecastSnapshotArgs {
    /// Market contract address.
    #[arg(value_name = "MARKET")]
    pub(crate) market: String,
    /// Forecast mean (μ).
    #[arg(long)]
    pub(crate) mean: f64,
    /// Forecast standard deviation (σ).
    #[arg(long)]
    pub(crate) sd: f64,
    /// Aggregation / pooling method label.
    #[arg(long, default_value = "manual")]
    pub(crate) method: String,
    /// Prose rationale.
    #[arg(long, default_value = "")]
    pub(crate) rationale: String,
    /// A reason the forecast could move up (repeatable).
    #[arg(long = "reason-up")]
    pub(crate) reason_up: Vec<String>,
    /// A reason the forecast could move down (repeatable).
    #[arg(long = "reason-down")]
    pub(crate) reason_down: Vec<String>,
    /// What would change your mind (repeatable).
    #[arg(long = "change-my-mind")]
    pub(crate) change_my_mind: Vec<String>,
}

/// Bayesian / aggregation routines exposed by `forecast bayes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum BayesRoutine {
    /// Aggregate component `(mu, sigma, weight, side)` beliefs → `(mean, sd, variance, quantiles)`.
    AggregateNormal,
    /// Blend reference classes `[{base_rate, applicability, uncertainty}]` → prior.
    BlendBaseRates,
    /// Pool probabilities `[{p, weight}]` via log-odds (default) or linear.
    Pool,
    /// Convert qualitative evidence → likelihood ratio.
    EvidenceWeight,
    /// Apply likelihood ratios to a prior: `{prior, lrs:[...]}`.
    LrUpdate,
    /// De-vig a binary market: `{yes, no}`.
    Devig,
    /// Effective independent count of a correlated cluster: `{n, rho}`.
    EffectiveCount,
    /// P(outcome ≤ x) under a normal: `{x, mean, sd}`.
    ProbBelow,
}

#[derive(Debug, clap::Args)]
pub(crate) struct ForecastBayesArgs {
    /// Routine to run.
    #[arg(value_name = "ROUTINE")]
    pub(crate) routine: BayesRoutine,
    /// JSON input. If omitted, read from stdin.
    #[arg(long, value_name = "JSON")]
    pub(crate) input: Option<String>,
    /// Emit only JSON (no human rationale line).
    #[arg(long)]
    pub(crate) json: bool,
}

impl StanceArg {
    pub(crate) const fn into_ledger(self) -> crate::forecast::ledger::Stance {
        use crate::forecast::ledger::Stance;
        match self {
            Self::Up => Stance::Up,
            Self::Down => Stance::Down,
            Self::Context => Stance::Context,
            Self::Mixed => Stance::Mixed,
        }
    }
}

/// CLI-facing family enum. Mirrors [`deadeye_sdk::bulk::Family`] without
/// inheriting its derives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FamilyArg {
    Normal,
    Lognormal,
    Multinoulli,
    Bivariate,
}

impl FamilyArg {
    pub(crate) fn as_indexer_slug(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Lognormal => "lognormal",
            Self::Multinoulli => "multinoulli",
            Self::Bivariate => "bivariate",
        }
    }

    pub(crate) fn as_sdk(self) -> deadeye_sdk::bulk::Family {
        match self {
            Self::Normal => deadeye_sdk::bulk::Family::Normal,
            Self::Lognormal => deadeye_sdk::bulk::Family::Lognormal,
            Self::Multinoulli => deadeye_sdk::bulk::Family::Multinoulli,
            Self::Bivariate => deadeye_sdk::bulk::Family::Bivariate,
        }
    }
}

impl DeployFamilyArg {
    /// Convert to the [`deadeye_deployer::runtime::Family`] enum.
    pub(crate) const fn as_deployer(self) -> deadeye_deployer::runtime::Family {
        match self {
            Self::Normal => deadeye_deployer::runtime::Family::Normal,
            Self::Lognormal => deadeye_deployer::runtime::Family::Lognormal,
            Self::Multinoulli => deadeye_deployer::runtime::Family::Multinoulli,
            Self::Bivariate => deadeye_deployer::runtime::Family::Bivariate,
        }
    }
}
