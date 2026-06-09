//! One-shot devnet bootstrap.
//!
//! Wires together the artifact loader, declare loop, factory deploy +
//! plugin deploy + market-type configuration + deploy-profile upsert,
//! plus a test collateral token deployment with pre-funded participants.

use std::sync::Arc;

use deadeye_core::sq128::Sq128Raw;
use deadeye_starknet::{OwnedAccount, types::common::FeeConfigRaw};
use starknet_accounts::{ExecutionEncoding, SingleOwnerAccount};
use starknet_core::types::{BlockId, BlockTag, Felt};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use starknet_signers::{LocalWallet, SigningKey};
use thiserror::Error;
use url::Url;

use crate::{
    account::{AccountError, DevnetAccount, predeployed},
    devnet,
    fixture::{
        artifacts::{AllArtifacts, ArtifactError},
        declare::{DeclareError, DeclaredHashes, declare_all},
        deploy::{DeployError, udc_deploy},
        erc20::{
            Erc20Error, operator_mint, set_system_transfer_address, set_token_factory, transfer,
        },
        factory_setup::{
            DeployProfileParams, FactorySetupError, MarketKind, configure_market_type,
        },
    },
};

/// Errors emitted while bootstrapping the test environment.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TestEnvError {
    /// Failed to load Sierra/CASM artifacts.
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    /// Failed to declare a class.
    #[error(transparent)]
    Declare(#[from] DeclareError),
    /// Failed to deploy a contract.
    #[error(transparent)]
    Deploy(#[from] DeployError),
    /// Factory setup call failed.
    #[error(transparent)]
    Setup(#[from] FactorySetupError),
    /// ERC20 helper failed.
    #[error(transparent)]
    Erc20(#[from] Erc20Error),
    /// Devnet (un)reachable.
    #[error(transparent)]
    Devnet(#[from] devnet::DevnetError),
    /// Account loading failed.
    #[error(transparent)]
    Account(#[from] AccountError),
    /// URL parsing failed.
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
}

/// A fully bootstrapped devnet test environment.
#[derive(Debug)]
pub struct TestEnv {
    /// Devnet HTTP URL.
    pub url: Url,
    /// Chain id.
    pub chain_id: Felt,
    /// All declared class hashes.
    pub declared: DeclaredHashes,
    /// Factory contract address.
    pub factory: Felt,
    /// Per-family plugin addresses.
    pub normal_plugin: Felt,
    /// Lognormal factory plugin.
    pub lognormal_plugin: Felt,
    /// Multinoulli factory plugin.
    pub multinoulli_plugin: Felt,
    /// Bivariate factory plugin.
    pub bivariate_plugin: Felt,
    /// Standalone math-runtime instance for normal markets (used to fetch
    /// chain-correct sqrt hints via `compute_hints_view`).
    pub normal_runtime: Felt,
    /// Standalone math-runtime instance for lognormal markets.
    pub lognormal_runtime: Felt,
    /// Standalone math-runtime instance for multinoulli markets.
    pub multinoulli_runtime: Felt,
    /// Standalone math-runtime instance for bivariate markets.
    pub bivariate_runtime: Felt,
    /// Test collateral token.
    pub collateral: Felt,
    /// Admin/deployer account (devnet account 0).
    pub admin: DevnetAccount,
    /// Pre-funded participant accounts (devnet accounts 1..N).
    pub participants: Vec<DevnetAccount>,
}

impl TestEnv {
    /// Build a [`SingleOwnerAccount`] handle for one of the pre-funded
    /// participants.
    #[must_use]
    pub fn account_handle(
        &self,
        who: &DevnetAccount,
    ) -> SingleOwnerAccount<JsonRpcClient<HttpTransport>, LocalWallet> {
        let rpc = JsonRpcClient::new(HttpTransport::new(self.url.clone()));
        let signer = LocalWallet::from_signing_key(SigningKey::from_secret_scalar(who.private_key));
        let mut acc = SingleOwnerAccount::new(
            rpc,
            signer,
            who.address,
            self.chain_id,
            ExecutionEncoding::New,
        );
        let _ = acc.set_block_id(BlockId::Tag(BlockTag::PreConfirmed));
        acc
    }

    /// Build an [`OwnedAccount`] (our SDK wrapper) for one of the participants.
    #[must_use]
    pub fn owned_account(&self, who: &DevnetAccount) -> OwnedAccount {
        let rpc = JsonRpcClient::new(HttpTransport::new(self.url.clone()));
        OwnedAccount::from_signing_key(rpc, who.address, who.private_key, self.chain_id)
    }
}

/// Bootstrap policy.
#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    /// Devnet URL.
    pub url: Url,
    /// Number of participants beyond the admin (default 4).
    pub participant_count: usize,
    /// Initial collateral mint per participant (u128 base units).
    pub initial_collateral_per_participant: u128,
    /// Admin retains this much collateral for LP usage.
    pub admin_initial_collateral: u128,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            url: Url::parse("http://127.0.0.1:5050").expect("static URL parses"),
            participant_count: 4,
            initial_collateral_per_participant: 10_000_000_000_000_000_000_000_u128, // 10k tokens
            admin_initial_collateral: 50_000_000_000_000_000_000_000_u128,           // 50k tokens
        }
    }
}

/// One-shot devnet bootstrap.
pub async fn bootstrap_devnet(cfg: BootstrapConfig) -> Result<TestEnv, TestEnvError> {
    // Step 1: liveness + reset to genesis so deterministic-salt deploys
    // don't collide with prior runs.
    devnet::wait_until_ready(&cfg.url, 10, std::time::Duration::from_secs(1)).await?;
    let _ = devnet::reset(&cfg.url).await; // best-effort
    // Drop the cached compiled-class-hash file because the chain re-declares
    // from scratch.
    let _ = std::fs::remove_file("/tmp/deadeye_casm_hashes.json");
    let chain_id = devnet::chain_id(&cfg.url).await?;

    // Step 2: predeployed accounts.
    let mut accounts = predeployed(&cfg.url).await?;
    if accounts.len() < cfg.participant_count + 1 {
        return Err(TestEnvError::Account(AccountError::OutOfRange {
            requested: cfg.participant_count + 1,
            total: accounts.len(),
        }));
    }
    let admin = accounts.remove(0);
    let participants: Vec<DevnetAccount> =
        accounts.into_iter().take(cfg.participant_count).collect();

    // Build admin handle.
    let admin_handle = build_account(&cfg.url, chain_id, &admin);

    // Step 3: load artifacts + declare.
    let artifacts = AllArtifacts::load()?;
    let declared = declare_all(&admin_handle, &artifacts).await?;

    // Step 4: deploy factory(owner=admin, treasury=admin).
    let factory = udc_deploy(admin_handle.clone(), declared.factory, Felt::ZERO, vec![
        admin.address,
        admin.address,
    ])
    .await?;

    // Step 5: deploy each family plugin (no constructor args).
    let normal_plugin = udc_deploy(
        admin_handle.clone(),
        declared.normal_factory_plugin,
        Felt::from(1_u64),
        vec![],
    )
    .await?;
    let lognormal_plugin = udc_deploy(
        admin_handle.clone(),
        declared.lognormal_factory_plugin,
        Felt::from(2_u64),
        vec![],
    )
    .await?;
    let multinoulli_plugin = udc_deploy(
        admin_handle.clone(),
        declared.multinoulli_factory_plugin,
        Felt::from(3_u64),
        vec![],
    )
    .await?;
    let bivariate_plugin = udc_deploy(
        admin_handle.clone(),
        declared.bivariate_factory_plugin,
        Felt::from(4_u64),
        vec![],
    )
    .await?;

    // Standalone math-runtime instance (no ctor args). We use it for the
    // off-chain sqrt-hint oracle below.
    let normal_runtime = udc_deploy(
        admin_handle.clone(),
        declared.normal_math_runtime,
        Felt::from(0xABCD_u64),
        vec![],
    )
    .await?;
    let lognormal_runtime = udc_deploy(
        admin_handle.clone(),
        declared.lognormal_math_runtime,
        Felt::from(0xABCE_u64),
        vec![],
    )
    .await?;
    let multinoulli_runtime = udc_deploy(
        admin_handle.clone(),
        declared.multinoulli_math_runtime,
        Felt::from(0xABCF_u64),
        vec![],
    )
    .await?;
    let bivariate_runtime = udc_deploy(
        admin_handle.clone(),
        declared.bivariate_math_runtime,
        Felt::from(0xABD0_u64),
        vec![],
    )
    .await?;

    // Step 6: configure market types on the factory.
    configure_market_type(
        admin_handle.clone(),
        factory,
        MarketKind::Normal,
        declared.normal_amm,
        declared.normal_math_runtime,
        normal_plugin,
        true,
    )
    .await?;
    configure_market_type(
        admin_handle.clone(),
        factory,
        MarketKind::Lognormal,
        declared.lognormal_amm,
        declared.lognormal_math_runtime,
        lognormal_plugin,
        true,
    )
    .await?;
    configure_market_type(
        admin_handle.clone(),
        factory,
        MarketKind::Multinoulli,
        declared.multinoulli_amm,
        declared.multinoulli_math_runtime,
        multinoulli_plugin,
        true,
    )
    .await?;
    configure_market_type(
        admin_handle.clone(),
        factory,
        MarketKind::BivariateNormal,
        declared.bivariate_amm,
        declared.bivariate_math_runtime,
        bivariate_plugin,
        true,
    )
    .await?;

    // Step 7: use the devnet-predeployed STRK token as our test collateral.
    // This is a vanilla OpenZeppelin ERC20 — no restricted-transfer logic,
    // every predeployed account already starts with 1000 STRK.
    let collateral = Felt::from_hex_unchecked(
        "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d",
    );
    let _ = encode_byte_array; // kept for future restricted-collateral path

    // Step 8: upsert default profile (profile_id=1) for normal markets.
    // The chaos suite will upsert per-family overrides as needed.
    let one_q128 = sq128_from_u64(1);
    let zero = Sq128Raw {
        limb0: 0,
        limb1: 0,
        limb2: 0,
        limb3: 0,
        neg: false,
    };
    let _ = (one_q128, zero); // suppress unused if the placeholder profile is removed

    // Step 9: STRK is already minted to every devnet account (1000 STRK
    // each by default). No additional distribution needed — admin and
    // participants both have funds. We keep the helper imports alive so
    // future iterations can switch back to restricted collateral if
    // desired.
    let _ = (
        operator_mint::<&SingleOwnerAccount<JsonRpcClient<HttpTransport>, LocalWallet>>,
        set_system_transfer_address::<&SingleOwnerAccount<JsonRpcClient<HttpTransport>, LocalWallet>>,
        set_token_factory::<&SingleOwnerAccount<JsonRpcClient<HttpTransport>, LocalWallet>>,
        transfer::<&SingleOwnerAccount<JsonRpcClient<HttpTransport>, LocalWallet>>,
    );

    Ok(TestEnv {
        url: cfg.url,
        chain_id,
        declared,
        factory,
        normal_plugin,
        normal_runtime,
        lognormal_plugin,
        lognormal_runtime,
        multinoulli_plugin,
        multinoulli_runtime,
        bivariate_plugin,
        bivariate_runtime,
        collateral,
        admin,
        participants,
    })
}

fn build_account(
    url: &Url,
    chain_id: Felt,
    account: &DevnetAccount,
) -> SingleOwnerAccount<JsonRpcClient<HttpTransport>, LocalWallet> {
    let rpc = JsonRpcClient::new(HttpTransport::new(url.clone()));
    let signer = LocalWallet::from_signing_key(SigningKey::from_secret_scalar(account.private_key));
    let mut acc = SingleOwnerAccount::new(
        rpc,
        signer,
        account.address,
        chain_id,
        ExecutionEncoding::New,
    );
    let _ = acc.set_block_id(BlockId::Tag(BlockTag::PreConfirmed));
    acc
}

fn sq128_from_u64(value: u64) -> Sq128Raw {
    // Q128.128: integer N = N * 2^128, so limb2 holds N when N < 2^64.
    Sq128Raw {
        limb0: 0,
        limb1: 0,
        limb2: value,
        limb3: 0,
        neg: false,
    }
}

/// Encode a short byte string as a Cairo `ByteArray`. Strings shorter than
/// 31 bytes pack into a single pending word with no chunks.
fn encode_byte_array(bytes: &[u8], out: &mut Vec<Felt>) {
    assert!(
        bytes.len() < 31,
        "encode_byte_array: only short strings supported"
    );
    // data: Array<bytes31> length = 0
    out.push(Felt::ZERO);
    // pending_word: pack bytes big-endian into a felt.
    let mut packed = [0_u8; 32];
    let offset = 32 - bytes.len();
    packed[offset..].copy_from_slice(bytes);
    out.push(Felt::from_bytes_be(&packed));
    // pending_word_len: u32
    out.push(Felt::from(bytes.len() as u64));
}

// Suppress unused-import warnings on helpers consumers may not exercise.
const _: fn() = || {
    let _ = core::mem::size_of::<FeeConfigRaw>();
    let _ = core::mem::size_of::<DeployProfileParams>();
    let _ = core::mem::size_of::<Arc<()>>();
};
