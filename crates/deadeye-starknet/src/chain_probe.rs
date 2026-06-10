//! Chain-probe `x*` refinement — the issue #13 root-cause fix.
//!
//! ## Why this module exists
//!
//! The AMM's trade verification (`check_trade_view`, library-called by
//! `execute_trade`) checks that the trader-supplied `x*` is a stationary
//! point of the **λ-scaled** PDF difference: `|λ_g·g'(x*) − λ_f·f'(x*)| ≤
//! tolerance`. Crucially, the contract evaluates that derivative with its own
//! `SQ128x128` fixed-point `pdf`, whose relative error (measured live: up to
//! ~1.5e-4) dwarfs the `tolerance` (1e-3 absolute on a derivative whose
//! terms are ~10³). The acceptance window around the *chain's* root is only
//! `≈ 2·tolerance / d''(x*)` wide — often ~1e-7 in `x` — and sits noticeably
//! off the mathematically-true root (measured live: 1.3e-5 away on a CPI
//! σ-tighten trade, with chain `d'` = −0.16 at the f64 root). So an off-chain
//! f64 solver lands on a mathematically-perfect `x*` that the chain rejects
//! with `VERIFICATION_FAILED` every time.
//!
//! ## What it does
//!
//! Rather than bit-exactly porting the Cairo fixed-point math (fragile across
//! contract upgrades), this module runs **Newton's method against the chain's
//! own arithmetic**, all inside gas-free `starknet_simulateTransactions`
//! calls (skip-validate + skip-fee-charge — no fee, signature ignored):
//!
//! 1. Read the market's `get_runtime_class_hash()` — the exact math-runtime
//!    class the AMM library-calls — and deploy it via the UDC *inside the
//!    simulation* (nothing lands on chain).
//! 2. Measure the chain's `pdf_f(x)` / `pdf_g(x)` through the runtime's
//!    `compute_trade_lot_value_view` (a lot with `from_λ=0, to_λ=1` returns
//!    exactly `pdf_to(x)` in chain arithmetic).
//! 3. Reconstruct the chain's `d'(x) = λ_g·g'(x) − λ_f·f'(x)` from those pdf
//!    values (the outer multipliers are exact), Newton-step `x ← x − d'/d''`,
//!    and ask `check_trade_view` for the verdict at each iterate.
//! 4. Return the first `x*` the chain itself certifies (stationary ✓ side ✓
//!    curvature ✓ argmin ✓) plus the chain's own `computed_collateral`, so the
//!    caller supplies exactly what `execute_trade` will demand.
//!
//! Because the probed class hash is read from the market, the verdicts match
//! what `execute_trade` enforces — across any future runtime upgrade. No gas
//! is ever spent, and no live runtime deployment is required, so the CLI
//! works against fresh markets out of the box.

use deadeye_core::{
    Distribution as _, Sq128, distribution::NormalDistributionRaw, sq128::Sq128Raw,
};
use starknet_accounts::Account as _;
use starknet_core::{
    types::{ExecuteInvocation, Felt, FunctionInvocation, TransactionTrace},
    utils::get_selector_from_name,
};
use tracing::instrument;

use crate::{
    account::OwnedAccount,
    cairo_serde::CairoSerde,
    error::{ContractError, ContractResult},
    execution::Call,
    normal_amm::{NormalMarketReader, NormalTradeQuote},
    provider::Provider,
    types::normal::TradeCheckRaw,
};

/// Universal Deployer Contract address (mainnet + testnets share it).
pub const UDC_ADDRESS: Felt =
    Felt::from_hex_unchecked("0x041a78e741e5af2fec34b695679bc6891742439f7afb8484ecd7766661ad02bf");

/// Base salt for the in-simulation runtime deployment. The deployment never
/// lands on chain (it lives only inside the simulated transaction), so the
/// only constraint is that no contract already exists at the derived address;
/// on a collision the probe retries once with `salt + 1`.
const PROBE_SALT_BASE: Felt = Felt::from_hex_unchecked("0xdeade7e9");

/// Maximum Newton rounds (each is one gas-free simulation). Convergence is
/// quadratic when the chain's pdf error is smooth in `x`; 2–3 rounds are
/// typical, the rest is headroom.
const MAX_ROUNDS: usize = 5;

/// A chain-certified `x*` produced by [`refine_normal_quote`].
#[derive(Debug, Clone, Copy)]
pub struct ProbeOutcome {
    /// The accepted stationary point, ready for `execute_trade`.
    pub x_star: Sq128Raw,
    /// Offset (in `x` units) from the off-chain root that the chain accepted.
    pub offset: f64,
    /// The chain's own collateral requirement at `x_star` (computed with the
    /// market's **effective k** — base k scaled by live LP backing — exactly
    /// as `execute_trade` will). This is the *net* amount the verifier
    /// demands; see [`Self::net_rate`] for the gross→net conversion.
    pub computed_collateral: Sq128Raw,
    /// Fraction of the gross supplied collateral that survives the deposit
    /// fee deduction (`net_collateral / supplied`, measured through the
    /// runtime's own `compute_deposit_fees_view`). `execute_trade` verifies
    /// `net ≥ computed_collateral`, so the caller must supply
    /// `computed_collateral / net_rate` (plus margin) **gross**.
    pub net_rate: f64,
    /// Full verdict at the accepted point (all verification flags true).
    pub check: TradeCheckRaw,
    /// Number of simulation rounds spent.
    pub rounds: usize,
}

/// `true` iff every chain-side verification flag a trader cannot fix by
/// supplying more collateral is satisfied. (`collateral_sufficient` is
/// excluded — the caller sizes the supply from `computed_collateral`.)
fn flags_accept(
    backing: &crate::types::common::ScaledBackingCheckRaw,
    v: &crate::types::common::CollateralVerificationRaw,
) -> bool {
    backing.is_valid
        && backing.computation_succeeded
        && v.side_valid
        && v.stationary_valid
        && v.curvature_valid
        && v.computation_valid
        && v.argmin_value_valid
}

/// [`flags_accept`] over a normal-market verdict.
fn chain_accepts(check: &TradeCheckRaw) -> bool {
    flags_accept(&check.backing_check, &check.verification)
}

/// [`flags_accept`] over a lognormal-market verdict.
fn lognormal_chain_accepts(check: &crate::types::lognormal::LognormalTradeCheckRaw) -> bool {
    flags_accept(&check.backing_check, &check.verification)
}

/// λ(σ, k) = k·√(2σ√π) — the scoring-rule scale factor; matches the chain's
/// `compute_lambda` to ~1e-8 relative (verified live), which contributes
/// ≤1e-5 absolute to the reconstructed derivative — far inside the 1e-3
/// stationary tolerance.
fn lambda(sigma: f64, k: f64) -> f64 {
    k * (2.0 * sigma * core::f64::consts::PI.sqrt()).sqrt()
}

