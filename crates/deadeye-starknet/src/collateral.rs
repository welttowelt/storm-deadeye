//! View + write clients for the Deadeye `restricted_collateral_token`.
//!
//! The collateral token (XP on mainnet) is an ERC-20
//! that the Deadeye AMMs accept as `transfer_from` source on every trade.
//! It additionally exposes a one-shot per-account `claim_initial_grant`
//! entrypoint — a fixed-size mint that lets a fresh wallet obtain enough
//! collateral to participate in markets without an out-of-band funding
//! step.
//!
//! ## What this module is for
//!
//! A bot (e.g. `cpi-arb-bot`) that wants to trade against a deployed
//! market needs XP in the signer wallet. The chain side of that grant is
//! `claim_initial_grant()` — a zero-arg external that mints
//! `initial_grant()` tokens to the caller iff
//! `has_claimed_initial_grant(caller) == false`. This module provides:
//!
//! 1. [`CollateralTokenReader`] — typed view calls for `balance_of`,
//!    `allowance`, `initial_grant`, `has_claimed_initial_grant`, plus the
//!    registered-markets surface.
//! 2. [`CollateralTokenWriter`] — pairs a reader with an [`Account`] and
//!    exposes `claim_initial_grant` + `approve` as one-call submits.
//!
//! ## Idempotency
//!
//! `claim_initial_grant` reverts on a second call from the same wallet.
//! Callers should preflight with
//! [`CollateralTokenReader::has_claimed_initial_grant`] and skip the submit
//! when it returns `true` (see the cpi-bot's `claim_grant` subcommand for the
//! canonical shape).
//!
//! ## Address constants
//!
//! The mainnet XP address is pinned at compile time — it's part of the
//! deployment artifact and won't change without a redeploy. Callers
//! targeting other networks (devnet) must supply the address
//! explicitly.

use starknet_core::types::{Felt, FunctionCall, U256};
use tracing::instrument;

use crate::{
    account::Account,
    cairo_serde::{CairoSerde, CairoSerdeError},
    error::{ContractError, ContractResult},
    execution::{Call, ExecutionReceipt},
    provider::Provider,
};

/// Mainnet `XP` collateral token address (pinned from
/// `deployment-mainnet.json`).
pub const MAINNET_XP_TOKEN_ADDRESS: Felt =
    Felt::from_hex_unchecked("0x6b363ef72144d0a39a2821eb11d7c4fa760eee4c8d3e81dd5bcc2ae58f7ea64");

/// ABI-decoded `core::integer::u256` returned from view calls.
///
/// Wraps [`starknet_core::types::U256`] so the standard `CairoSerde`
/// impl in this crate can carry it across the trait boundary without
/// needing an orphan-rule workaround. Use [`U256Value::into_inner`]
/// (or the `From` impls) to get the bare `U256` for arithmetic /
/// display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct U256Value(pub U256);

impl U256Value {
    /// Extract the wrapped value.
    #[must_use]
    pub const fn into_inner(self) -> U256 {
        self.0
    }
}

impl From<U256Value> for U256 {
    fn from(v: U256Value) -> Self {
        v.0
    }
}

impl From<U256> for U256Value {
    fn from(v: U256) -> Self {
        Self(v)
    }
}

impl CairoSerde for U256Value {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(Felt::from(self.0.low()));
        out.push(Felt::from(self.0.high()));
    }

    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (low, rest) = u128::decode(slice)?;
        let (high, rest) = u128::decode(rest)?;
        Ok((Self(U256::from_words(low, high)), rest))
    }
}

/// Selectors used by the collateral token.
mod selectors {
    use std::sync::LazyLock;

    use starknet_core::{types::Felt, utils::get_selector_from_name};

    fn compute(name: &'static str) -> Felt {
        get_selector_from_name(name).expect("entry-point name must be a valid Cairo identifier")
    }

    pub(super) fn balance_of() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("balance_of"));
        *V
    }
    pub(super) fn allowance() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("allowance"));
        *V
    }
    pub(super) fn total_supply() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("total_supply"));
        *V
    }
    pub(super) fn initial_grant() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("initial_grant"));
        *V
    }
    pub(super) fn has_claimed_initial_grant() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("has_claimed_initial_grant"));
        *V
    }
    pub(super) fn is_market_registered() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("is_market_registered"));
        *V
    }
    pub(super) fn is_market_enabled() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("is_market_enabled"));
        *V
    }
    pub(super) fn claim_initial_grant() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("claim_initial_grant"));
        *V
    }
    pub(super) fn approve() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| compute("approve"));
        *V
    }
}

/// Typed read accessors for a deployed collateral token.
#[derive(Debug)]
pub struct CollateralTokenReader<P>
where
    P: Provider,
{
    provider: P,
    address: Felt,
}

