//! `deadeye admin …` — factory-owner write paths (settle / pause /
//! unpause / collect-fees / deploy-math-runtime).

use anyhow::{Context as _, Result};
use deadeye_core::{Sq128, bivariate::BivariatePointRaw};
use deadeye_sdk::bulk::Family;
use deadeye_starknet::{FactoryReader, FactoryWriter};

use crate::{
    cli::{
        AdminCmd, AdminCollectFeesArgs, AdminDeployMathRuntimeArgs, AdminPauseArgs,
        AdminSettleArgs, FamilyArg,
    },
    commands::{
        render_helpers::{submission_from_receipt, submission_from_trade_error},
        runtime_resolver::{build_owned_account, build_provider, parse_felt},
    },
    context::AppContext,
    output::OutputMode,
};

pub(crate) async fn run(action: AdminCmd, ctx: &AppContext, confirm: bool) -> Result<()> {
    match action {
        AdminCmd::Settle(args) => settle(ctx, args, confirm).await,
        AdminCmd::Pause(args) => pause(ctx, args, confirm, /* unpause */ false).await,
        AdminCmd::Unpause(args) => pause(ctx, args, confirm, /* unpause */ true).await,
        AdminCmd::CollectFees(args) => collect_fees(ctx, args, confirm).await,
        AdminCmd::DeployMathRuntime(args) => deploy_math_runtime(ctx, args).await,
    }
}

fn resolve_factory(arg: &Option<String>) -> Result<deadeye_starknet::Felt> {
    if let Some(s) = arg {
        return parse_felt("factory address", s);
    }
    let env = std::env::var("DEADEYE_FACTORY_ADDR")
        .context("factory address required: pass --factory 0x... or set DEADEYE_FACTORY_ADDR")?;
    parse_felt("factory address", &env)
}

