//! Normal-distribution AMM market handle.
//!
//! Provides the "read-then-quote-then-execute" loop a market-maker drives
//! against a Gaussian AMM. The handle wraps [`deadeye_starknet`]'s
//! lower-level reader / writer pair and adds the off-chain collateral
//! solver — most callers want exactly this.
//!
//! ## Worked example — quote, execute, sell
//!
//! ```no_run
//! use deadeye_sdk::normal::NormalMarket;
//! use deadeye_sdk::starknet::{
//!     Felt, JsonRpcProvider, NormalMarketReader, NormalMarketWriter, OwnedAccount,
//! };
//! use deadeye_sdk::core::{distribution::NormalDistributionRaw, sq128::Sq128Raw};
//! use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?));
//! let provider = JsonRpcProvider::new(rpc);
//! let (market, runtime): (Felt, Felt) = (Felt::ZERO, Felt::ZERO);
//!
//! let reader = NormalMarketReader::new(&provider, market);
//! let signer = OwnedAccount::from_signing_key(
//!     JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?)),
//!     Felt::ZERO, Felt::ZERO, Felt::ZERO,
//! );
//! let writer = NormalMarketWriter::new(reader, signer);
//!
//! let candidate = NormalDistributionRaw {
//!     mean: Sq128Raw::ZERO, variance: Sq128Raw::ZERO, sigma: Sq128Raw::ZERO,
//! };
//! let quote = writer
//!     .reader()
//!     .quote_trade(runtime, candidate, Sq128Raw::ZERO, Sq128Raw::ZERO, Sq128Raw::ZERO)
//!     .await?;
//! if quote.on_chain_will_accept {
//!     writer.execute_quote(quote).await?;
//! }
//! writer.sell_position(runtime, 0).await?;
//! # Ok(()) }
//! ```

use deadeye_collateral::{MinimizationPolicy, normal_collateral};
use deadeye_core::{
    NormalDistribution, Sq128,
    distribution::NormalSqrtHintsRaw,
    sq128::Sq128Raw,
};
use deadeye_optimizer::{NormalOptimizationInput, optimize_normal_trade};
use deadeye_starknet::{
    Account, ExecutionReceipt, Felt, NormalMarketReader, NormalMarketWriter, NormalTradeQuote,
    Provider,
    types::normal::TradeInput,
};
use tracing::instrument;

use crate::{error::SdkResult, quote::PreparedQuote};

/// Cairo `SQRT_PI_RAW` from `the-situation/contracts/src/market/normal/constants.cairo`.
///
/// `sqrt(π) ≈ 1.7724538509055160272981674833...` floor-encoded into Q128.128.
/// This is the **exact same limb representation** the on-chain math runtime
/// uses when computing `compute_hints_view`, so any hints we derive from
/// it via `Sq128::checked_mul` + `Sq128::sqrt` are bit-identical to the
/// chain's output.
const SQRT_PI_RAW: Sq128Raw = Sq128Raw {
    limb0: 0xC3B0_520D_5DB9_383F,
    limb1: 0xC5BF_891B_4EF6_AA79,
    limb2: 0x1,
    limb3: 0x0,
    neg: false,
};

/// Returns the chain-bit-exact `√π` constant used inside `compute_hints_view`.
#[inline]
fn sqrt_pi() -> Sq128 {
    Sq128::from_raw(SQRT_PI_RAW)
}

/// Off-chain mirror of the Cairo `compute_hints_view(dist)` on a normal
/// math runtime.
///
/// Formulas (from `the-situation/contracts/src/market/normal/`):
/// * `l2_norm_denom = sqrt(mul_down(mul_down(2, σ), √π))`
/// * `backing_denom = sqrt(mul_down(σ, √π))`
///
/// `Sq128::checked_mul` matches `mul_down` (Q128.128 floor-truncating
/// product) and `Sq128::sqrt` matches `sqrt_verified`/`sqrt_unchecked`
/// for non-negative inputs (proven bit-exact in `docs/SQ128_SQRT.md`).
///
/// Returns `Err` only on `σ == 0` (degenerate distribution) or arithmetic
/// overflow — both of which the chain runtime also rejects with `Option::None`.
fn compute_normal_hints_offline(sigma: Sq128) -> SdkResult<NormalSqrtHintsRaw> {
    if sigma.is_zero() {
        return Err(deadeye_core::CoreError::invalid_input(
            "sigma",
            "must be > 0 for hint derivation",
        )
        .into());
    }
    let two = Sq128::from_i128(2);
    let two_sigma = two.checked_mul(sigma)?;
    let denom_sq_l2 = two_sigma.checked_mul(sqrt_pi())?;
    let l2_norm_denom = denom_sq_l2.sqrt()?;
    let sigma_sqrt_pi = sigma.checked_mul(sqrt_pi())?;
    let backing_denom = sigma_sqrt_pi.sqrt()?;
    Ok(NormalSqrtHintsRaw {
        l2_norm_denom: l2_norm_denom.to_raw(),
        backing_denom: backing_denom.to_raw(),
    })
}