/// f64 evaluation of the λ-scaled second derivative
/// `d''(x) = λ_g·g''(x) − λ_f·f''(x)` (the Newton-step denominator; a
/// relative error here only perturbs the step length, not the target).
fn d_double_prime_scaled(
    current: &deadeye_core::NormalDistribution,
    candidate: &deadeye_core::NormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    x: f64,
) -> Option<f64> {
    let x_q = Sq128::from_f64(x).ok()?;
    let f_pp = current.pdf_second_derivative(x_q).ok()?.to_f64();
    let g_pp = candidate.pdf_second_derivative(x_q).ok()?.to_f64();
    Some(lambda_g.mul_add(g_pp, -(lambda_f * f_pp)))
}

/// Chain-side context shared by every probe round.
///
/// `k` and `backing` mirror what `execute_trade` itself passes to the
/// verifier: the **effective** k (base k scaled by live LP backing via
/// `compute_effective_trade_k_view`) and the **live pool backing** — not the
/// immutable `params` values.
struct ProbeContext {
    runtime_class: Felt,
    current_raw: NormalDistributionRaw,
    candidate_raw: NormalDistributionRaw,
    supplied: Sq128Raw,
    k: Sq128Raw,
    backing: Sq128Raw,
    tolerance: Sq128Raw,
    min_trade_collateral: Sq128Raw,
    current_hints: deadeye_core::distribution::NormalSqrtHintsRaw,
    candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw,
    /// `(internal_decimals, decimal_shift, fee_config)` for the deposit-fee
    /// measurement call.
    fee_inputs: (u8, u8, crate::types::common::FeeConfigRaw),
    check_selector: Felt,
    lot_selector: Felt,
    fees_selector: Felt,
    deploy_selector: Felt,
}

impl ProbeContext {
    /// `compute_trade_lot_value_view` with a `{from_λ=0, to_λ=1}` lot —
    /// returns the chain's `pdf_dist(x)` verbatim.
    fn pdf_call(&self, to: Felt, dist: &NormalDistributionRaw, x: Sq128Raw) -> Call {
        let one = Sq128::from_i128(1).to_raw();
        let zero = Sq128::ZERO.to_raw();
        let mut cd = Vec::with_capacity(53);
        cd.push(Felt::ZERO); // lot_id: u64
        cd.push(Felt::ZERO); // trader: ContractAddress
        zero.encode(&mut cd); // collateral_locked
        dist.mean.encode(&mut cd); // from_* (λ=0 ⇒ value-irrelevant)
        dist.variance.encode(&mut cd);
        dist.sigma.encode(&mut cd);
        zero.encode(&mut cd); // from_lambda = 0
        dist.mean.encode(&mut cd); // to_*
        dist.variance.encode(&mut cd);
        dist.sigma.encode(&mut cd);
        one.encode(&mut cd); // to_lambda = 1
        cd.push(Felt::ONE); // flags: u8 = LOT_FLAG_EXISTS
        x.encode(&mut cd);
        Call {
            to,
            selector: self.lot_selector,
            calldata: cd,
        }
    }

    /// `check_trade_view` for the candidate trade at `x`.
    fn check_call(&self, to: Felt, x: Sq128Raw) -> Call {
        let mut cd = Vec::with_capacity(80);
        self.current_raw.encode(&mut cd);
        self.candidate_raw.encode(&mut cd);
        x.encode(&mut cd);
        self.supplied.encode(&mut cd);
        self.k.encode(&mut cd);
        self.backing.encode(&mut cd);
        self.tolerance.encode(&mut cd);
        self.min_trade_collateral.encode(&mut cd);
        self.current_hints.encode(&mut cd);
        self.candidate_hints.encode(&mut cd);
        Call {
            to,
            selector: self.check_selector,
            calldata: cd,
        }
    }

    /// The UDC deploy of the market's runtime class.
    fn deploy_call(&self, salt: Felt) -> Call {
        Call {
            to: UDC_ADDRESS,
            selector: self.deploy_selector,
            calldata: vec![self.runtime_class, salt, Felt::ZERO, Felt::ZERO],
        }
    }

    /// `compute_deposit_fees_view(supplied, internal_decimals, decimal_shift,
    /// fee_config)` — measures the gross→net collateral rate the AMM applies
    /// before its sufficiency check.
    fn fees_call(&self, to: Felt) -> Call {
        let (internal_decimals, decimal_shift, fee_config) = self.fee_inputs;
        let mut cd = Vec::with_capacity(10);
        self.supplied.encode(&mut cd);
        cd.push(Felt::from(internal_decimals));
        cd.push(Felt::from(decimal_shift));
        fee_config.encode(&mut cd);
        Call {
            to,
            selector: self.fees_selector,
            calldata: cd,
        }
    }
}

/// Decoded results of one probe round.
struct RoundResult {
    chain_pdf_f: f64,
    chain_pdf_g: f64,
    /// `net_collateral / supplied` from the deposit-fee measurement (only
    /// requested on the first round).
    net_rate: Option<f64>,
    checks: Vec<TradeCheckRaw>,
}

/// Run one `[deploy, (fees,) pdf_f(x), pdf_g(x), check×xs]` simulation and
/// decode it. Retries once with a bumped salt if the UDC address is already
/// occupied.
async fn run_round(
    account: &OwnedAccount,
    ctx: &ProbeContext,
    x: Sq128Raw,
    check_xs: &[Sq128Raw],
    measure_fees: bool,
) -> ContractResult<RoundResult> {
    let mut last_revert: Option<String> = None;
    for attempt in 0_u64..2 {
        let salt = PROBE_SALT_BASE + Felt::from(attempt);
        let deployed = starknet_core::utils::get_udc_deployed_address(
            salt,
            ctx.runtime_class,
            &starknet_core::utils::UdcUniqueness::NotUnique,
            &[],
        );
        let mut calls = Vec::with_capacity(4 + check_xs.len());
        calls.push(ctx.deploy_call(salt));
        if measure_fees {
            calls.push(ctx.fees_call(deployed));
        }
        calls.push(ctx.pdf_call(deployed, &ctx.current_raw, x));
        calls.push(ctx.pdf_call(deployed, &ctx.candidate_raw, x));
        for cx in check_xs {
            calls.push(ctx.check_call(deployed, *cx));
        }
        let sim = account
            .inner()
            .execute_v3(calls)
            .simulate(true, true)
            .await
            .map_err(|e| ContractError::Provider(format!("probe simulate: {e}")))?;
        let TransactionTrace::Invoke(trace) = sim.transaction_trace else {
            return Err(ContractError::InvalidResponse {
                call: "chain_probe",
                message: "non-INVOKE simulation trace".into(),
            });
        };
        match trace.execute_invocation {
            ExecuteInvocation::Success(top) => {
                return decode_round(&top, ctx, check_xs.len(), measure_fees);
            },
            ExecuteInvocation::Reverted(r) => last_revert = Some(r.revert_reason),
        }
    }
    Err(ContractError::Provider(format!(
        "probe simulation reverted: {}",
        last_revert.unwrap_or_else(|| "unknown".into())
    )))
}

