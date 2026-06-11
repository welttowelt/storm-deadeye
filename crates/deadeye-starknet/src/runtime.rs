//! Thin view-call helpers for the per-family math runtime contracts.
//!
//! Every market type ships a dedicated math runtime that exposes
//! `compute_hints_view(dist)` (and friends). The AMM writers call into
//! these helpers when they need byte-exact hints — production bots want
//! `sell_position()` / `execute_quote()` to handle hint fetching
//! transparently, not to plumb a separate runtime address through every
//! call site.
//!
//! The functions here are deliberately tiny — calldata builders +
//! `Option<T>` decoders — so the higher layers can compose them inside
//! `quote_trade()` and `sell_position()` without duplicating selector
//! logic.

use deadeye_core::{
    bivariate::{
        BivariateNormalDistributionCoreRaw, BivariateNormalDistributionRaw,
        BivariateNormalSqrtHintsRaw, BivariatePointRaw,
    },
    categorical::{CategoricalDistributionRaw, CategoricalL2HintRaw},
    distribution::{LognormalDistributionRaw, NormalDistributionRaw, NormalSqrtHintsRaw},
    sq128::Sq128Raw,
};
use starknet_core::{
    types::{Felt, FunctionCall},
    utils::get_selector_from_name,
};

use crate::{
    cairo_serde::CairoSerde,
    error::{ContractError, ContractResult},
    provider::Provider,
    types::{
        lognormal::{LognormalSqrtHintsRaw, LognormalTradeCheckRaw},
        multinoulli::MultinoulliTradeCheckRaw,
        normal::TradeCheckRaw,
    },
};

/// `Felt::ZERO` (Cairo `Option::Some` discriminant) — the math runtimes
/// return `Option<HintsRaw>` so the success path is always
/// `[0, ...hints]`.
const OPTION_SOME: Felt = Felt::ZERO;

async fn call_runtime<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    selector_name: &'static str,
    calldata: Vec<Felt>,
) -> ContractResult<Vec<Felt>> {
    let selector =
        get_selector_from_name(selector_name).map_err(|e| ContractError::InvalidResponse {
            call: selector_name,
            message: format!("invalid selector name: {e}"),
        })?;
    provider
        .call(
            FunctionCall {
                contract_address: runtime,
                entry_point_selector: selector,
                calldata,
            },
            provider.default_block(),
        )
        .await
}

fn decode_some_or_err<T>(response: &[Felt], call_name: &'static str) -> ContractResult<T>
where
    T: CairoSerde,
{
    if response.first() != Some(&OPTION_SOME) {
        return Err(ContractError::InvalidResponse {
            call: call_name,
            message: "runtime returned Option::None — input rejected (most often an \
                       inconsistent (σ, σ²) encoding: build candidates so σ·σ == variance \
                       at Sq128 precision, e.g. via from_variance/from_sigma + to_raw)"
                .into(),
        });
    }
    let tail = response
        .get(1..)
        .ok_or_else(|| ContractError::InvalidResponse {
            call: call_name,
            message: "response too short".into(),
        })?;
    let (value, _rest) = T::decode(tail)?;
    Ok(value)
}

/// Calls `compute_hints_view(dist)` on a normal-AMM math runtime.
pub async fn compute_normal_hints<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    dist: NormalDistributionRaw,
) -> ContractResult<NormalSqrtHintsRaw> {
    let mut calldata = Vec::with_capacity(15);
    dist.encode(&mut calldata);
    let response = call_runtime(provider, runtime, "compute_hints_view", calldata).await?;
    decode_some_or_err(&response, "compute_hints_view")
}