/// Live-state-derived `effective_k` mirroring the on-chain
/// `compute_effective_trade_k_view`:
/// `effective_k = max(base_k, mul_down(base_k, pool_backing) / initial_backing)`.
///
/// `base_k`         := `params.k` (the AMM-stored invariant constant).
/// `pool_backing`   := `params.backing` (live, post-LP-flows).
/// `initial_backing`:= `lp_info.total_backing_deposited` (the cumulative
///                    backing deposited gross of withdrawals — i.e. the
///                    immutable reference against which `k` is scaled).
///
/// Returns `base_k` as a fallback when `initial_backing == 0` (matches the
/// chain's `Option::None` path: no scaling possible, so no upgrade applied).
fn live_effective_k(params_k: Sq128, pool_backing: Sq128, initial_backing: Sq128) -> Sq128 {
    if initial_backing.is_zero() || initial_backing.is_negative() {
        return params_k;
    }
    let scaled = params_k
        .checked_mul(pool_backing)
        .and_then(|s| s.checked_div(initial_backing));
    match scaled {
        Ok(s) if s > params_k => s,
        _ => params_k,
    }
}

/// Handle to a deployed normal AMM market.
///
/// A handle bundles a reader (and optionally a writer) bound to a single
/// market address. It is the canonical entry point for off-chain quote
/// preparation + on-chain execution.
#[derive(Debug)]
pub struct NormalMarket<'p, P>
where
    P: Provider,
{
    reader: NormalMarketReader<&'p P>,
}