/// Decode `Option<Sq128Raw>` from a lot-value invocation result.
fn decode_lot_value(inv: &FunctionInvocation) -> ContractResult<f64> {
    if inv.result.first() != Some(&Felt::ZERO) {
        return Err(ContractError::InvalidResponse {
            call: "compute_trade_lot_value_view",
            message: "runtime returned None for the pdf probe".into(),
        });
    }
    let (sq, _) = Sq128Raw::decode(&inv.result[1..]).map_err(ContractError::from)?;
    Ok(Sq128::from_raw(sq).to_f64())
}

/// Decode `Option<DepositFeeComputationRaw>` and return
/// `net_collateral / supplied`.
fn decode_net_rate(inv: &FunctionInvocation, supplied: Sq128Raw) -> ContractResult<f64> {
    if inv.result.first() != Some(&Felt::ZERO) {
        return Err(ContractError::InvalidResponse {
            call: "compute_deposit_fees_view",
            message: "runtime returned None for the fee probe".into(),
        });
    }
    // DepositFeeComputationRaw = { token_amount: u256, lp_fee: u256,
    // protocol_fee: u256, net_collateral: Sq128Raw } — skip 3×2 felts of
    // u256s, then decode the net collateral.
    let tail = inv
        .result
        .get(7..)
        .ok_or_else(|| ContractError::InvalidResponse {
            call: "compute_deposit_fees_view",
            message: "fee probe result too short".into(),
        })?;
    let (net, _) = Sq128Raw::decode(tail).map_err(ContractError::from)?;
    let gross = Sq128::from_raw(supplied).to_f64();
    let net_f = Sq128::from_raw(net).to_f64();
    if !(gross.is_finite() && net_f.is_finite()) || gross <= 0.0 || net_f <= 0.0 || net_f > gross {
        return Err(ContractError::InvalidResponse {
            call: "compute_deposit_fees_view",
            message: format!("implausible net/gross collateral: {net_f} / {gross}"),
        });
    }
    Ok(net_f / gross)
}

fn decode_round(
    top: &FunctionInvocation,
    ctx: &ProbeContext,
    expected_checks: usize,
    measure_fees: bool,
) -> ContractResult<RoundResult> {
    let pdfs: Vec<&FunctionInvocation> = top
        .calls
        .iter()
        .filter(|c| c.entry_point_selector == ctx.lot_selector)
        .collect();
    let checks_inv: Vec<&FunctionInvocation> = top
        .calls
        .iter()
        .filter(|c| c.entry_point_selector == ctx.check_selector)
        .collect();
    if pdfs.len() != 2 || checks_inv.len() != expected_checks {
        return Err(ContractError::InvalidResponse {
            call: "chain_probe",
            message: format!(
                "trace shape mismatch: {} pdf probes (want 2), {} checks (want {expected_checks})",
                pdfs.len(),
                checks_inv.len(),
            ),
        });
    }
    let net_rate = if measure_fees {
        let fees_inv = top
            .calls
            .iter()
            .find(|c| c.entry_point_selector == ctx.fees_selector)
            .ok_or_else(|| ContractError::InvalidResponse {
                call: "chain_probe",
                message: "fee probe missing from trace".into(),
            })?;
        Some(decode_net_rate(fees_inv, ctx.supplied)?)
    } else {
        None
    };
    let chain_pdf_f = decode_lot_value(pdfs[0])?;
    let chain_pdf_g = decode_lot_value(pdfs[1])?;
    let mut checks = Vec::with_capacity(expected_checks);
    for inv in checks_inv {
        let (check, _) = TradeCheckRaw::decode(&inv.result).map_err(ContractError::from)?;
        checks.push(check);
    }
    Ok(RoundResult {
        chain_pdf_f,
        chain_pdf_g,
        net_rate,
        checks,
    })
}