impl<P> CollateralTokenReader<P>
where
    P: Provider,
{
    /// Bind to a specific collateral-token address.
    pub const fn new(provider: P, address: Felt) -> Self {
        Self { provider, address }
    }

    /// Bind to the mainnet XP token.
    pub const fn mainnet_xp(provider: P) -> Self {
        Self {
            provider,
            address: MAINNET_XP_TOKEN_ADDRESS,
        }
    }

    /// Contract address this reader targets.
    pub const fn address(&self) -> Felt {
        self.address
    }

    /// Borrow the underlying [`Provider`].
    pub const fn provider(&self) -> &P {
        &self.provider
    }

    /// ERC-20 `balance_of(account)`.
    #[instrument(skip(self), fields(token = %self.address, %account))]
    pub async fn balance_of(&self, account: Felt) -> ContractResult<U256> {
        self.call_view::<U256Value>("balance_of", selectors::balance_of(), &[account])
            .await
            .map(U256Value::into_inner)
    }

    /// ERC-20 `allowance(owner, spender)`.
    #[instrument(skip(self), fields(token = %self.address, %owner, %spender))]
    pub async fn allowance(&self, owner: Felt, spender: Felt) -> ContractResult<U256> {
        self.call_view::<U256Value>("allowance", selectors::allowance(), &[owner, spender])
            .await
            .map(U256Value::into_inner)
    }

    /// ERC-20 `total_supply()`.
    pub async fn total_supply(&self) -> ContractResult<U256> {
        self.call_view::<U256Value>("total_supply", selectors::total_supply(), &[])
            .await
            .map(U256Value::into_inner)
    }

    /// Fixed-amount initial grant minted by `claim_initial_grant()`.
    /// Returns the raw token amount (18 decimals on mainnet XP).
    #[instrument(skip(self), fields(token = %self.address))]
    pub async fn initial_grant(&self) -> ContractResult<U256> {
        self.call_view::<U256Value>("initial_grant", selectors::initial_grant(), &[])
            .await
            .map(U256Value::into_inner)
    }

    /// `true` iff `account` has already consumed its one-shot initial
    /// grant. A second `claim_initial_grant` from a wallet for which
    /// this returns `true` will revert on chain.
    #[instrument(skip(self), fields(token = %self.address, %account))]
    pub async fn has_claimed_initial_grant(&self, account: Felt) -> ContractResult<bool> {
        self.call_view::<bool>(
            "has_claimed_initial_grant",
            selectors::has_claimed_initial_grant(),
            &[account],
        )
        .await
    }

    /// `true` iff `market` has been registered with the token (admin op).
    pub async fn is_market_registered(&self, market: Felt) -> ContractResult<bool> {
        self.call_view::<bool>(
            "is_market_registered",
            selectors::is_market_registered(),
            &[market],
        )
        .await
    }

    /// `true` iff `market` is registered AND currently enabled.
    pub async fn is_market_enabled(&self, market: Felt) -> ContractResult<bool> {
        self.call_view::<bool>("is_market_enabled", selectors::is_market_enabled(), &[
            market,
        ])
        .await
    }

    async fn call_view<T>(
        &self,
        call_name: &'static str,
        selector: Felt,
        calldata: &[Felt],
    ) -> ContractResult<T>
    where
        T: CairoSerde,
    {
        let response = self
            .provider
            .call(
                FunctionCall {
                    contract_address: self.address,
                    entry_point_selector: selector,
                    calldata: calldata.to_vec(),
                },
                self.provider.default_block(),
            )
            .await?;
        let (value, rest) = T::decode(&response)?;
        if !rest.is_empty() {
            return Err(ContractError::UnexpectedReturnSize {
                call: call_name,
                actual: response.len(),
                expected: response.len() - rest.len(),
            });
        }
        Ok(value)
    }
}

/// Write-capable companion to [`CollateralTokenReader`]. Pairs the reader
/// with an [`Account`] so a single handle can pre-flight reads, build
/// calldata, and submit transactions in one place.
#[derive(Debug)]
pub struct CollateralTokenWriter<P, A>
where
    P: Provider,
    A: Account,
{
    reader: CollateralTokenReader<P>,
    account: A,
}