impl<'p, P> NormalMarket<'p, P>
where
    P: Provider,
{
    /// Construct a handle bound to `provider` and the on-chain `address`.
    pub fn new(provider: &'p P, address: Felt) -> Self {
        Self {
            reader: NormalMarketReader::new(provider, address),
        }
    }

    /// Underlying read-only reader.
    pub const fn reader(&self) -> &NormalMarketReader<&'p P> {
        &self.reader
    }

    /// Returns the contract address.
    pub const fn address(&self) -> Felt {
        self.reader.address()
    }

    /// Fetch the current market distribution.
    pub async fn distribution(&self) -> SdkResult<NormalDistribution> {
        Ok(self.reader.distribution().await?)
    }

    /// Prepares a quote that moves the market from its current distribution
    /// to `candidate`. Returns the [`PreparedQuote`] containing the off-chain
    /// `x*` and required collateral.
    #[instrument(skip(self), fields(market = %self.reader.address()))]
    pub async fn prepare_quote(
        &self,
        candidate: NormalDistribution,
        policy: MinimizationPolicy,
    ) -> SdkResult<PreparedQuote> {
        let current = self.distribution().await?;
        let verified = normal_collateral(&current, &candidate, policy)?;
        Ok(PreparedQuote {
            x_star: Sq128::from_f64(verified.x_min)?,
            collateral: Sq128::from_f64(verified.collateral)?,
            iterations: verified.iterations,
        })
    }

    /// Pick the EV-maximizing candidate distribution given a belief +
    /// budget, then quote it against `runtime`.
    ///
    /// Use this when you have a directional view (the trader's belief
    /// about the true outcome) and a budget, but don't want to hand-pick
    /// the candidate `(μ_g, σ_g)`. The optimizer searches a fixed grid
    /// inside the policy region for the highest net-EV trade, then
    /// hands the winner to [`NormalMarketReader::quote_trade`] for a
    /// full chain preflight.
    ///
    /// # Inputs
    ///
    /// * `runtime` — math runtime contract address (used by `quote_trade`).
    /// * `belief_mean` / `belief_sigma` — the trader's belief about the
    ///   true outcome distribution.
    /// * `budget` — maximum collateral the trader is willing to risk.
    ///
    /// # Returns
    ///
    /// A [`NormalTradeQuote`]. Inspect `on_chain_will_accept`; if `false`
    /// the budget was too tight for any in-policy candidate (and the
    /// quote will carry the optimizer's "no trade" candidate equal to
    /// the current market distribution). If `true`, hand the quote to
    /// [`NormalMarketWriter::execute_quote`] from a signed handle.
    ///
    /// # Worked example
    ///
    /// ```no_run
    /// # use deadeye_sdk::normal::NormalMarket;
    /// # use deadeye_sdk::starknet::{Felt, JsonRpcProvider};
    /// # use deadeye_sdk::core::Sq128;
    /// # use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let rpc = JsonRpcClient::new(HttpTransport::new(
    ///     "http://localhost:5050".parse::<url::Url>()?,
    /// ));
    /// let provider = JsonRpcProvider::new(rpc);
    /// let market = NormalMarket::new(&provider, Felt::ZERO);
    /// // I believe outcome ≈ 50 (±2σ confidence), can spend up to 10 STRK.
    /// let quote = market.optimize_quote(Felt::ZERO, 50.0, 2.0, 10.0).await?;
    /// if quote.on_chain_will_accept {
    ///     // hand to a signed handle to execute
    /// }
    /// # Ok(()) }
    /// ```
    #[instrument(skip(self), fields(market = %self.reader.address(), runtime = %runtime))]
    pub async fn optimize_quote(
        &self,
        runtime: Felt,
        belief_mean: f64,
        belief_sigma: f64,
        budget: f64,
    ) -> SdkResult<NormalTradeQuote> {
        let current = self.distribution().await?;
        let params = self.reader.params().await?;
        let market_mean = current.mean().to_f64();
        let market_sigma = current.sigma().to_f64();
        // TODO(v1.1): scale `k` by `pool_backing / initial_backing` once
        // `lp_info` exposes the immutable initial backing distinctly from
        // `params.backing`. Today `params.k` reflects the AMM's stored k;
        // the on-chain verifier re-checks any candidate against the live
        // effective k, so a slightly-stale k here only widens the
        // optimizer's search region — never approves a bad trade.
        let effective_k = Sq128::from_raw(params.k).to_f64();
        let opt = optimize_normal_trade(NormalOptimizationInput::new(
            budget,
            belief_mean,
            belief_sigma,
            market_mean,
            market_sigma,
            effective_k,
        ));
        // Construct the candidate via `from_variance` so σ is derived
        // via [`Sq128::sqrt`] (chain-bit-exact with `sqrt_verified`),
        // not via f64. This ensures the on-chain runtime's
        // `compute_hints_view` accepts the candidate by construction —
        // mirrors the path `optimize_quote_offline` uses below.
        let cand_mean = Sq128::from_f64(opt.optimized_mean)?;
        let cand_variance = Sq128::from_f64(opt.optimized_variance)?;
        let candidate =
            deadeye_core::NormalDistribution::from_variance(cand_mean, cand_variance)?;
        let supplied = Sq128::from_f64(opt.collateral_required.max(0.0))?;
        let pad = supplied;
        // `x_star` = the optimizer's μ_g is a reasonable seed; the chain
        // re-derives the true stationary point from the candidate, so a
        // seed inside the candidate's support is sufficient.
        let x_star = cand_mean;
        Ok(self
            .reader
            .quote_trade(
                runtime,
                candidate.to_raw(),
                x_star.to_raw(),
                supplied.to_raw(),
                pad.to_raw(),
            )
            .await?)
    }

    /// Off-chain-only EV optimizer — chain-bit-exact σ + hints, no runtime
    /// round-trip.
    ///
    /// Use this when **no math-runtime instance is deployed** (mainnet
    /// today, where the normal AMM ships as a library-dispatch class
    /// hash with no separately deployed instance). The output is
    /// chain-bit-exact for σ (derived via [`Sq128::sqrt`]) and the
    /// hints (derived via the same Sq128 formulas the on-chain
    /// `compute_hints_view` runs), so the resulting candidate
    /// distribution and `candidate_hints` survive the on-chain
    /// `check_trade_view` `sqrt_verified` / `mul_down` invariants by
    /// construction.
    ///
    /// # Pipeline
    ///
    /// 1. Reads live market state from chain — `distribution`, `params`,
    ///    `lp_info`.
    /// 2. Derives `effective_k` from `(params.k, params.backing,
    ///    lp_info.total_backing_deposited)` using the exact chain
    ///    formula (`max(base_k, mul_down(base_k, pool_backing) /
    ///    initial_backing)`).
    /// 3. Hands the live `(μ_market, σ_market, effective_k, budget)` to
    ///    [`deadeye_optimizer::optimize_normal_trade`] to pick the
    ///    EV-optimal `(μ_g, σ_g²)`.
    /// 4. **Replaces the optimizer's f64-derived σ** with the
    ///    chain-bit-exact σ obtained via
    ///    [`NormalDistribution::from_variance`] (internally
    ///    `Sq128::sqrt`). This is the critical fix that prevents
    ///    `INVALID_DISTRIBUTION` rejections on submit — f64 sqrt rounds
    ///    in the last bit for any non-perfect Sq128 square, but
    ///    `Sq128::sqrt` is bit-exact with Cairo `u512_sqrt`.
    /// 5. Computes `(l2_norm_denom, backing_denom)` offline via the
    ///    same `sqrt(mul_down(...))` chain the on-chain runtime uses.
    ///
    /// # Returns
    ///
    /// A [`NormalTradeQuote`] with:
    /// * `candidate` — chain-bit-exact `(μ, variance, σ)` triple
    /// * `candidate_hints` — chain-bit-exact `(l2_norm_denom, backing_denom)`
    /// * `x_star` — the optimizer's `μ_g` (an in-support seed; the chain
    ///   re-derives the true stationary point at execute time)
    /// * `required_collateral` / `padded_collateral` — both set to the
    ///   off-chain collateral solver's output
    /// * `on_chain_will_accept` — `true` iff the optimizer found a
    ///   positive-EV trade (i.e. cost > 0 and EV > cost). Mind the
    ///   caveat below.
    /// * `rejection` — `Some("off-chain optimizer found no positive-EV
    ///   trade")` when `!on_chain_will_accept`, else `None`.
    ///
    /// # Bit-exactness vs. chain acceptance
    ///
    /// **Off-chain accept ≠ chain guarantee.** The σ-derivation, hints
    /// and candidate distribution are bit-exact with what the chain
    /// would compute (proven against the deployed `normal_math_runtime`
    /// at 20/20 in [`docs/SQ128_SQRT.md`](../../../docs/SQ128_SQRT.md)),
    /// so `INVALID_DISTRIBUTION` and `INVALID_HINTS` are eliminated.
    /// What's still re-checked at execute time:
    /// * trader collateral balance + allowance
    /// * fresh-state nonce (no front-run between quote and execute)
    /// * the AMM's `check_trade_view` admissibility envelope (`min
    ///   trade collateral`, `tolerance`, policy region — these are
    ///   conservative side-constraints, not σ/hint correctness)
    ///
    /// In other words: this method removes the σ-precision footgun
    /// that the f64-fallback used to introduce. The chain still
    /// re-verifies the trade on submit, as it does for every path.
    #[instrument(skip(self), fields(market = %self.reader.address()))]
    pub async fn optimize_quote_offline(
        &self,
        belief_mean: f64,
        belief_sigma: f64,
        budget_xp: f64,
    ) -> SdkResult<NormalTradeQuote> {
        let current = self.distribution().await?;
        let params = self.reader.params().await?;
        let lp_info = self.reader.lp_info().await?;

        let base_k = Sq128::from_raw(params.k);
        let pool_backing = Sq128::from_raw(params.backing);
        let initial_backing = Sq128::from_raw(lp_info.total_backing_deposited);
        let effective_k_sq = live_effective_k(base_k, pool_backing, initial_backing);
        let effective_k = effective_k_sq.to_f64();

        let market_mean = current.mean().to_f64();
        let market_sigma = current.sigma().to_f64();

        let opt = optimize_normal_trade(NormalOptimizationInput::new(
            budget_xp,
            belief_mean,
            belief_sigma,
            market_mean,
            market_sigma,
            effective_k,
        ));

        // Build the candidate distribution with the chain-bit-exact σ.
        // The optimizer reports `optimized_variance = optimized_sigma²`
        // in f64; we promote `variance` to Sq128 (round-trip via
        // `from_f64`) and then *re-derive* σ via `Sq128::sqrt`.
        // The optimizer's `optimized_sigma` field is discarded — it
        // was an f64 sqrt of the variance and would mis-satisfy
        // `sqrt_verified` for most variances.
        let cand_mean = Sq128::from_f64(opt.optimized_mean)?;
        let cand_variance = Sq128::from_f64(opt.optimized_variance)?;
        let candidate = NormalDistribution::from_variance(cand_mean, cand_variance)?;
        let candidate_sigma = candidate.sigma();
        let candidate_hints = compute_normal_hints_offline(candidate_sigma)?;

        // `x_star` seed = μ_g; chain re-derives the true stationary point.
        let x_star = cand_mean;

        let collateral_required = Sq128::from_f64(opt.collateral_required.max(0.0))?;
        let padded_collateral = collateral_required;

        // Decide acceptance: positive cost AND positive net EV. The
        // optimizer's "no-trade" sentinel (`collateral_required == 0`)
        // surfaces as `on_chain_will_accept = false` with an
        // informative rejection message.
        // `rejection` is the typed on-chain rejection enum; off-chain
        // we have no chain verdict to surface. Callers that want a
        // human-readable "no positive-EV trade" message read off the
        // pair `(on_chain_will_accept == false, required_collateral == 0)`.
        let has_positive_trade =
            opt.collateral_required > 0.0 && opt.expected_value > opt.collateral_required;
        let rejection = None;

        Ok(NormalTradeQuote {
            candidate: candidate.to_raw(),
            candidate_hints,
            x_star: x_star.to_raw(),
            required_collateral: collateral_required.to_raw(),
            padded_collateral: padded_collateral.to_raw(),
            on_chain_will_accept: has_positive_trade,
            rejection,
        })
    }

    /// Bind an [`Account`] to this market, returning a write-capable handle.
    ///
    /// The returned [`NormalMarketSigned`] holds the same reader plus the
    /// signer, so reads remain cheap and writes funnel through a single
    /// place.
    pub fn with_account<A>(self, account: A) -> NormalMarketSigned<'p, P, A>
    where
        A: Account,
    {
        NormalMarketSigned {
            writer: NormalMarketWriter::new(self.reader, account),
        }
    }
}