/// Refine a normal-market quote's `x*` against the chain's own verifier.
///
/// Runs Newton's method on the **chain's** fixed-point derivative (measured
/// through gas-free simulations — no fee, signature ignored) and returns the
/// first point the chain certifies. Returns `Ok(None)` when the iteration
/// exhausts its rounds without acceptance (the caller should then keep the
/// original quote and rely on the pre-submit simulation gate). `Err` means
/// infrastructure failure (RPC, decode), not trade rejection.
#[instrument(skip_all, fields(market = %reader.address()))]
pub async fn refine_normal_quote<P>(
    account: &OwnedAccount,
    reader: &NormalMarketReader<P>,
    quote: &NormalTradeQuote,
) -> ContractResult<Option<ProbeOutcome>>
where
    P: Provider + Sync,
{
    // Market state the verifier sees: params + live distribution + the
    // chain's canonical current-distribution hints + the runtime class, plus
    // LP info / config / fees so the probe can mirror `execute_trade`'s
    // *effective* inputs (live backing, scaled k, net-of-fees collateral).
    let params = reader.params().await?;
    let current = reader.distribution().await?;
    let current_hints = reader.distribution_hints().await?;
    let runtime_class = reader.runtime_class_hash().await?;
    let lp_info = reader.lp_info().await?;
    let config = reader.config().await?;
    let fee_config = reader.fee_config().await?;

    let candidate =
        deadeye_core::NormalDistribution::from_raw(quote.candidate).map_err(ContractError::Core)?;
    let x0 = Sq128::from_raw(quote.x_star).to_f64();
    let tolerance = Sq128::from_raw(params.tolerance).to_f64();
    // Effective k, exactly as `execute_trade` derives it:
    // `max(base_k, base_k × pool_backing / initial_backing)`.
    let base_k = Sq128::from_raw(params.k).to_f64();
    let pool_backing = Sq128::from_raw(lp_info.total_backing_deposited).to_f64();
    let initial_backing = Sq128::from_raw(params.backing).to_f64();
    let k = if initial_backing > 0.0 && pool_backing.is_finite() {
        (base_k * pool_backing / initial_backing).max(base_k)
    } else {
        base_k
    };
    let k_raw = Sq128::from_f64(k).map_err(ContractError::Core)?.to_raw();
    let lambda_f = lambda(current.sigma().to_f64(), k);
    let lambda_g = lambda(candidate.sigma().to_f64(), k);
    let mu_f = current.mean().to_f64();
    let mu_g = candidate.mean().to_f64();
    let var_f = current.variance().to_f64();
    let var_g = candidate.variance().to_f64();
    // Cap on how far Newton may wander from the off-chain root: the chain's
    // pdf error displaces the root by ~|err·pdf·λ| / d'' — give it 10⁴ window
    // widths, bounded by a σ-scale sanity limit.
    let sigma_min = current.sigma().to_f64().min(candidate.sigma().to_f64());
    let max_drift = (sigma_min * 0.05).max(1e-6);

    let selector = |name: &'static str| -> ContractResult<Felt> {
        get_selector_from_name(name).map_err(|e| ContractError::Provider(format!("selector: {e}")))
    };
    let ctx = ProbeContext {
        runtime_class,
        current_raw: current.to_raw(),
        candidate_raw: quote.candidate,
        supplied: quote.padded_collateral,
        k: k_raw,
        backing: lp_info.total_backing_deposited,
        tolerance: params.tolerance,
        min_trade_collateral: params.min_trade_collateral,
        current_hints,
        candidate_hints: quote.candidate_hints,
        fee_inputs: (config.internal_decimals, config.decimal_shift, fee_config),
        check_selector: selector("check_trade_view")?,
        lot_selector: selector("compute_trade_lot_value_view")?,
        fees_selector: selector("compute_deposit_fees_view")?,
        deploy_selector: selector("deployContract")?,
    };

    let mut x_t = x0;
    let mut net_rate: Option<f64> = None;
    for round in 1..=MAX_ROUNDS {
        let x_raw = Sq128::from_f64(x_t).map_err(ContractError::Core)?.to_raw();
        let result = run_round(account, &ctx, x_raw, &[x_raw], round == 1).await?;
        if result.net_rate.is_some() {
            net_rate = result.net_rate;
        }

        if let Some(check) = result.checks.first().copied().filter(chain_accepts) {
            return Ok(Some(ProbeOutcome {
                x_star: x_raw,
                offset: x_t - x0,
                computed_collateral: check.verification.computed_collateral,
                net_rate: net_rate.unwrap_or(1.0),
                check,
                rounds: round,
            }));
        }

        // Reconstruct the chain's d'(x_t) from its own pdf values — the
        // outer multipliers are exact:
        //   d'(x) = λ_g·(−(x−μ_g)/σ_g²)·pdf_g(x) − λ_f·(−(x−μ_f)/σ_f²)·pdf_f(x)
        let g_term = lambda_g * (-(x_t - mu_g) / var_g);
        let f_term = lambda_f * (-(x_t - mu_f) / var_f);
        let chain_dprime = g_term.mul_add(result.chain_pdf_g, -(f_term * result.chain_pdf_f));
        let Some(d2) = d_double_prime_scaled(&current, &candidate, lambda_f, lambda_g, x_t)
            .filter(|v| v.is_finite() && v.abs() > 1e-12)
        else {
            tracing::debug!(round, x_t, "chain probe: degenerate d'' — giving up");
            return Ok(None);
        };
        let step = -chain_dprime / d2;
        let x_next = x_t + step;
        if !x_next.is_finite() || (x_next - x0).abs() > max_drift {
            tracing::debug!(
                round,
                x_t,
                step,
                "chain probe: Newton step drifted out of bounds — giving up"
            );
            return Ok(None);
        }
        // Converged in x but still rejected (e.g. window narrower than the
        // pdf-error jitter): no point iterating further.
        if step.abs() < tolerance.max(1e-9) * 1e-9 {
            tracing::debug!(round, x_t, "chain probe: converged without acceptance");
            return Ok(None);
        }
        x_t = x_next;
    }
    Ok(None)
}

// ─── Lognormal ───────────────────────────────────────────────────────────────

/// A chain-certified lognormal `x*` produced by [`refine_lognormal_quote`].
#[derive(Debug, Clone, Copy)]
pub struct LognormalProbeOutcome {
    /// The accepted stationary point (natural frame, `x* > 0`).
    pub x_star: Sq128Raw,
    /// Offset (in `x` units) from the off-chain root that the chain accepted.
    pub offset: f64,
    /// The chain's net collateral requirement at `x_star` (effective-k).
    pub computed_collateral: Sq128Raw,
    /// `net_collateral / supplied` deposit-fee rate (see
    /// [`ProbeOutcome::net_rate`]).
    pub net_rate: f64,
    /// **Chain-computed** candidate hints (`compute_hints_view`) — byte-exact
    /// for `execute_trade`, no off-chain closed form involved.
    pub candidate_hints: crate::types::lognormal::LognormalSqrtHintsRaw,
    /// Full verdict at the accepted point.
    pub check: crate::types::lognormal::LognormalTradeCheckRaw,
    /// Number of simulation rounds spent (incl. the measurement prelude).
    pub rounds: usize,
}

/// Lognormal probe context (the lognormal runtime shares the deposit-fee,
/// effective-k and check entry-point shapes with the normal runtime; hints
/// and λs are fetched from the chain itself rather than recomputed).
struct LognormalProbeContext {
    runtime_class: Felt,
    current_raw: deadeye_core::distribution::LognormalDistributionRaw,
    candidate_raw: deadeye_core::distribution::LognormalDistributionRaw,
    supplied: Sq128Raw,
    k: Sq128Raw,
    backing: Sq128Raw,
    tolerance: Sq128Raw,
    min_trade_collateral: Sq128Raw,
    current_hints: crate::types::lognormal::LognormalSqrtHintsRaw,
    fee_inputs: (u8, u8, crate::types::common::FeeConfigRaw),
    hints_selector: Felt,
    lambda_selector: Felt,
    compact_selector: Felt,
    check_selector: Felt,
    fees_selector: Felt,
    deploy_selector: Felt,
}

impl LognormalProbeContext {
    fn deploy_call(&self, salt: Felt) -> Call {
        Call {
            to: UDC_ADDRESS,
            selector: self.deploy_selector,
            calldata: vec![self.runtime_class, salt, Felt::ZERO, Felt::ZERO],
        }
    }

    /// `compute_position_value_compact_view` with `{orig_λ=0, eff_λ=1,
    /// eff=dist}` — returns the chain's `pdf_dist(x)` verbatim (the compact
    /// value is `eff_λ·pdf_eff(x) − orig_λ·pdf_orig(x)`).
    fn pdf_call(
        &self,
        to: Felt,
        dist: &deadeye_core::distribution::LognormalDistributionRaw,
        x: Sq128Raw,
    ) -> Call {
        let one = Sq128::from_i128(1).to_raw();
        let zero = Sq128::ZERO.to_raw();
        let mut cd = Vec::with_capacity(51);
        dist.mu.encode(&mut cd); // original_* (λ=0 ⇒ value-irrelevant)
        dist.variance.encode(&mut cd);
        dist.sigma.encode(&mut cd);
        zero.encode(&mut cd); // original_lambda = 0
        dist.mu.encode(&mut cd); // effective_*
        dist.variance.encode(&mut cd);
        dist.sigma.encode(&mut cd);
        one.encode(&mut cd); // effective_lambda = 1
        zero.encode(&mut cd); // total_collateral
        cd.push(Felt::ONE); // flags = POSITION_FLAG_HAS_POSITION
        x.encode(&mut cd);
        Call {
            to,
            selector: self.compact_selector,
            calldata: cd,
        }
    }

