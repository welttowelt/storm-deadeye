//! Error hierarchy shared across the Starknet contract bindings.
//!
//! ## Plumbing vs domain errors
//!
//! [`ContractError`] is the low-level error that every JSON-RPC call
//! produces — it covers transport failures, decode failures, and arbitrary
//! provider error strings.
//!
//! Production market-makers do not want to branch on `&str`. They need to
//! ask "did the chain reject my trade for a recoverable reason (e.g.
//! `STALE_STATE` — re-read and retry) or for a terminal one (e.g.
//! `MARKET_SETTLED` — abandon the position)?".
//!
//! [`TradeError`] is the higher-level variant returned by every
//! write-path on the AMM and factory writers. Its [`TradeError::Rejected`]
//! arm carries a typed [`TradeRejectionReason`] that the writer parses out
//! of the underlying revert-string felt; [`TradeError::Submission`] carries
//! the raw plumbing failure for everything else.

use deadeye_core::CoreError;
use starknet_core::{types::Felt, utils::parse_cairo_short_string};
use thiserror::Error;

use crate::cairo_serde::CairoSerdeError;

/// `Result` alias for contract calls.
pub type ContractResult<T> = core::result::Result<T, ContractError>;

/// All failure modes a contract call can produce.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContractError {
    /// Wraps a numeric / domain error from [`deadeye_core`].
    #[error(transparent)]
    Core(#[from] CoreError),

    /// Calldata serialization failed.
    #[error(transparent)]
    Serde(#[from] CairoSerdeError),

    /// The provider returned an error.
    #[error("provider error: {0}")]
    Provider(String),

    /// The contract call returned an unexpected number of felts.
    #[error("contract returned {actual} felts, expected {expected} for `{call}`")]
    UnexpectedReturnSize {
        /// Name of the call.
        call: &'static str,
        /// Number of felts actually returned.
        actual: usize,
        /// Number of felts the decoder needed.
        expected: usize,
    },

    /// The contract returned a felt that cannot be decoded as the expected
    /// type (e.g. a u64 with the high bits set).
    #[error("invalid response from `{call}`: {message}")]
    InvalidResponse {
        /// Name of the call.
        call: &'static str,
        /// Diagnostic message.
        message: String,
    },
}

// ─── Typed trade-error model ─────────────────────────────────────────────────

/// `Result` alias for write-path AMM / factory calls.
pub type TradeResult<T> = core::result::Result<T, TradeError>;

/// Top-level error returned by every write-path on the AMM and factory
/// writers.
///
/// Pattern-match on [`TradeError::Rejected`] when you want to decide
/// "retry" vs "abandon" based on *why* the chain rejected the trade;
/// match on [`TradeError::Submission`] for anything else (RPC down,
/// nonce conflicts, fee estimation, etc.).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TradeError {
    /// The on-chain verifier rejected the trade with a structured reason.
    /// The original [`ContractError`] is preserved as `source` for
    /// callers that want the raw revert text for logging.
    #[error("trade rejected: {reason:?}")]
    Rejected {
        /// Structured rejection reason parsed out of the Cairo revert.
        reason: TradeRejectionReason,
        /// Original plumbing error, kept for tracing / log forensics.
        #[source]
        source: ContractError,
    },

    /// Non-revert plumbing failure (RPC down, decode error, etc.).
    #[error(transparent)]
    Submission(#[from] ContractError),
}

impl TradeError {
    /// Returns the structured rejection reason when the chain reverted
    /// with a known guard, `None` otherwise.
    #[must_use]
    pub const fn rejection(&self) -> Option<TradeRejectionReason> {
        match self {
            Self::Rejected { reason, .. } => Some(*reason),
            Self::Submission(_) => None,
        }
    }

    /// Build a [`TradeError`] from a raw [`ContractError`], promoting it
    /// to [`TradeError::Rejected`] when the underlying revert string is
    /// one of the well-known Cairo guards. Falls back to
    /// [`TradeError::Submission`] otherwise.
    #[must_use]
    pub fn from_contract(err: ContractError) -> Self {
        if let Some(reason) = parse_revert_reason(&err) {
            return Self::Rejected {
                reason,
                source: err,
            };
        }
        Self::Submission(err)
    }
}

/// Refined sub-reason for a [`TradeRejectionReason::VerificationFailed`].
///
/// On chain, `VERIFICATION_FAILED` is the catch-all guard wrapping four
/// distinct verifier asserts (`SIDE_INVALID`, `STATIONARY_INVALID`,
/// `CURVATURE_INVALID`, `COLLATERAL_INSUFFICIENT` — see
/// `onchain-*-amm/src/internal/guards.cairo`). Bots that need to
/// differentiate (e.g. retry on `CollateralInsufficient` with more
/// padding, abandon on `CurvatureInvalid`) can match on this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VerificationSubReason {
    /// `SIDE_INVALID` — the verifier rejected the chosen side.
    SideInvalid,
    /// `STATIONARY_INVALID` — `d'(x*)` outside tolerance.
    StationaryInvalid,
    /// `CURVATURE_INVALID` — `d''(x*) ≤ 0`.
    CurvatureInvalid,
    /// `COLLATERAL_INSUFFICIENT` — chain-recomputed collateral exceeded
    /// the supplied amount.
    CollateralInsufficient,
    /// `MINIMUM_INVALID` — minimum-finding failed.
    MinimumInvalid,
}