async fn settle(ctx: &AppContext, args: AdminSettleArgs, confirm: bool) -> Result<()> {
    let factory = resolve_factory(&args.factory)?;
    let market = parse_felt("market address", &args.market)?;
    let provider = build_provider(ctx)?;
    let account = build_owned_account(ctx)?;
    let writer = FactoryWriter::new(FactoryReader::new(provider, factory), account);

    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!(
            "About to settle {:?} market {market:#x} via factory {factory:#x}.",
            args.family
        );
        super::confirm_or_bail("Continue?")?;
    }

    let market_hex = format!("{market:#x}");
    let family = args.family;
    let result = match family {
        FamilyArg::Normal => {
            let x = args
                .x_star
                .context("`--x-star <f64>` is required for normal settle")?;
            let x_star = Sq128::from_f64(x)?.to_raw();
            match writer.settle_normal_market(market, x_star).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
        FamilyArg::Lognormal => {
            let x = args
                .x_star
                .context("`--x-star <f64>` is required for lognormal settle")?;
            let x_star = Sq128::from_f64(x)?.to_raw();
            match writer.settle_lognormal_market(market, x_star).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
        FamilyArg::Multinoulli => {
            let outcome = args
                .outcome
                .context("`--outcome <u32>` is required for multinoulli settle")?;
            match writer.settle_multinoulli_market(market, outcome).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
        FamilyArg::Bivariate => {
            let raw = args
                .point
                .context("`--point X1,X2` is required for bivariate settle")?;
            let (x1_str, x2_str) = raw
                .split_once(',')
                .context("`--point` must be of the form `X1,X2`")?;
            let x1: f64 = x1_str.trim().parse().context("parsing point.x1")?;
            let x2: f64 = x2_str.trim().parse().context("parsing point.x2")?;
            let point = BivariatePointRaw {
                x1: Sq128::from_f64(x1)?.to_raw(),
                x2: Sq128::from_f64(x2)?.to_raw(),
            };
            match writer.settle_bivariate_market(market, point).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
    };
    ctx.renderer.print(&result)
}

async fn pause(ctx: &AppContext, args: AdminPauseArgs, confirm: bool, unpause: bool) -> Result<()> {
    let factory = resolve_factory(&args.factory)?;
    let market = parse_felt("market address", &args.market)?;
    let provider = build_provider(ctx)?;
    let account = build_owned_account(ctx)?;
    let writer = FactoryWriter::new(FactoryReader::new(provider, factory), account);

    let action_name = if unpause { "unpause" } else { "pause" };
    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!("About to {action_name} market {market:#x} via factory {factory:#x}.");
        super::confirm_or_bail("Continue?")?;
    }

    let market_hex = format!("{market:#x}");
    let outcome = if unpause {
        writer.unpause_market_typed(market).await
    } else {
        writer.pause_market_typed(market).await
    };
    let result = match outcome {
        Ok(receipt) => submission_from_receipt(
            if unpause { "unpause" } else { "pause" },
            market_hex,
            receipt,
        ),
        Err(e) => {
            submission_from_trade_error(if unpause { "unpause" } else { "pause" }, market_hex, &e)
        },
    };
    ctx.renderer.print(&result)
}

async fn collect_fees(ctx: &AppContext, args: AdminCollectFeesArgs, confirm: bool) -> Result<()> {
    let factory = resolve_factory(&args.factory)?;
    let market = parse_felt("market address", &args.market)?;
    let recipient = parse_felt("recipient address", &args.recipient)?;
    let provider = build_provider(ctx)?;
    let account = build_owned_account(ctx)?;
    let writer = FactoryWriter::new(FactoryReader::new(provider, factory), account);

    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!(
            "About to collect protocol fees on market {market:#x} → recipient {recipient:#x}."
        );
        super::confirm_or_bail("Continue?")?;
    }

    let market_hex = format!("{market:#x}");
    let result = match writer.collect_protocol_fees(market, recipient).await {
        Ok(receipt) => submission_from_receipt("collect_fees", market_hex, receipt),
        Err(e) => submission_from_trade_error("collect_fees", market_hex, &e),
    };
    let _ = Family::Normal;
    ctx.renderer.print(&result)
}

// ─── deploy-math-runtime ──────────────────────────────────────────────

use deadeye_deployer::runtime::{
    ChainKey, Family as DeployerFamily, RuntimeCache, RuntimeEntry, legacy_udc_address,
    projected_deploy_address, runtime_class_hash,
};
use rand::RngCore as _;
use serde::Serialize;
use starknet_contract::{ContractFactory, UdcSelector};
use starknet_core::types::{BlockId, BlockTag, Felt};
use starknet_providers::Provider as _;

use crate::output::Render;

/// Result row for a single math-runtime deploy / status check.
#[derive(Debug, Clone, Serialize)]
struct DeployMathRuntimeResult {
    /// Either "deploy", "dry_run", or "status".
    mode: &'static str,
    /// Chain slug (`mainnet`, `devnet`).
    chain: &'static str,
    /// Family slug (`normal`, ...).
    family: &'static str,
    /// Hex address of the runtime instance.
    address: String,
    /// Hex class hash that was deployed.
    class_hash: String,
    /// Whether the cache already contained this address (idempotency hit).
    cached: bool,
    /// Whether `getClassHashAt(address)` returned `class_hash`.
    on_chain_verified: bool,
    /// Hex tx hash, when we actually submitted a deploy.
    tx_hash: Option<String>,
    /// Hex salt used (always present, even for cache hits).
    salt: String,
    /// One-line note: next steps, hints, etc.
    note: Option<String>,
}

impl Render for DeployMathRuntimeResult {
    fn render_pretty(&self, r: &crate::output::Renderer) {
        if self.on_chain_verified {
            r.success(&format!("math-runtime/{} {}", self.family, self.mode));
        } else {
            r.warning(&format!(
                "math-runtime/{} {} (unverified)",
                self.family, self.mode
            ));
        }
        r.kv("chain", self.chain);
        r.kv("family", self.family);
        r.kv("address", &self.address);
        r.kv("class_hash", &self.class_hash);
        r.kv("salt", &self.salt);
        r.kv("cached", &self.cached.to_string());
        r.kv("on_chain_verified", &self.on_chain_verified.to_string());
        if let Some(h) = &self.tx_hash {
            r.kv("tx_hash", h);
        }
        if let Some(n) = &self.note {
            r.kv("note", n);
        }
    }

    fn render_plain(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        writeln!(w, "mode: {}", self.mode)?;
        writeln!(w, "chain: {}", self.chain)?;
        writeln!(w, "family: {}", self.family)?;
        writeln!(w, "address: {}", self.address)?;
        writeln!(w, "class_hash: {}", self.class_hash)?;
        writeln!(w, "salt: {}", self.salt)?;
        writeln!(w, "cached: {}", self.cached)?;
        writeln!(w, "on_chain_verified: {}", self.on_chain_verified)?;
        if let Some(h) = &self.tx_hash {
            writeln!(w, "tx_hash: {h}")?;
        }
        if let Some(n) = &self.note {
            writeln!(w, "note: {n}")?;
        }
        Ok(())
    }
}

fn fresh_random_salt() -> Felt {
    let mut rng = rand::thread_rng();
    let mut bytes = [0_u8; 32];
    rng.fill_bytes(&mut bytes);
    // Mask the high byte so the felt stays inside the prime field.
    bytes[0] &= 0x07;
    Felt::from_bytes_be(&bytes)
}

fn parse_salt(raw: &str) -> Result<Felt> {
    parse_felt("salt", raw)
}

async fn deploy_math_runtime(ctx: &AppContext, args: AdminDeployMathRuntimeArgs) -> Result<()> {
    // Resolve chain key from the configured chain id.
    let chain = ChainKey::from_chain_id_hex(&ctx.config.chain_id);

    if args.status {
        return deploy_math_runtime_status(ctx, chain).await;
    }

    let family = args
        .family
        .context(
            "`--family <normal|lognormal|multinoulli|bivariate>` is required (or use `--status`)",
        )?
        .as_deployer();

    // Resolve class hash: CLI override > canonical mainnet.
    let class_hash = if let Some(raw) = args.class_hash.as_deref() {
        parse_felt("class_hash", raw)?
    } else {
        runtime_class_hash(chain, family).with_context(|| {
            format!(
                "no canonical class hash for chain={} family={}; pass --class-hash 0x...",
                chain.slug(),
                family.slug()
            )
        })?
    };

    let salt = if let Some(raw) = args.salt.as_deref() {
        parse_salt(raw)?
    } else {
        fresh_random_salt()
    };

    // Load cache + see if a previous deploy is already recorded.
    let cache_path = RuntimeCache::default_path()
        .context("cannot determine runtime cache path; set DEADEYE_RUNTIMES_PATH")?;
    let mut cache = RuntimeCache::load(&cache_path).context("loading runtime cache")?;

    // ── Idempotency check via on-chain class hash ──
    if let Some(entry) = cache.get(chain, family).cloned() {
        let cached_addr = parse_felt("cached address", &entry.address)?;
        let cached_hash = parse_felt("cached class_hash", &entry.class_hash)?;
        let provider = build_provider(ctx)?;
        let on_chain = provider
            .inner()
            .get_class_hash_at(BlockId::Tag(BlockTag::Latest), cached_addr)
            .await
            .ok();
        if on_chain == Some(cached_hash) {
            // Idempotent fast-path — already deployed, verified alive.
            let result = DeployMathRuntimeResult {
                mode: if args.confirm { "deploy" } else { "dry_run" },
                chain: chain.slug(),
                family: family.slug(),
                address: entry.address.clone(),
                class_hash: entry.class_hash.clone(),
                cached: true,
                on_chain_verified: true,
                tx_hash: entry.deployed_tx.clone(),
                salt: format!("{salt:#x}"),
                note: Some(format!(
                    "already deployed; set {}={} in your consumer's .env",
                    family.env_var_name(),
                    entry.address
                )),
            };
            return ctx.renderer.print(&result);
        }
        // Cache hit but chain drift — fall through to re-deploy.
    }

    // Project the deploy address from (class_hash, salt, deployer=admin).
    // For unique=false the deployer doesn't enter the hash, but we still
    // need the admin address for the OwnedAccount path on a real deploy.
    let projected = projected_deploy_address(class_hash, salt, legacy_udc_address());

    if !args.confirm {
        // ── Dry-run path ──
        let result = DeployMathRuntimeResult {
            mode: "dry_run",
            chain: chain.slug(),
            family: family.slug(),
            address: format!("{projected:#x}"),
            class_hash: format!("{class_hash:#x}"),
            cached: false,
            on_chain_verified: false,
            tx_hash: None,
            salt: format!("{salt:#x}"),
            note: Some(format!(
                "dry-run only; rerun with --confirm + DEADEYE_PRIVATE_KEY set to deploy via UDC {}",
                deadeye_deployer::runtime::LEGACY_UDC_ADDRESS_HEX,
            )),
        };
        return ctx.renderer.print(&result);
    }

    // ── Real deploy path ──
    let account = build_owned_account(ctx)?;

    // Surface a confirmation prompt on a TTY when the global --confirm
    // wasn't passed. The local --confirm is a separate, mandatory flag;
    // the prompt is a second layer of intentionality for mainnet runs.
    if std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!(
            "About to deploy math-runtime {family} on chain={chain} via UDC.\n  class_hash: {class_hash:#x}\n  salt:       {salt:#x}\n  projected:  {projected:#x}",
            family = family.slug(),
            chain = chain.slug(),
        );
        super::confirm_or_bail("Continue? This spends gas.")?;
    }

    // Build a ContractFactory using the inner SingleOwnerAccount.
    let factory = ContractFactory::new_with_udc(class_hash, account.inner(), UdcSelector::Legacy);
    let deployment = factory.deploy_v3(vec![], salt, false);
    let actual_address = deployment.deployed_address();
    debug_assert_eq!(actual_address, projected, "address derivation mismatch");

    let tx = deployment
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("UDC deploy submission failed: {e}"))?;

    // Poll for `getClassHashAt` until the class is visible at the new
    // address — this is our success criterion. Bounded to a few seconds.
    let provider = build_provider(ctx)?;
    let mut verified = false;
    let mut deployed_block: Option<u64> = None;
    for _ in 0..30_u32 {
        if let Ok(h) = provider
            .inner()
            .get_class_hash_at(BlockId::Tag(BlockTag::PreConfirmed), actual_address)
            .await
            && h == class_hash
        {
            verified = true;
            deployed_block = provider.inner().block_number().await.ok();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    let address_hex = format!("{actual_address:#x}");
    let class_hex = format!("{class_hash:#x}");
    let tx_hex = format!("{:#x}", tx.transaction_hash);

    if verified {
        cache.upsert(chain, family, RuntimeEntry {
            address: address_hex.clone(),
            class_hash: class_hex.clone(),
            deployed_at_block: deployed_block,
            deployed_tx: Some(tx_hex.clone()),
        });
        cache.save(&cache_path).context("saving runtime cache")?;
    }

    let result = DeployMathRuntimeResult {
        mode: "deploy",
        chain: chain.slug(),
        family: family.slug(),
        address: address_hex.clone(),
        class_hash: class_hex,
        cached: false,
        on_chain_verified: verified,
        tx_hash: Some(tx_hex),
        salt: format!("{salt:#x}"),
        note: Some(format!(
            "set {}={} in your consumer's .env",
            family.env_var_name(),
            address_hex
        )),
    };
    ctx.renderer.print(&result)
}

async fn deploy_math_runtime_status(ctx: &AppContext, chain: ChainKey) -> Result<()> {
    let cache_path = RuntimeCache::default_path()
        .context("cannot determine runtime cache path; set DEADEYE_RUNTIMES_PATH")?;
    let cache = RuntimeCache::load(&cache_path).context("loading runtime cache")?;

    if cache.chains.is_empty() {
        let result = DeployMathRuntimeResult {
            mode: "status",
            chain: chain.slug(),
            family: "-",
            address: String::new(),
            class_hash: String::new(),
            cached: false,
            on_chain_verified: false,
            tx_hash: None,
            salt: String::new(),
            note: Some(format!(
                "no cached runtimes; cache file is at {}",
                cache_path.display()
            )),
        };
        return ctx.renderer.print(&result);
    }

    let provider = build_provider(ctx)?;

    let mut rows: Vec<DeployMathRuntimeResult> = Vec::new();
    let mut any_drift = false;

    for (chain_slug, family_map) in &cache.chains {
        for (family_slug, entry) in family_map {
            let family =
                DeployerFamily::from_slug(family_slug).context("malformed family slug in cache")?;
            let addr = parse_felt("address", &entry.address)?;
            let expected_class = parse_felt("class_hash", &entry.class_hash)?;
            let on_chain = provider
                .inner()
                .get_class_hash_at(BlockId::Tag(BlockTag::Latest), addr)
                .await
                .ok();
            let verified = on_chain == Some(expected_class);
            if !verified {
                any_drift = true;
            }
            rows.push(DeployMathRuntimeResult {
                mode: "status",
                chain: static_chain_slug(chain_slug),
                family: family.slug(),
                address: entry.address.clone(),
                class_hash: entry.class_hash.clone(),
                cached: true,
                on_chain_verified: verified,
                tx_hash: entry.deployed_tx.clone(),
                salt: String::new(),
                note: if verified {
                    None
                } else {
                    Some(format!(
                        "drift! getClassHashAt returned {} expected {}",
                        on_chain
                            .map(|h| format!("{h:#x}"))
                            .unwrap_or_else(|| "<error>".to_owned()),
                        entry.class_hash
                    ))
                },
            });
        }
    }

    ctx.renderer.print_table(&rows)?;
    if any_drift {
        anyhow::bail!("one or more cached runtime entries drifted from on-chain state");
    }
    Ok(())
}

/// Map a chain slug string back to the static slug we use for output.
fn static_chain_slug(s: &str) -> &'static str {
    match s {
        "mainnet" => "mainnet",
        "devnet" => "devnet",
        _ => "unknown",
    }
}