    fn hints_call(&self, to: Felt) -> Call {
        let mut cd = Vec::with_capacity(15);
        self.candidate_raw.encode(&mut cd);
        Call {
            to,
            selector: self.hints_selector,
            calldata: cd,
        }
    }

    fn lambda_call(
        &self,
        to: Felt,
        dist: &deadeye_core::distribution::LognormalDistributionRaw,
    ) -> Call {
        let mut cd = Vec::with_capacity(20);
        dist.encode(&mut cd);
        self.k.encode(&mut cd);
        Call {
            to,
            selector: self.lambda_selector,
            calldata: cd,
        }
    }

    fn fees_call(&self, to: Felt) -> Call {
        let (internal_decimals, decimal_shift, fee_config) = self.fee_inputs;
        let mut cd = Vec::with_capacity(10);
        self.supplied.encode(&mut cd);
        cd.push(Felt::from(internal_decimals));
        cd.push(Felt::from(decimal_shift));
        fee_config.encode(&mut cd);
        Call {
            to,
            selector: self.fees_selector,
            calldata: cd,
        }
    }

    fn check_call(
        &self,
        to: Felt,
        x: Sq128Raw,
        candidate_hints: &crate::types::lognormal::LognormalSqrtHintsRaw,
    ) -> Call {
        let mut cd = Vec::with_capacity(80);
        self.current_raw.encode(&mut cd);
        self.candidate_raw.encode(&mut cd);
        x.encode(&mut cd);
        self.supplied.encode(&mut cd);
        self.k.encode(&mut cd);
        self.backing.encode(&mut cd);
        self.tolerance.encode(&mut cd);
        self.min_trade_collateral.encode(&mut cd);
        self.current_hints.encode(&mut cd);
        candidate_hints.encode(&mut cd);
        Call {
            to,
            selector: self.check_selector,
            calldata: cd,
        }
    }
}

/// Decode an `Option<T>`-wrapped view result (`Some` tag is `0`).
fn decode_option_tail(inv: &FunctionInvocation, call: &'static str) -> ContractResult<Vec<Felt>> {
    if inv.result.first() != Some(&Felt::ZERO) {
        return Err(ContractError::InvalidResponse {
            call,
            message: "runtime returned None".into(),
        });
    }
    Ok(inv.result.get(1..).unwrap_or_default().to_vec())
}

/// Run one lognormal probe simulation and return the raw invocation list.
async fn run_lognormal_sim(
    account: &OwnedAccount,
    ctx: &LognormalProbeContext,
    build: impl Fn(Felt) -> Vec<Call>,
) -> ContractResult<Vec<FunctionInvocation>> {
    let mut last_revert: Option<String> = None;
    for attempt in 0_u64..2 {
        let salt = PROBE_SALT_BASE + Felt::from(attempt);
        let deployed = starknet_core::utils::get_udc_deployed_address(
            salt,
            ctx.runtime_class,
            &starknet_core::utils::UdcUniqueness::NotUnique,
            &[],
        );
        let mut calls = vec![ctx.deploy_call(salt)];
        calls.extend(build(deployed));
        let sim = account
            .inner()
            .execute_v3(calls)
            .simulate(true, true)
            .await
            .map_err(|e| ContractError::Provider(format!("probe simulate: {e}")))?;
        let TransactionTrace::Invoke(trace) = sim.transaction_trace else {
            return Err(ContractError::InvalidResponse {
                call: "chain_probe",
                message: "non-INVOKE simulation trace".into(),
            });
        };
        match trace.execute_invocation {
            ExecuteInvocation::Success(top) => return Ok(top.calls),
            ExecuteInvocation::Reverted(r) => last_revert = Some(r.revert_reason),
        }
    }
    Err(ContractError::Provider(format!(
        "probe simulation reverted: {}",
        last_revert.unwrap_or_else(|| "unknown".into())
    )))
}