/// Structured reason a write-path was rejected by an on-chain guard.
///
/// Each variant maps to a distinct `assert(false, '<reason>')` in the
/// Cairo source. The exhaustive mapping table lives in the rustdoc on
/// [`parse_revert_reason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TradeRejectionReason {
    /// `INVALID_DISTRIBUTION`.
    InvalidDistribution,
    /// `INVALID_HINTS` — sqrt hints failed validation.
    InvalidHints,
    /// `BACKING_FAIL` — backing check failed.
    BackingFail,
    /// `SIGMA_TOO_LOW` — σ below the per-market floor.
    SigmaTooLow,
    /// `LOW_COLLATERAL` — supplied collateral below the per-market floor.
    LowCollateral,
    /// `VERIFICATION_FAILED` — wraps a verifier sub-reason when one was
    /// recoverable from the revert text.
    VerificationFailed {
        /// More-specific failure when the chain re-asserted a refined
        /// guard ahead of the catch-all (e.g. `SIDE_INVALID`).
        sub_reason: Option<VerificationSubReason>,
    },
    /// `STALE_STATE` — one of the `expected_*` guards mismatched live
    /// state. The `field` describes which expected value was wrong.
    StaleState {
        /// `"distribution"`, `"backing"`, `"tolerance"`, or generic.
        field: &'static str,
    },
    /// `market is settled` / `market settled`.
    MarketSettled,
    /// `market is paused` / `market paused`.
    MarketPaused,
    /// `no position`.
    NoPosition,
    /// `already claimed`.
    AlreadyClaimed,
    /// `REQUIRES_ADDITIONAL_COLLATERAL`.
    RequiresAdditionalCollateral,
    /// `NO_COLLATERAL_OUT`.
    NoCollateralOut,
    /// `CONVERSION_FAILED`.
    ConversionFailed,
    /// `only owner` — admin-gated entrypoint called by non-owner.
    OnlyOwner,
    /// `not authorized` — admin-or-owner-gated entrypoint, neither.
    NotAuthorized,
    /// `MIN_OUT_NOT_MET` — slippage floor breached.
    MinOutNotMet,
    /// `INVALID_MIN_OUTCOME` — multinoulli `min_outcome_index` was wrong.
    InvalidMinOutcome,
    /// `REENTRANT`.
    Reentrant,
    /// `market not initialized`.
    MarketNotInitialized,
    /// `already settled` — settle entrypoint called twice (reachable via
    /// factory `settle_*` wrappers).
    AlreadySettled,
    /// `already paused` — pause called on a paused market.
    AlreadyPaused,
    /// `not paused` — unpause called on a market that wasn't paused.
    NotPaused,
    /// `market not settled` — claim called before settlement.
    MarketNotSettled,
    /// `no claim` — claim path produced an empty result.
    NoClaim,
    /// `trader claims pending` — LP withdraw / lp-claim path blocked
    /// because traders still have unclaimed positions.
    TraderClaimsPending,
    /// `only factory` — protocol-fee collection called by non-factory.
    OnlyFactory,
    /// `invalid matrix mode` — multinoulli matrix-mode validation failed.
    InvalidMatrixMode,
    /// `invalid settlement mode` — multinoulli LP claim path saw an
    /// unexpected settlement-mode discriminant.
    InvalidSettlementMode,
    /// `missing snapshot ref` — multinoulli snapshot bookkeeping
    /// inconsistent.
    MissingSnapshotRef,
    /// Catch-all for revert strings that are recognised but not yet
    /// mapped to a dedicated variant. `raw` is the verbatim Cairo short
    /// string, ready for logging.
    Other {
        /// Raw Cairo short string.
        raw: &'static str,
    },
}