/// Account-bound companion to [`NormalMarket`].
#[derive(Debug)]
pub struct NormalMarketSigned<'p, P, A>
where
    P: Provider,
    A: Account,
{
    writer: NormalMarketWriter<&'p P, A>,
}

impl<P, A> NormalMarketSigned<'_, P, A>
where
    P: Provider,
    A: Account,
{
    /// Borrow the underlying writer.
    pub const fn writer(&self) -> &NormalMarketWriter<&P, A> {
        &self.writer
    }

    /// Returns the market address.
    pub const fn address(&self) -> Felt {
        self.writer.reader().address()
    }

    /// Fetch the current market distribution (read passthrough).
    pub async fn distribution(&self) -> SdkResult<NormalDistribution> {
        Ok(self.writer.reader().distribution().await?)
    }

    /// Execute a previously-prepared quote.
    #[instrument(skip(self, candidate), fields(market = %self.writer.reader().address()))]
    pub async fn execute(
        &self,
        candidate: NormalDistribution,
        quote: PreparedQuote,
    ) -> SdkResult<ExecutionReceipt> {
        // Build the on-chain trade input from the off-chain quote +
        // candidate. Hints default to zero — the on-chain math runtime
        // will fall back to its internal computation if the hints don't
        // validate. Callers that want deterministic gas should supply
        // their own hints via `writer().build_trade_call(...)`.
        let input = TradeInput {
            candidate: candidate.to_raw(),
            x_star: quote.x_star.to_raw(),
            supplied_collateral: quote.collateral.to_raw(),
            candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw {
                l2_norm_denom: Sq128::ZERO.to_raw(),
                backing_denom: Sq128::ZERO.to_raw(),
            },
        };
        Ok(self.writer.execute_trade(input).await?)
    }

    /// Pass-through to the underlying writer for advanced calldata building.
    pub const fn writer_ref(&self) -> &NormalMarketWriter<&P, A> {
        &self.writer
    }
}