/// Calls `check_trade_view(...)` on a normal math runtime and returns
/// the raw [`TradeCheckRaw`] verdict — `is_valid` + `rejection_reason`
/// reveal whether the chain would accept the candidate trade.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the Cairo ABI 1:1; collapsing into a struct would \
              hide the call site"
)]
pub async fn check_normal_trade<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    current: NormalDistributionRaw,
    candidate: NormalDistributionRaw,
    x_star: Sq128Raw,
    supplied_collateral: Sq128Raw,
    k: Sq128Raw,
    backing: Sq128Raw,
    tolerance: Sq128Raw,
    min_trade_collateral: Sq128Raw,
    current_hints: NormalSqrtHintsRaw,
    candidate_hints: NormalSqrtHintsRaw,
) -> ContractResult<TradeCheckRaw> {
    let mut calldata = Vec::with_capacity(80);
    current.encode(&mut calldata);
    candidate.encode(&mut calldata);
    x_star.encode(&mut calldata);
    supplied_collateral.encode(&mut calldata);
    k.encode(&mut calldata);
    backing.encode(&mut calldata);
    tolerance.encode(&mut calldata);
    min_trade_collateral.encode(&mut calldata);
    current_hints.encode(&mut calldata);
    candidate_hints.encode(&mut calldata);
    let response = call_runtime(provider, runtime, "check_trade_view", calldata).await?;
    let (check, _rest) = TradeCheckRaw::decode(&response)?;
    Ok(check)
}

/// Calls `check_trade_view(...)` on a lognormal math runtime.
#[expect(clippy::too_many_arguments, reason = "mirrors the Cairo ABI 1:1")]
pub async fn check_lognormal_trade<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    current: LognormalDistributionRaw,
    candidate: LognormalDistributionRaw,
    x_star: Sq128Raw,
    supplied_collateral: Sq128Raw,
    k: Sq128Raw,
    backing: Sq128Raw,
    tolerance: Sq128Raw,
    min_trade_collateral: Sq128Raw,
    current_hints: LognormalSqrtHintsRaw,
    candidate_hints: LognormalSqrtHintsRaw,
) -> ContractResult<LognormalTradeCheckRaw> {
    let mut calldata = Vec::with_capacity(80);
    current.encode(&mut calldata);
    candidate.encode(&mut calldata);
    x_star.encode(&mut calldata);
    supplied_collateral.encode(&mut calldata);
    k.encode(&mut calldata);
    backing.encode(&mut calldata);
    tolerance.encode(&mut calldata);
    min_trade_collateral.encode(&mut calldata);
    current_hints.encode(&mut calldata);
    candidate_hints.encode(&mut calldata);
    let response = call_runtime(provider, runtime, "check_trade_view", calldata).await?;
    let (check, _rest) = LognormalTradeCheckRaw::decode(&response)?;
    Ok(check)
}

/// Calls `compute_hints_view(dist)` on a lognormal math runtime.
pub async fn compute_lognormal_hints<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    dist: LognormalDistributionRaw,
) -> ContractResult<LognormalSqrtHintsRaw> {
    let mut calldata = Vec::with_capacity(15);
    dist.encode(&mut calldata);
    let response = call_runtime(provider, runtime, "compute_hints_view", calldata).await?;
    decode_some_or_err(&response, "compute_hints_view")
}

/// Calls `compute_hint_view(dist)` (or `compute_hints_view` if the
/// runtime exposes the plural form) on a multinoulli math runtime.
pub async fn compute_multinoulli_hint<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    dist: &CategoricalDistributionRaw,
) -> ContractResult<CategoricalL2HintRaw> {
    let mut calldata = Vec::with_capacity(2 + 5 * dist.probs.len());
    dist.encode(&mut calldata);
    for sel_name in ["compute_hint_view", "compute_hints_view"] {
        if let Ok(response) = call_runtime(provider, runtime, sel_name, calldata.clone()).await {
            return decode_some_or_err(&response, sel_name);
        }
    }
    Err(ContractError::InvalidResponse {
        call: "compute_hint_view",
        message: "no compute_hint(s)_view selector accepted".into(),
    })
}

/// Calls `expand_distribution_core_view(core_dist)` on a bivariate math
/// runtime.
///
/// The core dist's `sigma_i`, `inv_one_minus_rho_sq` and `normalization`
/// fields must come from the chain (Sq128 derivations) or the
/// constructor inside `BivariateNormalDistribution::new` will reject
/// them with `inv_one_minus_rho_sq_hint != expected_inv`.
pub async fn expand_bivariate_distribution<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    core_dist: BivariateNormalDistributionCoreRaw,
) -> ContractResult<BivariateNormalDistributionRaw> {
    let mut calldata = Vec::with_capacity(32);
    core_dist.encode(&mut calldata);
    let response =
        call_runtime(provider, runtime, "expand_distribution_core_view", calldata).await?;
    decode_some_or_err(&response, "expand_distribution_core_view")
}