/// Tries to parse a structured rejection reason out of a [`ContractError`].
///
/// ## Strategy
///
/// Starknet returns reverts as nested JSON containing felt-encoded short
/// strings. The provider stringifies them via `Display`, so the error
/// message ends up with hex felts like `0x53544c5f4654...` (`"STALE_STATE"`)
/// embedded in it. We scan the stringified message for `0x[0-9a-fA-F]+`
/// runs, try to decode each as a Cairo short string, and return the first
/// one we recognise.
///
/// ## Cairo source → variant mapping
///
/// | Cairo `assert(false, '…')` | Variant |
/// |----------------------------|---------|
/// | `'INVALID_DISTRIBUTION'`            | [`TradeRejectionReason::InvalidDistribution`] |
/// | `'INVALID_HINTS'`                   | [`TradeRejectionReason::InvalidHints`] |
/// | `'BACKING_FAIL'`                    | [`TradeRejectionReason::BackingFail`] |
/// | `'SIGMA_TOO_LOW'`                   | [`TradeRejectionReason::SigmaTooLow`] |
/// | `'LOW_COLLATERAL'`                  | [`TradeRejectionReason::LowCollateral`] |
/// | `'VERIFICATION_FAILED'`             | [`TradeRejectionReason::VerificationFailed`] (no sub-reason) |
/// | `'SIDE_INVALID'`                    | [`TradeRejectionReason::VerificationFailed`] / `SideInvalid` |
/// | `'STATIONARY_INVALID'`              | [`TradeRejectionReason::VerificationFailed`] / `StationaryInvalid` |
/// | `'CURVATURE_INVALID'`               | [`TradeRejectionReason::VerificationFailed`] / `CurvatureInvalid` |
/// | `'COLLATERAL_INSUFFICIENT'`         | [`TradeRejectionReason::VerificationFailed`] / `CollateralInsufficient` |
/// | `'MINIMUM_INVALID'`                 | [`TradeRejectionReason::VerificationFailed`] / `MinimumInvalid` |
/// | `'STALE_STATE'`                     | [`TradeRejectionReason::StaleState`] (`field = "guard"`) |
/// | `'market settled'`, `'market is settled'` | [`TradeRejectionReason::MarketSettled`] |
/// | `'market paused'`, `'market is paused'`   | [`TradeRejectionReason::MarketPaused`] |
/// | `'no position'`                     | [`TradeRejectionReason::NoPosition`] |
/// | `'already claimed'`                 | [`TradeRejectionReason::AlreadyClaimed`] |
/// | `'REQUIRES_ADDITIONAL_COLLATERAL'`  | [`TradeRejectionReason::RequiresAdditionalCollateral`] |
/// | `'NO_COLLATERAL_OUT'`               | [`TradeRejectionReason::NoCollateralOut`] |
/// | `'CONVERSION_FAILED'`               | [`TradeRejectionReason::ConversionFailed`] |
/// | `'only owner'`                      | [`TradeRejectionReason::OnlyOwner`] |
/// | `'not authorized'`                  | [`TradeRejectionReason::NotAuthorized`] |
/// | `'MIN_OUT_NOT_MET'`                 | [`TradeRejectionReason::MinOutNotMet`] |
/// | `'INVALID_MIN_OUTCOME'`             | [`TradeRejectionReason::InvalidMinOutcome`] |
/// | `'REENTRANT'`                       | [`TradeRejectionReason::Reentrant`] |
/// | `'market not initialized'`, `'not initialized'` | [`TradeRejectionReason::MarketNotInitialized`] |
/// | `'market not settled'`              | [`TradeRejectionReason::MarketNotSettled`] |
/// | `'already settled'`                 | [`TradeRejectionReason::AlreadySettled`] |
/// | `'already paused'`                  | [`TradeRejectionReason::AlreadyPaused`] |
/// | `'not paused'`                      | [`TradeRejectionReason::NotPaused`] |
/// | `'trader claims pending'`           | [`TradeRejectionReason::TraderClaimsPending`] |
/// | `'no claim'`                        | [`TradeRejectionReason::NoClaim`] |
/// | `'only factory'`                    | [`TradeRejectionReason::OnlyFactory`] |
/// | `'invalid matrix mode'`             | [`TradeRejectionReason::InvalidMatrixMode`] |
/// | `'invalid settlement mode'`         | [`TradeRejectionReason::InvalidSettlementMode`] |
/// | `'missing snapshot ref'`            | [`TradeRejectionReason::MissingSnapshotRef`] |
///
/// Anything else falls through to `None` so the caller can keep the raw
/// [`ContractError`].
#[must_use]
pub fn parse_revert_reason(err: &ContractError) -> Option<TradeRejectionReason> {
    let msg = match err {
        ContractError::Provider(s) => s.as_str(),
        ContractError::InvalidResponse { message, .. } => message.as_str(),
        ContractError::Core(_)
        | ContractError::Serde(_)
        | ContractError::UnexpectedReturnSize { .. } => return None,
    };
    classify_revert_text(msg)
}