/// Refine a lognormal-market quote's `x*` against the chain's own verifier.
///
/// Same Newton-against-the-chain strategy as [`refine_normal_quote`], with
/// the lognormal twists: candidate hints and λs are **fetched from the
/// chain** (`compute_hints_view` / `compute_lambda_view` on the deployed-in-
/// simulation runtime — no off-chain closed form), and the pdf oracle is
/// `compute_position_value_compact_view`. Returns the certified `x*`, the
/// chain's collateral + deposit-fee rate, and the chain-exact candidate
/// hints to submit with `execute_trade`.
#[instrument(skip_all, fields(market = %reader.address()))]
pub async fn refine_lognormal_quote<P>(
    account: &OwnedAccount,
    reader: &crate::lognormal_amm::LognormalMarketReader<P>,
    quote: &crate::lognormal_amm::LognormalTradeQuote,
) -> ContractResult<Option<LognormalProbeOutcome>>
where
    P: Provider + Sync,
{
    let params = reader.params().await?;
    let current = reader.distribution().await?;
    let current_hints = reader.distribution_hints().await?;
    let runtime_class = reader.runtime_class_hash().await?;
    let lp_info = reader.lp_info().await?;
    let config = reader.config().await?;
    let fee_config = reader.fee_config().await?;

    let candidate = deadeye_core::LognormalDistribution::from_raw(quote.candidate)
        .map_err(ContractError::Core)?;
    let x0 = Sq128::from_raw(quote.x_star).to_f64();
    let tolerance = Sq128::from_raw(params.tolerance).to_f64();
    let base_k = Sq128::from_raw(params.k).to_f64();
    let pool_backing = Sq128::from_raw(lp_info.total_backing_deposited).to_f64();
    let initial_backing = Sq128::from_raw(params.backing).to_f64();
    let k = if initial_backing > 0.0 && pool_backing.is_finite() {
        (base_k * pool_backing / initial_backing).max(base_k)
    } else {
        base_k
    };
    let k_raw = Sq128::from_f64(k).map_err(ContractError::Core)?.to_raw();
    let max_drift = (x0.abs() * 0.05).max(1e-6);

    let selector = |name: &'static str| -> ContractResult<Felt> {
        get_selector_from_name(name).map_err(|e| ContractError::Provider(format!("selector: {e}")))
    };
    let ctx = LognormalProbeContext {
        runtime_class,
        current_raw: current.to_raw(),
        candidate_raw: quote.candidate,
        supplied: quote.padded_collateral,
        k: k_raw,
        backing: lp_info.total_backing_deposited,
        tolerance: params.tolerance,
        min_trade_collateral: params.min_trade_collateral,
        current_hints,
        fee_inputs: (config.internal_decimals, config.decimal_shift, fee_config),
        hints_selector: selector("compute_hints_view")?,
        lambda_selector: selector("compute_lambda_view")?,
        compact_selector: selector("compute_position_value_compact_view")?,
        check_selector: selector("check_trade_view")?,
        fees_selector: selector("compute_deposit_fees_view")?,
        deploy_selector: selector("deployContract")?,
    };

    // Prelude round: chain-exact candidate hints + λs + fee rate + pdfs(x0).
    let x0_raw = Sq128::from_f64(x0).map_err(ContractError::Core)?.to_raw();
    let prelude = run_lognormal_sim(account, &ctx, |to| {
        vec![
            ctx.hints_call(to),
            ctx.lambda_call(to, &ctx.current_raw),
            ctx.lambda_call(to, &ctx.candidate_raw),
            ctx.fees_call(to),
            ctx.pdf_call(to, &ctx.current_raw, x0_raw),
            ctx.pdf_call(to, &ctx.candidate_raw, x0_raw),
        ]
    })
    .await?;
    // prelude[0] is the UDC deploy; the rest follow build order.
    let decode_sq = |inv: &FunctionInvocation, call: &'static str| -> ContractResult<f64> {
        let tail = decode_option_tail(inv, call)?;
        let (sq, _) = Sq128Raw::decode(&tail).map_err(ContractError::from)?;
        Ok(Sq128::from_raw(sq).to_f64())
    };
    let hints_tail = decode_option_tail(&prelude[1], "compute_hints_view")?;
    let (candidate_hints, _) = crate::types::lognormal::LognormalSqrtHintsRaw::decode(&hints_tail)
        .map_err(ContractError::from)?;
    let lambda_f = decode_sq(&prelude[2], "compute_lambda_view(current)")?;
    let lambda_g = decode_sq(&prelude[3], "compute_lambda_view(candidate)")?;
    let net_rate = {
        let tail = decode_option_tail(&prelude[4], "compute_deposit_fees_view")?;
        let net_slice = tail
            .get(6..)
            .ok_or_else(|| ContractError::InvalidResponse {
                call: "compute_deposit_fees_view",
                message: "fee result too short".into(),
            })?;
        let (net, _) = Sq128Raw::decode(net_slice).map_err(ContractError::from)?;
        let gross = Sq128::from_raw(quote.padded_collateral).to_f64();
        let net_f = Sq128::from_raw(net).to_f64();
        if gross > 0.0 && net_f > 0.0 && net_f <= gross {
            net_f / gross
        } else {
            1.0
        }
    };
    let mut pdf_f = decode_sq(&prelude[5], "pdf probe (current)")?;
    let mut pdf_g = decode_sq(&prelude[6], "pdf probe (candidate)")?;

    // Newton iterates: reconstruct the chain's d'(x) from its own pdf values
    // using the f64 log-density slope m(x) = pdf'(x)/pdf(x), step, and ask
    // check_trade_view for the verdict at each new point.
    let multiplier = |dist: &deadeye_core::LognormalDistribution, x: f64| -> Option<f64> {
        let x_q = Sq128::from_f64(x).ok()?;
        let p = dist.pdf(x_q).ok()?.to_f64();
        let dp = dist.pdf_derivative(x_q).ok()?.to_f64();
        if p.abs() < 1e-300 { None } else { Some(dp / p) }
    };
    let d2_at = |x: f64| -> Option<f64> {
        let x_q = Sq128::from_f64(x).ok()?;
        let f_pp = current.pdf_second_derivative(x_q).ok()?.to_f64();
        let g_pp = candidate.pdf_second_derivative(x_q).ok()?.to_f64();
        Some(lambda_g.mul_add(g_pp, -(lambda_f * f_pp)))
    };

    let mut x_prev = x0;
    for round in 2..=MAX_ROUNDS + 1 {
        let (Some(m_f), Some(m_g), Some(d2)) = (
            multiplier(&current, x_prev),
            multiplier(&candidate, x_prev),
            d2_at(x_prev).filter(|v| v.is_finite() && v.abs() > 1e-12),
        ) else {
            tracing::debug!(round, x_prev, "lognormal probe: degenerate derivatives");
            return Ok(None);
        };
        let g_term = lambda_g * m_g;
        let f_term = lambda_f * m_f;
        let chain_dprime = g_term.mul_add(pdf_g, -(f_term * pdf_f));
        let step = -chain_dprime / d2;
        let x_t = x_prev + step;
        if !x_t.is_finite() || x_t <= 0.0 || (x_t - x0).abs() > max_drift {
            tracing::debug!(round, x_prev, step, "lognormal probe: step out of bounds");
            return Ok(None);
        }
        let x_prev_raw = Sq128::from_f64(x_prev)
            .map_err(ContractError::Core)?
            .to_raw();
        let x_t_raw = Sq128::from_f64(x_t).map_err(ContractError::Core)?.to_raw();
        let calls = run_lognormal_sim(account, &ctx, |to| {
            vec![
                ctx.pdf_call(to, &ctx.current_raw, x_t_raw),
                ctx.pdf_call(to, &ctx.candidate_raw, x_t_raw),
                ctx.check_call(to, x_prev_raw, &candidate_hints),
                ctx.check_call(to, x_t_raw, &candidate_hints),
            ]
        })
        .await?;
        let decode_check = |inv: &FunctionInvocation| -> ContractResult<
            crate::types::lognormal::LognormalTradeCheckRaw,
        > {
            let (check, _) = crate::types::lognormal::LognormalTradeCheckRaw::decode(&inv.result)
                .map_err(ContractError::from)?;
            Ok(check)
        };
        for (x_candidate, raw, idx) in [(x_prev, x_prev_raw, 3_usize), (x_t, x_t_raw, 4_usize)] {
            let check = decode_check(&calls[idx])?;
            if lognormal_chain_accepts(&check) {
                return Ok(Some(LognormalProbeOutcome {
                    x_star: raw,
                    offset: x_candidate - x0,
                    computed_collateral: check.verification.computed_collateral,
                    net_rate,
                    candidate_hints,
                    check,
                    rounds: round,
                }));
            }
        }
        pdf_f = decode_sq(&calls[1], "pdf probe (current)")?;
        pdf_g = decode_sq(&calls[2], "pdf probe (candidate)")?;
        if step.abs() < tolerance.max(1e-9) * 1e-9 {
            tracing::debug!(round, x_t, "lognormal probe: converged without acceptance");
            return Ok(None);
        }
        x_prev = x_t;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::{CollateralVerificationRaw, ScaledBackingCheckRaw, TradeRejection};

    fn sq(n: u64) -> Sq128Raw {
        Sq128Raw {
            limb0: 0,
            limb1: 0,
            limb2: n,
            limb3: 0,
            neg: false,
        }
    }

    fn check(stationary: bool, argmin: bool) -> TradeCheckRaw {
        TradeCheckRaw {
            backing_check: ScaledBackingCheckRaw {
                max_value_upper: sq(1),
                is_valid: true,
                computation_succeeded: true,
            },
            verification: CollateralVerificationRaw {
                side_valid: true,
                stationary_valid: stationary,
                curvature_valid: true,
                computed_collateral: sq(131),
                collateral_sufficient: false, // deliberately ignored by accept
                computation_valid: true,
                argmin_value_valid: argmin,
            },
            min_trade_collateral: sq(1),
            collateral_above_min: true,
            is_valid: false,
            rejection_reason: TradeRejection::VerificationFailed,
        }
    }

    #[test]
    fn accept_requires_stationary_and_argmin_but_not_collateral() {
        // collateral_sufficient=false must NOT veto acceptance — the caller
        // re-sizes the supply from the chain's computed_collateral.
        assert!(chain_accepts(&check(true, true)));
        assert!(!chain_accepts(&check(false, true)));
        assert!(!chain_accepts(&check(true, false)));
    }

    #[test]
    fn lambda_matches_chain_value_for_live_cpi_sigmas() {
        // Verified live against compute_lambda_view on the deployed runtime:
        // λ(0.19198107, 200) = 164.99153508, λ(0.12, 200) = 130.44369271.
        assert!((lambda(0.191_981_067_900_659_27, 200.0) - 164.991_535_08).abs() < 1e-6);
        assert!((lambda(0.12, 200.0) - 130.443_692_71).abs() < 1e-6);
    }

    #[test]
    fn pdf_call_encodes_a_53_felt_lot_probe() {
        let ctx_dist = NormalDistributionRaw {
            mean: sq(4),
            variance: sq(1),
            sigma: sq(1),
        };
        let ctx = ProbeContext {
            runtime_class: Felt::from(7_u64),
            current_raw: ctx_dist,
            candidate_raw: ctx_dist,
            supplied: sq(5),
            k: sq(200),
            backing: sq(1000),
            tolerance: sq(1),
            min_trade_collateral: sq(1),
            current_hints: deadeye_core::distribution::NormalSqrtHintsRaw {
                l2_norm_denom: sq(1),
                backing_denom: sq(1),
            },
            candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw {
                l2_norm_denom: sq(1),
                backing_denom: sq(1),
            },
            fee_inputs: (18, 12, crate::types::common::FeeConfigRaw {
                lp_fee_bps: 100,
                protocol_fee_bps: 20,
                settlement_fee_bps: 50,
            }),
            check_selector: Felt::from(1_u64),
            lot_selector: Felt::from(2_u64),
            fees_selector: Felt::from(4_u64),
            deploy_selector: Felt::from(3_u64),
        };
        let call = ctx.pdf_call(Felt::from(9_u64), &ctx_dist, sq(4));
        // lot_id(1) + trader(1) + 9×Sq128Raw(5) + flags(1) + x(5) = 53.
        assert_eq!(call.calldata.len(), 53);
        // flags felt must be EXISTS=1, immediately before the 5-felt x tail.
        assert_eq!(call.calldata[47], Felt::ONE);
        // to_lambda = 1.0 lives in limb2 of the 5-felt group before flags.
        assert_eq!(call.calldata[44], Felt::from(1_u64));
    }

    #[test]
    fn check_call_encodes_80_felts() {
        let dist = NormalDistributionRaw {
            mean: sq(4),
            variance: sq(1),
            sigma: sq(1),
        };
        let ctx = ProbeContext {
            runtime_class: Felt::from(7_u64),
            current_raw: dist,
            candidate_raw: dist,
            supplied: sq(5),
            k: sq(200),
            backing: sq(1000),
            tolerance: sq(1),
            min_trade_collateral: sq(1),
            current_hints: deadeye_core::distribution::NormalSqrtHintsRaw {
                l2_norm_denom: sq(1),
                backing_denom: sq(1),
            },
            candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw {
                l2_norm_denom: sq(1),
                backing_denom: sq(1),
            },
            fee_inputs: (18, 12, crate::types::common::FeeConfigRaw {
                lp_fee_bps: 100,
                protocol_fee_bps: 20,
                settlement_fee_bps: 50,
            }),
            check_selector: Felt::from(1_u64),
            lot_selector: Felt::from(2_u64),
            fees_selector: Felt::from(4_u64),
            deploy_selector: Felt::from(3_u64),
        };
        let call = ctx.check_call(Felt::from(9_u64), sq(4));
        // 2×dist(30) + x(5) + supplied(5) + k/backing/tol/min(20) + 2×hints(20).
        assert_eq!(call.calldata.len(), 80);
    }

    /// LIVE VALIDATION (gated): run the production [`refine_lognormal_quote`]
    /// against the **deployed v0.13 lognormal math-runtime class** on mainnet
    /// (no live lognormal market exists yet, so the market views are canned
    /// with realistic state and the chain-exact current hints are fetched in
    /// a prelude simulation). Proves the Newton loop converges to a
    /// chain-certified `x*` for a lognormal trade, gas-free. Run with:
    /// `DEADEYE_LIVE_SIM=1 cargo test -p deadeye-starknet --all-features \
    ///   live_lognormal_probe -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live mainnet simulation — run manually with DEADEYE_LIVE_SIM=1"]
    #[expect(
        clippy::print_stderr,
        clippy::unwrap_used,
        clippy::panic,
        reason = "diagnostic validation tool"
    )]
    async fn live_lognormal_probe_certifies_xstar() {
        use std::sync::Mutex;

        use async_trait::async_trait;
        use starknet_accounts::{Account as _, ExecutionEncoding, SingleOwnerAccount};
        use starknet_core::types::{BlockId, BlockTag, FunctionCall};
        use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
        use starknet_signers::{LocalWallet, SigningKey};

        use crate::types::common::{AmmConfigRaw, AmmParamsRaw, FeeConfigRaw, LpInfoRaw};

        struct SeqProvider {
            responses: Mutex<Vec<Vec<Felt>>>, // popped front-to-back
        }
        #[async_trait]
        impl Provider for SeqProvider {
            async fn call(
                &self,
                _call: FunctionCall,
                _block: BlockId,
            ) -> ContractResult<Vec<Felt>> {
                let mut g = self.responses.lock().unwrap();
                if g.is_empty() {
                    return Err(ContractError::Provider("canned responses exhausted".into()));
                }
                Ok(g.remove(0))
            }
        }
        if std::env::var("DEADEYE_LIVE_SIM").is_err() {
            eprintln!("skipped (set DEADEYE_LIVE_SIM=1)");
            return;
        }
        let rpc = "https://api.zan.top/public/starknet-mainnet/rpc/v0_10";
        // v0.13 lognormal math-runtime class (deployment-mainnet.json).
        let runtime_class =
            Felt::from_hex("0x7dcbf032695bf2cc60fa124d9271111e178f295c5e92c7a42530902d0fcb1c6")
                .unwrap();
        let deployer =
            Felt::from_hex("0x77b277249a962ad47b04cc60bda09625c1258bbcc3dbab613d23a68833d8f0")
                .unwrap();
        let url = url::Url::parse(rpc).unwrap();

        // Synthetic-but-realistic lognormal market state (median 1.0).
        let current = deadeye_core::LognormalDistribution::from_variance(
            deadeye_core::Sq128::ZERO,
            Sq128::from_f64(0.0625).unwrap(), // σ_log = 0.25
        )
        .unwrap();
        let candidate = deadeye_core::LognormalDistribution::from_variance(
            Sq128::from_f64(-0.02).unwrap(),
            Sq128::from_f64(0.0484).unwrap(), // σ_log = 0.22
        )
        .unwrap();

        // Prelude: chain-exact hints for the synthetic CURRENT distribution
        // (the verifier's λ_f hint-residual check needs them byte-exact).
        let acct_provider = JsonRpcClient::new(HttpTransport::new(url.clone()));
        let signer = LocalWallet::from(SigningKey::from_secret_scalar(Felt::from(2_u32)));
        let chain_id = Felt::from_hex("0x534e5f4d41494e").unwrap();
        let mut account = SingleOwnerAccount::new(
            acct_provider,
            signer,
            deployer,
            chain_id,
            ExecutionEncoding::New,
        );
        account.set_block_id(BlockId::Tag(BlockTag::PreConfirmed));
        let salt = PROBE_SALT_BASE;
        let deployed = starknet_core::utils::get_udc_deployed_address(
            salt,
            runtime_class,
            &starknet_core::utils::UdcUniqueness::NotUnique,
            &[],
        );
        let mut hints_cd = Vec::new();
        current.to_raw().encode(&mut hints_cd);
        let sim = account
            .execute_v3(vec![
                Call {
                    to: UDC_ADDRESS,
                    selector: get_selector_from_name("deployContract").unwrap(),
                    calldata: vec![runtime_class, salt, Felt::ZERO, Felt::ZERO],
                },
                Call {
                    to: deployed,
                    selector: get_selector_from_name("compute_hints_view").unwrap(),
                    calldata: hints_cd,
                },
            ])
            .simulate(true, true)
            .await
            .expect("prelude simulate");
        let TransactionTrace::Invoke(trace) = sim.transaction_trace else {
            panic!("non-invoke trace");
        };
        let ExecuteInvocation::Success(top) = trace.execute_invocation else {
            panic!("prelude reverted");
        };
        let tail = decode_option_tail(&top.calls[1], "compute_hints_view").unwrap();
        let (current_hints, _) =
            crate::types::lognormal::LognormalSqrtHintsRaw::decode(&tail).unwrap();
        eprintln!(
            "chain current hints: l2={:.10} backing={:.10}",
            Sq128::from_raw(current_hints.l2_norm_denom).to_f64(),
            Sq128::from_raw(current_hints.backing_denom).to_f64(),
        );

        // Canned market views in the exact order refine_lognormal_quote
        // reads them: params, distribution, distribution_hints,
        // runtime_class_hash, lp_info, config, fee_config.
        let params = AmmParamsRaw {
            k: Sq128::from_f64(200.0).unwrap().to_raw(),
            backing: Sq128::from_f64(1000.0).unwrap().to_raw(),
            tolerance: Sq128::from_f64(1e-3).unwrap().to_raw(),
            min_trade_collateral: Sq128::from_f64(0.01).unwrap().to_raw(),
        };
        let lp = LpInfoRaw {
            total_shares: Sq128::from_f64(1000.0).unwrap().to_raw(),
            total_backing_deposited: Sq128::from_f64(1000.0).unwrap().to_raw(),
        };
        let config = AmmConfigRaw {
            collateral_token: crate::collateral::MAINNET_XP_TOKEN_ADDRESS,
            token_decimals: 18,
            internal_decimals: 6,
            decimal_shift: 12,
            params,
        };
        let fee_config = FeeConfigRaw {
            lp_fee_bps: 100,
            protocol_fee_bps: 20,
            settlement_fee_bps: 50,
        };
        let provider = SeqProvider {
            responses: Mutex::new(vec![
                params.to_calldata(),
                current.to_raw().to_calldata(),
                current_hints.to_calldata(),
                vec![runtime_class],
                lp.to_calldata(),
                config.to_calldata(),
                fee_config.to_calldata(),
            ]),
        };
        let reader =
            crate::lognormal_amm::LognormalMarketReader::new(provider, Felt::from(0x1234_u64));

        // Off-chain draft, exactly as the CLI builds it.
        let solved = deadeye_collateral::lognormal_collateral(
            &current,
            &candidate,
            deadeye_collateral::LognormalOptions::default(),
        )
        .expect("offline lognormal solver");
        eprintln!(
            "offline x0 = {:.12}, collateral est = {:.4}",
            solved.x_star, solved.collateral
        );
        let quote = crate::lognormal_amm::LognormalTradeQuote {
            candidate: candidate.to_raw(),
            candidate_hints: crate::types::lognormal::LognormalSqrtHintsRaw {
                l2_norm_denom: Sq128::ZERO.to_raw(),
                backing_denom: Sq128::ZERO.to_raw(),
            },
            x_star: Sq128::from_f64(solved.x_star).unwrap().to_raw(),
            required_collateral: Sq128::from_f64(solved.collateral).unwrap().to_raw(),
            padded_collateral: Sq128::from_f64(500.0).unwrap().to_raw(),
            on_chain_will_accept: true,
            rejection: None,
        };

        let owned = OwnedAccount::from_signing_key(
            JsonRpcClient::new(HttpTransport::new(url)),
            deployer,
            Felt::from(2_u32),
            chain_id,
        );
        let outcome = refine_lognormal_quote(&owned, &reader, &quote)
            .await
            .expect("probe infrastructure");
        let outcome = outcome.expect("chain must certify an x* for this candidate");
        let v = &outcome.check.verification;
        eprintln!(
            "CERTIFIED: x*={:.12} (offset {:+.3e}, {} rounds)  collateral={:.4} net  \
             fees={:.2}%  hints l2={:.8}",
            Sq128::from_raw(outcome.x_star).to_f64(),
            outcome.offset,
            outcome.rounds,
            Sq128::from_raw(outcome.computed_collateral).to_f64(),
            (1.0 - outcome.net_rate) * 100.0,
            Sq128::from_raw(outcome.candidate_hints.l2_norm_denom).to_f64(),
        );
        assert!(v.side_valid && v.stationary_valid && v.curvature_valid);
        assert!(v.computation_valid && v.argmin_value_valid);
        assert!(Sq128::from_raw(outcome.computed_collateral).to_f64() > 0.0);
    }
}