impl<P, A> CollateralTokenWriter<P, A>
where
    P: Provider,
    A: Account,
{
    /// Construct a writer from a reader and an account.
    pub const fn new(reader: CollateralTokenReader<P>, account: A) -> Self {
        Self { reader, account }
    }

    /// Borrow the underlying reader.
    pub const fn reader(&self) -> &CollateralTokenReader<P> {
        &self.reader
    }

    /// Borrow the underlying account.
    pub const fn account(&self) -> &A {
        &self.account
    }

    /// Build the [`Call`] for `claim_initial_grant()` without submitting.
    /// Exposed so callers can bundle the claim with subsequent calls
    /// (e.g. an `approve` for the market) into a single multicall.
    #[must_use]
    pub fn build_claim_initial_grant_call(&self) -> Call {
        Call {
            to: self.reader.address(),
            selector: selectors::claim_initial_grant(),
            calldata: Vec::new(),
        }
    }

    /// Build the [`Call`] for `approve(spender, amount)` without submitting.
    /// `amount` is encoded as a Cairo `u256` (two felts: `low, high`).
    #[must_use]
    pub fn build_approve_call(&self, spender: Felt, amount: U256) -> Call {
        let mut calldata = Vec::with_capacity(3);
        calldata.push(spender);
        U256Value(amount).encode(&mut calldata);
        Call {
            to: self.reader.address(),
            selector: selectors::approve(),
            calldata,
        }
    }

    /// Submit `claim_initial_grant()` from the caller wallet.
    ///
    /// Reverts on chain if the caller has already claimed. Pre-flight
    /// with [`CollateralTokenReader::has_claimed_initial_grant`] to
    /// avoid burning gas on a guaranteed revert.
    #[instrument(skip(self), fields(token = %self.reader.address(), kind = "claim_initial_grant"))]
    pub async fn claim_initial_grant(&self) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_claim_initial_grant_call()])
            .await
    }

    /// Submit `approve(spender, amount)` from the caller wallet.
    #[instrument(skip(self, amount), fields(token = %self.reader.address(), %spender, kind = "approve"))]
    pub async fn approve(&self, spender: Felt, amount: U256) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_approve_call(spender, amount)])
            .await
    }
}

/// Build a standalone `claim_initial_grant()` [`Call`] for `token` (no signer
/// needed).
///
/// Used to bootstrap a fresh wallet inside a trade multicall: when the
/// trader's XP balance cannot cover the collateral and the one-shot grant is
/// still unclaimed, the CLI prepends this call so
/// `[claim, approve, trade]` lands atomically. Reverts on chain if the caller
/// has already claimed — pre-flight with
/// [`CollateralTokenReader::has_claimed_initial_grant`].
#[must_use]
pub fn build_claim_initial_grant_call(token: Felt) -> Call {
    Call {
        to: token,
        selector: selectors::claim_initial_grant(),
        calldata: Vec::new(),
    }
}

/// Build a standalone ERC-20 `approve(spender, amount)` [`Call`] for `token`
/// (no signer needed).
///
/// `amount` is the allowance in the token's base units, encoded as a Cairo
/// `u256` (calldata `[spender, low, high]`). Used to bundle a collateral
/// approval into a trade multicall so the AMM's `transfer_from` of the
/// trader's collateral succeeds atomically.
#[must_use]
pub fn build_erc20_approve_call(token: Felt, spender: Felt, amount: U256) -> Call {
    let mut calldata = Vec::with_capacity(3);
    calldata.push(spender);
    U256Value(amount).encode(&mut calldata);
    Call {
        to: token,
        selector: selectors::approve(),
        calldata,
    }
}