/// Pure-text classifier — extracted so unit tests can exercise it
/// without round-tripping a [`ContractError`].
#[must_use]
fn classify_revert_text(msg: &str) -> Option<TradeRejectionReason> {
    // 1) Walk every `0x...` run, try to parse each as a felt + decode
    //    as a Cairo short string. The chain stringifies revert short
    //    strings as 0x-prefixed felts, so this is the canonical path.
    let mut idx = 0_usize;
    while let Some(rel) = msg.get(idx..).and_then(|t| t.find("0x")) {
        let start = idx + rel + 2;
        let bytes = msg.as_bytes();
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end = end.saturating_add(1);
        }
        if end > start {
            let hex_run = msg.get(start..end).unwrap_or_default();
            if let Some(reason) = classify_felt_hex(hex_run) {
                return Some(reason);
            }
        }
        idx = end.max(start.saturating_add(1));
        if idx >= msg.len() {
            break;
        }
    }
    // 2) Fallback: some providers stringify the short-string directly
    //    (e.g. Foundry-style devnets in verbose mode), so also do a
    //    case-sensitive substring search.
    classify_short_string(msg)
}

fn classify_felt_hex(hex: &str) -> Option<TradeRejectionReason> {
    // Felt parsing tolerates short hex strings; skip empty or oversize.
    if hex.is_empty() || hex.len() > 62 {
        return None;
    }
    let felt = Felt::from_hex(&format!("0x{hex}")).ok()?;
    if felt == Felt::ZERO {
        return None;
    }
    let s = parse_cairo_short_string(&felt).ok()?;
    if !s.is_ascii() || s.is_empty() {
        return None;
    }
    classify_short_string(&s)
}