// We re-export `Distribution` so callers that need to read `.mean()` /
// `.variance()` on the returned distribution can do so without an extra
// import.
pub use deadeye_core::Distribution;

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {
        let _ = core::any::type_name::<T>();
    }

    #[test]
    fn normal_market_is_send_sync() {
        assert_send_sync::<NormalMarket<'_, MockProvider>>();
    }

    use async_trait::async_trait;
    use deadeye_starknet::ContractResult;
    use starknet_core::types::{BlockId, FunctionCall};

    #[derive(Debug)]
    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            unreachable!("mock provider only used for trait wiring")
        }
    }

    // ─── Offline hint derivation parity ─────────────────────────────────
    //
    // These don't hit chain; they cross-check the offline formula against
    // its own mathematical specification — `l2_norm_denom == sqrt(2σ√π)`
    // and `backing_denom == sqrt(σ√π)` — at Sq128 precision. Devnet
    // bit-parity vs. `compute_hints_view` is covered by the integration
    // test `offline_optimize_quote_parity`.

    #[test]
    fn sqrt_pi_constant_round_trips() {
        // Reading the limbs into `Sq128` and back to raw must be the identity.
        let value = sqrt_pi();
        assert_eq!(value.to_raw(), SQRT_PI_RAW);
        // Value should be ≈ 1.7724538509...
        let approx = value.to_f64();
        let true_sqrt_pi = core::f64::consts::PI.sqrt();
        assert!(
            (approx - true_sqrt_pi).abs() < 1e-15,
            "sqrt_pi constant: got {approx}, expected ≈ {true_sqrt_pi}",
        );
    }

    #[test]
    fn offline_hints_satisfy_definition() {
        // For σ ∈ {0.1, 1.0, 2.5, 10.0}:
        //   l2_norm_denom² ≈ 2σ√π   (within Sq128 floor tolerance)
        //   backing_denom² ≈ σ√π    (within Sq128 floor tolerance)
        for &s in &[0.1_f64, 1.0_f64, 2.5_f64, 10.0_f64] {
            let sigma = Sq128::from_f64(s).expect("σ converts");
            let hints = compute_normal_hints_offline(sigma).expect("hints derive");

            let l2 = Sq128::from_raw(hints.l2_norm_denom);
            let backing = Sq128::from_raw(hints.backing_denom);

            // `l2² ≤ 2σ√π` (floor sqrt invariant)
            let two_sigma = Sq128::from_i128(2).checked_mul(sigma).unwrap();
            let two_sigma_sqrt_pi = two_sigma.checked_mul(sqrt_pi()).unwrap();
            let l2_sq = l2.checked_mul(l2).unwrap();
            assert!(
                l2_sq <= two_sigma_sqrt_pi,
                "σ={s}: l2² > 2σ√π violates sqrt floor invariant",
            );

            // `backing² ≤ σ√π`
            let sigma_sqrt_pi = sigma.checked_mul(sqrt_pi()).unwrap();
            let backing_sq = backing.checked_mul(backing).unwrap();
            assert!(
                backing_sq <= sigma_sqrt_pi,
                "σ={s}: backing² > σ√π violates sqrt floor invariant",
            );
        }
    }

    #[test]
    fn offline_hints_reject_zero_sigma() {
        let err = compute_normal_hints_offline(Sq128::ZERO).expect_err("must reject");
        let msg = format!("{err}");
        assert!(
            msg.contains("sigma"),
            "expected sigma-related error, got: {msg}",
        );
    }

    #[test]
    fn live_effective_k_falls_back_when_initial_backing_zero() {
        let base = Sq128::from_f64(75.07).unwrap();
        let pool = Sq128::from_f64(1000.0).unwrap();
        let initial = Sq128::ZERO;
        // initial_backing == 0 → fall back to base_k (no scaling).
        let k = live_effective_k(base, pool, initial);
        assert_eq!(k, base);
    }

    #[test]
    fn live_effective_k_uses_base_when_pool_below_initial() {
        // pool < initial → max(base, base*pool/initial) == base (smaller fraction).
        let base = Sq128::from_f64(75.07).unwrap();
        let pool = Sq128::from_f64(500.0).unwrap();
        let initial = Sq128::from_f64(1000.0).unwrap();
        let k = live_effective_k(base, pool, initial);
        assert_eq!(k, base);
    }

    #[test]
    fn live_effective_k_scales_up_when_pool_above_initial() {
        // pool > initial → max(base, base*pool/initial) > base.
        let base = Sq128::from_f64(75.07).unwrap();
        let pool = Sq128::from_f64(2000.0).unwrap();
        let initial = Sq128::from_f64(1000.0).unwrap();
        let k = live_effective_k(base, pool, initial);
        assert!(k > base, "expected scaled-up k, got {k} vs base {base}");
        // ≈ 2 × base, within Sq128 precision.
        let expected = Sq128::from_f64(150.14).unwrap();
        let diff = if k > expected {
            k.checked_sub(expected).unwrap()
        } else {
            expected.checked_sub(k).unwrap()
        };
        let one_ulp = Sq128::from_f64(1e-9).unwrap();
        assert!(
            diff < one_ulp,
            "expected k ≈ 150.14, got {} (diff {})",
            k.to_f64(),
            diff.to_f64()
        );
    }
}