/// Scale a human collateral amount (e.g. `133.26` XP) to the token's base
/// units (`× 10^decimals`) plus a `buffer_pct%` margin, rounded up, for an
/// ERC-20 allowance.
///
/// Over-approving by the buffer mirrors the webapp and absorbs any
/// quote/settlement rounding so `transfer_from` never reverts on a short
/// allowance. Returns `0` for non-positive input; saturates to `u128::MAX`
/// on overflow.
#[must_use]
pub fn collateral_allowance_base_units(human_amount: f64, decimals: u8, buffer_pct: u32) -> U256 {
    if !human_amount.is_finite() || human_amount <= 0.0 {
        return U256::from_words(0, 0);
    }
    let buffered = human_amount * f64::from(buffer_pct).mul_add(0.01, 1.0);
    let scale = 10f64.powi(i32::from(decimals));
    let base = (buffered * scale).ceil();
    #[expect(clippy::cast_precision_loss, reason = "upper-bound comparison only")]
    let max = u128::MAX as f64;
    let amount = if base.is_finite() && base >= 0.0 && base < max {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounded to [0, u128::MAX) by the guard above"
        )]
        let v = base as u128;
        v
    } else {
        u128::MAX
    };
    U256::from_words(amount, 0)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    #[test]
    fn mainnet_xp_address_is_pinned_constant() {
        // Defence in depth: catch a copy-paste typo in the embedded
        // hex literal. The deployment manifest is the source of truth;
        // if this constant ever drifts, the test fixture below will
        // need to be updated explicitly.
        let expected =
            Felt::from_hex("0x6b363ef72144d0a39a2821eb11d7c4fa760eee4c8d3e81dd5bcc2ae58f7ea64")
                .unwrap();
        assert_eq!(MAINNET_XP_TOKEN_ADDRESS, expected);
    }

    #[test]
    fn u256_value_round_trip() {
        // Both limbs non-zero — pin the (low, high) encoding so a
        // future refactor doesn't swap the limb order silently.
        let raw = U256::from_words(0x1234_5678_9abc_def0_1111_2222_3333_4444, 0xdead_beef);
        let cd = U256Value(raw).to_calldata();
        assert_eq!(cd.len(), 2, "u256 must serialise to exactly two felts");
        let (back, rest) = U256Value::decode(&cd).unwrap();
        assert_eq!(back.into_inner(), raw);
        assert!(rest.is_empty());
    }

    #[test]
    fn u256_value_max_round_trip() {
        // Unlimited-allowance sentinel: both limbs at u128::MAX.
        let raw = U256::from_words(u128::MAX, u128::MAX);
        let cd = U256Value(raw).to_calldata();
        let (back, _) = U256Value::decode(&cd).unwrap();
        assert_eq!(back.into_inner(), raw);
    }

    #[test]
    fn approve_calldata_encodes_spender_then_low_then_high() {
        // Pin the Cairo `approve(spender, u256)` ABI shape. A regression
        // that flipped the limb order (or dropped the high limb) would
        // make the chain revert with `Input too long for arguments`.
        let spender = Felt::from_hex("0xe322").unwrap();
        let amount = U256::from_words(7_u128, 11_u128);
        let mut calldata = Vec::new();
        calldata.push(spender);
        U256Value(amount).encode(&mut calldata);
        assert_eq!(calldata.len(), 3);
        assert_eq!(calldata[0], spender);
        assert_eq!(calldata[1], Felt::from(7_u64));
        assert_eq!(calldata[2], Felt::from(11_u64));
    }

    #[test]
    fn selectors_are_stable_and_distinct() {
        // Two memoised calls must yield the same felt; distinct entry
        // points must yield distinct selectors.
        assert_eq!(selectors::balance_of(), selectors::balance_of());
        assert_ne!(selectors::balance_of(), selectors::allowance());
        assert_ne!(
            selectors::claim_initial_grant(),
            selectors::has_claimed_initial_grant()
        );
        assert_ne!(selectors::approve(), selectors::balance_of());
    }

    #[test]
    fn build_erc20_approve_call_targets_token_with_spender_then_amount() {
        // The standalone approve Call must hit the *token* (not the market),
        // call `approve`, and carry [spender, low, high] — the shape the trade
        // multicall prepends to fix the transfer_from revert (#13).
        let token = Felt::from_hex("0x1d77ce77").unwrap();
        let spender = Felt::from_hex("0xba9a4b3").unwrap();
        let amount = U256::from_words(42_u128, 0);
        let call = build_erc20_approve_call(token, spender, amount);
        assert_eq!(call.to, token, "approve must target the token contract");
        assert_eq!(call.selector, selectors::approve());
        assert_eq!(call.calldata, vec![spender, Felt::from(42_u64), Felt::ZERO]);
    }

    #[test]
    fn collateral_allowance_scales_to_base_units_with_buffer() {
        // 133.26 XP at 18 decimals + 5% buffer ⇒ ≈ 139.92e18 base units,
        // strictly above the un-buffered 133.26e18 (so transfer_from of the
        // exact collateral can never exceed the allowance).
        let a = collateral_allowance_base_units(133.26, 18, 5);
        assert_eq!(a.high(), 0, "133 XP fits the low limb");
        let low = a.low();
        let exact = 133_260_000_000_000_000_000_u128; // 133.26 × 1e18
        assert!(
            low > exact,
            "allowance {low} must exceed the exact collateral {exact}",
        );
        // ~5% buffer, not wildly more.
        let upper = 141_000_000_000_000_000_000_u128; // 141 × 1e18
        assert!(
            low < upper,
            "allowance {low} unexpectedly large (> {upper})"
        );
    }

    #[test]
    fn collateral_allowance_is_zero_for_non_positive() {
        assert_eq!(
            collateral_allowance_base_units(0.0, 18, 5),
            U256::from_words(0, 0)
        );
        assert_eq!(
            collateral_allowance_base_units(-5.0, 18, 5),
            U256::from_words(0, 0)
        );
        assert_eq!(
            collateral_allowance_base_units(f64::NAN, 18, 5),
            U256::from_words(0, 0)
        );
    }
}