fn classify_short_string(haystack: &str) -> Option<TradeRejectionReason> {
    // Order matters: refined verifier guards must beat the catch-all.
    let table: &[(&str, TradeRejectionReason)] = &[
        (
            "INVALID_DISTRIBUTION",
            TradeRejectionReason::InvalidDistribution,
        ),
        ("INVALID_HINTS", TradeRejectionReason::InvalidHints),
        ("BACKING_FAIL", TradeRejectionReason::BackingFail),
        ("SIGMA_TOO_LOW", TradeRejectionReason::SigmaTooLow),
        ("LOW_COLLATERAL", TradeRejectionReason::LowCollateral),
        (
            "SIDE_INVALID",
            TradeRejectionReason::VerificationFailed {
                sub_reason: Some(VerificationSubReason::SideInvalid),
            },
        ),
        (
            "STATIONARY_INVALID",
            TradeRejectionReason::VerificationFailed {
                sub_reason: Some(VerificationSubReason::StationaryInvalid),
            },
        ),
        (
            "CURVATURE_INVALID",
            TradeRejectionReason::VerificationFailed {
                sub_reason: Some(VerificationSubReason::CurvatureInvalid),
            },
        ),
        (
            "COLLATERAL_INSUFFICIENT",
            TradeRejectionReason::VerificationFailed {
                sub_reason: Some(VerificationSubReason::CollateralInsufficient),
            },
        ),
        (
            "MINIMUM_INVALID",
            TradeRejectionReason::VerificationFailed {
                sub_reason: Some(VerificationSubReason::MinimumInvalid),
            },
        ),
        (
            "VERIFICATION_FAILED",
            TradeRejectionReason::VerificationFailed { sub_reason: None },
        ),
        (
            "STALE_STATE",
            TradeRejectionReason::StaleState { field: "guard" },
        ),
        ("market is settled", TradeRejectionReason::MarketSettled),
        ("market settled", TradeRejectionReason::MarketSettled),
        ("market is paused", TradeRejectionReason::MarketPaused),
        ("market paused", TradeRejectionReason::MarketPaused),
        ("no position", TradeRejectionReason::NoPosition),
        ("already claimed", TradeRejectionReason::AlreadyClaimed),
        (
            "REQUIRES_ADDITIONAL_COLLATERAL",
            TradeRejectionReason::RequiresAdditionalCollateral,
        ),
        ("NO_COLLATERAL_OUT", TradeRejectionReason::NoCollateralOut),
        ("CONVERSION_FAILED", TradeRejectionReason::ConversionFailed),
        ("only owner", TradeRejectionReason::OnlyOwner),
        ("not authorized", TradeRejectionReason::NotAuthorized),
        ("MIN_OUT_NOT_MET", TradeRejectionReason::MinOutNotMet),
        (
            "INVALID_MIN_OUTCOME",
            TradeRejectionReason::InvalidMinOutcome,
        ),
        ("REENTRANT", TradeRejectionReason::Reentrant),
        (
            "market not initialized",
            TradeRejectionReason::MarketNotInitialized,
        ),
        ("market not settled", TradeRejectionReason::MarketNotSettled),
        (
            "not initialized",
            TradeRejectionReason::MarketNotInitialized,
        ),
        ("already settled", TradeRejectionReason::AlreadySettled),
        ("already paused", TradeRejectionReason::AlreadyPaused),
        ("not paused", TradeRejectionReason::NotPaused),
        (
            "trader claims pending",
            TradeRejectionReason::TraderClaimsPending,
        ),
        ("no claim", TradeRejectionReason::NoClaim),
        ("only factory", TradeRejectionReason::OnlyFactory),
        (
            "invalid matrix mode",
            TradeRejectionReason::InvalidMatrixMode,
        ),
        (
            "invalid settlement mode",
            TradeRejectionReason::InvalidSettlementMode,
        ),
        (
            "missing snapshot ref",
            TradeRejectionReason::MissingSnapshotRef,
        ),
    ];
    for (needle, variant) in table {
        if haystack.contains(needle) {
            return Some(*variant);
        }
    }
    None
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::expect_used,
    reason = "unit tests panic on assertion failure — that's the contract"
)]
mod tests {
    use super::*;

