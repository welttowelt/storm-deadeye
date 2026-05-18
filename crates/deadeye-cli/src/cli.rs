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