/// Calls `compute_hints_view(dist)` on a bivariate math runtime. The
/// caller must pass a chain-correct full distribution (see
/// [`expand_bivariate_distribution`]).
pub async fn compute_bivariate_hints<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    dist: BivariateNormalDistributionRaw,
) -> ContractResult<BivariateNormalSqrtHintsRaw> {
    let mut calldata = Vec::with_capacity(64);
    dist.encode(&mut calldata);
    let response = call_runtime(provider, runtime, "compute_hints_view", calldata).await?;
    decode_some_or_err(&response, "compute_hints_view")
}

/// Calls `check_trade_view(...)` on a multinoulli math runtime.
///
/// Mirrors the Cairo ABI; returns the chain's structured verdict
/// (`is_valid` + `rejection_reason`) so `quote_trade` can give an
/// authoritative "will-accept" answer rather than relying on hint
/// availability alone.
#[expect(clippy::too_many_arguments, reason = "mirrors the Cairo ABI 1:1")]
pub async fn check_multinoulli_trade<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    current: CategoricalDistributionRaw,
    candidate: CategoricalDistributionRaw,
    min_outcome_index: u32,
    supplied_collateral: Sq128Raw,
    k: Sq128Raw,
    backing: Sq128Raw,
    tolerance: Sq128Raw,
    min_trade_collateral: Sq128Raw,
    current_hint: CategoricalL2HintRaw,
    candidate_hint: CategoricalL2HintRaw,
) -> ContractResult<MultinoulliTradeCheckRaw> {
    let mut calldata = Vec::with_capacity(64 + 5 * (current.probs.len() + candidate.probs.len()));
    current.encode(&mut calldata);
    candidate.encode(&mut calldata);
    min_outcome_index.encode(&mut calldata);
    supplied_collateral.encode(&mut calldata);
    k.encode(&mut calldata);
    backing.encode(&mut calldata);
    tolerance.encode(&mut calldata);
    min_trade_collateral.encode(&mut calldata);
    current_hint.encode(&mut calldata);
    candidate_hint.encode(&mut calldata);
    let response = call_runtime(provider, runtime, "check_trade_view", calldata).await?;
    let (check, _rest) = MultinoulliTradeCheckRaw::decode(&response)?;
    Ok(check)
}

/// Calls `check_trade_view(...)` on a bivariate math runtime. Returns the
/// chain's structured verdict — caller must supply chain-correct hints +
/// expanded candidate (see [`expand_bivariate_distribution`]).
#[expect(clippy::too_many_arguments, reason = "mirrors the Cairo ABI 1:1")]
pub async fn check_bivariate_trade<P: Provider + Sync>(
    provider: &P,
    runtime: Felt,
    current: BivariateNormalDistributionRaw,
    candidate: BivariateNormalDistributionRaw,
    x_star: BivariatePointRaw,
    supplied_collateral: Sq128Raw,
    k: Sq128Raw,
    backing: Sq128Raw,
    tolerance: Sq128Raw,
    min_trade_collateral: Sq128Raw,
    current_hints: BivariateNormalSqrtHintsRaw,
    candidate_hints: BivariateNormalSqrtHintsRaw,
) -> ContractResult<TradeCheckRaw> {
    let mut calldata = Vec::with_capacity(120);
    current.encode(&mut calldata);
    candidate.encode(&mut calldata);
    x_star.encode(&mut calldata);
    supplied_collateral.encode(&mut calldata);
    k.encode(&mut calldata);
    backing.encode(&mut calldata);
    tolerance.encode(&mut calldata);
    min_trade_collateral.encode(&mut calldata);
    current_hints.encode(&mut calldata);
    candidate_hints.encode(&mut calldata);
    let response = call_runtime(provider, runtime, "check_trade_view", calldata).await?;
    let (check, _rest) = TradeCheckRaw::decode(&response)?;
    Ok(check)
}