    #[test]
    fn classify_short_strings_directly() {
        assert!(matches!(
            classify_short_string("STALE_STATE"),
            Some(TradeRejectionReason::StaleState { .. })
        ));
        assert!(matches!(
            classify_short_string("INVALID_HINTS"),
            Some(TradeRejectionReason::InvalidHints)
        ));
        assert!(matches!(
            classify_short_string("market is settled"),
            Some(TradeRejectionReason::MarketSettled)
        ));
        assert!(matches!(
            classify_short_string("SIDE_INVALID"),
            Some(TradeRejectionReason::VerificationFailed {
                sub_reason: Some(VerificationSubReason::SideInvalid)
            })
        ));
        assert!(matches!(
            classify_short_string("VERIFICATION_FAILED"),
            Some(TradeRejectionReason::VerificationFailed { sub_reason: None })
        ));
    }

    #[test]
    fn refined_verifier_guard_beats_catch_all() {
        let msg = "VERIFICATION_FAILED CURVATURE_INVALID";
        // First-matching wins; refined guards are listed first.
        let r = classify_short_string(msg).expect("classified");
        assert!(matches!(
            r,
            TradeRejectionReason::VerificationFailed {
                sub_reason: Some(VerificationSubReason::CurvatureInvalid)
            }
        ));
    }

    #[test]
    fn felt_encoded_revert_round_trips() {
        // 'STALE_STATE' encoded as a Cairo short-string felt → hex.
        let felt = starknet_core::utils::cairo_short_string_to_felt("STALE_STATE")
            .expect("encode short string");
        let hex_msg = format!("execute_v3: data=[{felt:#x}]");
        let r = classify_revert_text(&hex_msg).expect("parsed");
        assert!(matches!(r, TradeRejectionReason::StaleState { .. }));
    }

    #[test]
    fn unknown_message_returns_none() {
        assert!(classify_short_string("absolutely nothing here").is_none());
        // Non-hex 0x prefix is tolerated and ignored.
        assert!(classify_revert_text("rpc error: 0xnotahex").is_none());
    }

    #[test]
    fn trade_error_promotion() {
        let err = ContractError::Provider("execute_v3: STALE_STATE somewhere".into());
        let t = TradeError::from_contract(err);
        assert!(matches!(
            t.rejection(),
            Some(TradeRejectionReason::StaleState { .. })
        ));
    }

    #[test]
    fn trade_error_falls_back_to_submission() {
        let err = ContractError::Provider("nonce manager exhausted".into());
        let t = TradeError::from_contract(err);
        assert!(t.rejection().is_none());
    }

    #[test]
    fn admin_and_claim_revert_strings_are_mapped() {
        // Each cairo `assert(false, '<short string>')` reachable from one
        // of the typed admin / claim wrappers must classify to a typed
        // variant (not None and not a verifier sub-reason mis-mapping).
        let cases: &[(&str, TradeRejectionReason)] = &[
            ("already settled", TradeRejectionReason::AlreadySettled),
            ("already paused", TradeRejectionReason::AlreadyPaused),
            ("not paused", TradeRejectionReason::NotPaused),
            ("market not settled", TradeRejectionReason::MarketNotSettled),
            (
                "trader claims pending",
                TradeRejectionReason::TraderClaimsPending,
            ),
            ("no claim", TradeRejectionReason::NoClaim),
            ("only factory", TradeRejectionReason::OnlyFactory),
            (
                "invalid matrix mode",
                TradeRejectionReason::InvalidMatrixMode,
            ),
            (
                "invalid settlement mode",
                TradeRejectionReason::InvalidSettlementMode,
            ),
            (
                "missing snapshot ref",
                TradeRejectionReason::MissingSnapshotRef,
            ),
        ];
        for (needle, expected) in cases {
            let got = classify_short_string(needle).unwrap_or_else(|| {
                panic!("expected {needle:?} to classify, got None");
            });
            assert_eq!(got, *expected, "needle: {needle:?}");
        }
    }
}
