//! Cairo Serde shapes for the distribution factory.
//!
//! Mirrors `@the-situation/abi`'s `factory.ts`.

use deadeye_core::{
    bivariate::{BivariateNormalDistributionRaw, BivariateNormalSqrtHintsRaw},
    categorical::{CategoricalDistributionRaw, CategoricalL2HintRaw},
    distribution::{LognormalDistributionRaw, NormalDistributionRaw, NormalSqrtHintsRaw},
    sq128::Sq128Raw,
};
use starknet_core::types::Felt;

use crate::{
    cairo_serde::{CairoSerde, CairoSerdeError},
    cairo_serde_unit_enum,
    types::{common::FeeConfigRaw, lognormal::LognormalSqrtHintsRaw},
};

/// Per-market-type configuration registered with the factory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MarketTypeConfigRaw {
    /// AMM contract class hash.
    pub amm_class_hash: Felt,
    /// Math-runtime contract class hash.
    pub runtime_class_hash: Felt,
    /// Plugin contract address used during deployment.
    pub plugin: Felt,
    /// Whether deployment of this market type is enabled.
    pub enabled: bool,
}

impl CairoSerde for MarketTypeConfigRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.amm_class_hash.encode(out);
        self.runtime_class_hash.encode(out);
        self.plugin.encode(out);
        self.enabled.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (amm_class_hash, slice) = Felt::decode(slice)?;
        let (runtime_class_hash, slice) = Felt::decode(slice)?;
        let (plugin, slice) = Felt::decode(slice)?;
        let (enabled, slice) = bool::decode(slice)?;
        Ok((
            Self {
                amm_class_hash,
                runtime_class_hash,
                plugin,
                enabled,
            },
            slice,
        ))
    }
}

/// Deploy profile (preset configuration usable by `deploy_*_from_profile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MarketDeployProfileRaw {
    /// Market family discriminant (`u8`; mirrors `MarketKind` on-chain).
    pub market_type: u8,
    /// Collateral token contract.
    pub collateral_token: Felt,
    /// Token decimals.
    pub token_decimals: u8,
    /// Internal precision.
    pub internal_decimals: u8,
    /// AMM `k`.
    pub k: Sq128Raw,
    /// Initial backing.
    pub backing: Sq128Raw,
    /// Tolerance.
    pub tolerance: Sq128Raw,
    /// Floor on trade collateral.
    pub min_trade_collateral: Sq128Raw,
    /// Fee configuration.
    pub fee_config: FeeConfigRaw,
    /// Extension contract address.
    pub extension: Felt,
    /// Extension call-points bitfield.
    pub extension_call_points: u16,
}

impl CairoSerde for MarketDeployProfileRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.market_type.encode(out);
        self.collateral_token.encode(out);
        self.token_decimals.encode(out);
        self.internal_decimals.encode(out);
        self.k.encode(out);
        self.backing.encode(out);
        self.tolerance.encode(out);
        self.min_trade_collateral.encode(out);
        self.fee_config.encode(out);
        self.extension.encode(out);
        self.extension_call_points.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (market_type, slice) = u8::decode(slice)?;
        let (collateral_token, slice) = Felt::decode(slice)?;
        let (token_decimals, slice) = u8::decode(slice)?;
        let (internal_decimals, slice) = u8::decode(slice)?;
        let (k, slice) = Sq128Raw::decode(slice)?;
        let (backing, slice) = Sq128Raw::decode(slice)?;
        let (tolerance, slice) = Sq128Raw::decode(slice)?;
        let (min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
        let (fee_config, slice) = FeeConfigRaw::decode(slice)?;
        let (extension, slice) = Felt::decode(slice)?;
        let (extension_call_points, slice) = u16::decode(slice)?;
        Ok((
            Self {
                market_type,
                collateral_token,
                token_decimals,
                internal_decimals,
                k,
                backing,
                tolerance,
                min_trade_collateral,
                fee_config,
                extension,
                extension_call_points,
            },
            slice,
        ))
    }
}

impl CairoSerde for u16 {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(Felt::from(*self));
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (value, rest) = u64::decode(slice)?;
        let v16 = Self::try_from(value).map_err(|_| CairoSerdeError::U64OutOfRange {
            value: format!("{value}"),
        })?;
        Ok((v16, rest))
    }
}

/// Selective override carrying a u16 bitmask + replacement values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MarketDeployOverridesRaw {
    /// Bit `i` set means the corresponding field overrides the profile.
    pub mask: u16,
    /// Override collateral token.
    pub collateral_token: Felt,
    /// Override token decimals.
    pub token_decimals: u8,
    /// Override internal decimals.
    pub internal_decimals: u8,
    /// Override `k`.
    pub k: Sq128Raw,
    /// Override backing.
    pub backing: Sq128Raw,
    /// Override tolerance.
    pub tolerance: Sq128Raw,
    /// Override min trade collateral.
    pub min_trade_collateral: Sq128Raw,
    /// Override fee config.
    pub fee_config: FeeConfigRaw,
    /// Override extension contract.
    pub extension: Felt,
    /// Override extension call points.
    pub extension_call_points: u16,
}

impl CairoSerde for MarketDeployOverridesRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.mask.encode(out);
        self.collateral_token.encode(out);
        self.token_decimals.encode(out);
        self.internal_decimals.encode(out);
        self.k.encode(out);
        self.backing.encode(out);
        self.tolerance.encode(out);
        self.min_trade_collateral.encode(out);
        self.fee_config.encode(out);
        self.extension.encode(out);
        self.extension_call_points.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (mask, slice) = u16::decode(slice)?;
        let (collateral_token, slice) = Felt::decode(slice)?;
        let (token_decimals, slice) = u8::decode(slice)?;
        let (internal_decimals, slice) = u8::decode(slice)?;
        let (k, slice) = Sq128Raw::decode(slice)?;
        let (backing, slice) = Sq128Raw::decode(slice)?;
        let (tolerance, slice) = Sq128Raw::decode(slice)?;
        let (min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
        let (fee_config, slice) = FeeConfigRaw::decode(slice)?;
        let (extension, slice) = Felt::decode(slice)?;
        let (extension_call_points, slice) = u16::decode(slice)?;
        Ok((
            Self {
                mask,
                collateral_token,
                token_decimals,
                internal_decimals,
                k,
                backing,
                tolerance,
                min_trade_collateral,
                fee_config,
                extension,
                extension_call_points,
            },
            slice,
        ))
    }
}

/// Status enum for batched factory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FactoryOpStatus {
    /// Operation succeeded.
    Success,
    /// Provided address is not a known market.
    UnknownMarket,
    /// Market type didn't match the expected family.
    MarketTypeMismatch,
    /// Underlying call failed.
    CallFailed,
    /// Decoding of the return value failed.
    DecodeFailed,
}

cairo_serde_unit_enum!(FactoryOpStatus {
    Success = 0,
    UnknownMarket = 1,
    MarketTypeMismatch = 2,
    CallFailed = 3,
    DecodeFailed = 4,
});

/// Per-market entry in a factory batch op response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FactoryBatchOpResultRaw {
    /// Market address.
    pub market: Felt,
    /// Status of the operation against that market.
    pub status: FactoryOpStatus,
}

impl CairoSerde for FactoryBatchOpResultRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.market.encode(out);
        self.status.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (market, slice) = Felt::decode(slice)?;
        let (status, slice) = FactoryOpStatus::decode(slice)?;
        Ok((Self { market, status }, slice))
    }
}

// ─── Per-family deploy inputs ────────────────────────────────────────────────

/// Input to `deploy_normal_market_from_profile`.
#[derive(Debug, Clone, Copy)]
pub struct DeployNormalMarketFromProfileInput {
    /// Profile to deploy from.
    pub profile_id: u32,
    /// Salt for the deterministic address derivation.
    pub salt: Felt,
    /// Metadata hash (felt252).
    pub metadata_hash: Felt,
    /// Initial distribution.
    pub initial_distribution: NormalDistributionRaw,
    /// Sqrt hints for the initial distribution.
    pub initial_hints: NormalSqrtHintsRaw,
    /// Selective overrides.
    pub overrides: MarketDeployOverridesRaw,
}

impl CairoSerde for DeployNormalMarketFromProfileInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.profile_id.encode(out);
        self.salt.encode(out);
        self.metadata_hash.encode(out);
        self.initial_distribution.encode(out);
        self.initial_hints.encode(out);
        self.overrides.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (profile_id, slice) = u32::decode(slice)?;
        let (salt, slice) = Felt::decode(slice)?;
        let (metadata_hash, slice) = Felt::decode(slice)?;
        let (initial_distribution, slice) = NormalDistributionRaw::decode(slice)?;
        let (initial_hints, slice) = NormalSqrtHintsRaw::decode(slice)?;
        let (overrides, slice) = MarketDeployOverridesRaw::decode(slice)?;
        Ok((
            Self {
                profile_id,
                salt,
                metadata_hash,
                initial_distribution,
                initial_hints,
                overrides,
            },
            slice,
        ))
    }
}

/// Input to `deploy_lognormal_market_from_profile`.
#[derive(Debug, Clone, Copy)]
pub struct DeployLognormalMarketFromProfileInput {
    /// Profile id.
    pub profile_id: u32,
    /// Salt.
    pub salt: Felt,
    /// Metadata hash.
    pub metadata_hash: Felt,
    /// Initial distribution.
    pub initial_distribution: LognormalDistributionRaw,
    /// Sqrt hints.
    pub initial_hints: LognormalSqrtHintsRaw,
    /// Selective overrides.
    pub overrides: MarketDeployOverridesRaw,
}

impl CairoSerde for DeployLognormalMarketFromProfileInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.profile_id.encode(out);
        self.salt.encode(out);
        self.metadata_hash.encode(out);
        self.initial_distribution.encode(out);
        self.initial_hints.encode(out);
        self.overrides.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (profile_id, slice) = u32::decode(slice)?;
        let (salt, slice) = Felt::decode(slice)?;
        let (metadata_hash, slice) = Felt::decode(slice)?;
        let (initial_distribution, slice) = LognormalDistributionRaw::decode(slice)?;
        let (initial_hints, slice) = LognormalSqrtHintsRaw::decode(slice)?;
        let (overrides, slice) = MarketDeployOverridesRaw::decode(slice)?;
        Ok((
            Self {
                profile_id,
                salt,
                metadata_hash,
                initial_distribution,
                initial_hints,
                overrides,
            },
            slice,
        ))
    }
}

/// Input to `deploy_bivariate_normal_market_from_profile`.
#[derive(Debug, Clone, Copy)]
pub struct DeployBivariateNormalMarketFromProfileInput {
    /// Profile id.
    pub profile_id: u32,
    /// Salt.
    pub salt: Felt,
    /// Metadata hash.
    pub metadata_hash: Felt,
    /// Initial distribution.
    pub initial_distribution: BivariateNormalDistributionRaw,
    /// Sqrt hints.
    pub initial_hints: BivariateNormalSqrtHintsRaw,
    /// Selective overrides.
    pub overrides: MarketDeployOverridesRaw,
}

impl CairoSerde for DeployBivariateNormalMarketFromProfileInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.profile_id.encode(out);
        self.salt.encode(out);
        self.metadata_hash.encode(out);
        self.initial_distribution.encode(out);
        self.initial_hints.encode(out);
        self.overrides.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (profile_id, slice) = u32::decode(slice)?;
        let (salt, slice) = Felt::decode(slice)?;
        let (metadata_hash, slice) = Felt::decode(slice)?;
        let (initial_distribution, slice) = BivariateNormalDistributionRaw::decode(slice)?;
        let (initial_hints, slice) = BivariateNormalSqrtHintsRaw::decode(slice)?;
        let (overrides, slice) = MarketDeployOverridesRaw::decode(slice)?;
        Ok((
            Self {
                profile_id,
                salt,
                metadata_hash,
                initial_distribution,
                initial_hints,
                overrides,
            },
            slice,
        ))
    }
}

/// Input to `deploy_multinoulli_market_from_profile`.
#[derive(Debug, Clone)]
pub struct DeployMultinoulliMarketFromProfileInput {
    /// Profile id.
    pub profile_id: u32,
    /// Salt.
    pub salt: Felt,
    /// Metadata hash.
    pub metadata_hash: Felt,
    /// Initial distribution.
    pub initial_distribution: CategoricalDistributionRaw,
    /// L2 norm hint for the initial distribution.
    pub initial_hint: CategoricalL2HintRaw,
    /// Selective overrides.
    pub overrides: MarketDeployOverridesRaw,
}

impl CairoSerde for DeployMultinoulliMarketFromProfileInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.profile_id.encode(out);
        self.salt.encode(out);
        self.metadata_hash.encode(out);
        self.initial_distribution.encode(out);
        self.initial_hint.encode(out);
        self.overrides.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (profile_id, slice) = u32::decode(slice)?;
        let (salt, slice) = Felt::decode(slice)?;
        let (metadata_hash, slice) = Felt::decode(slice)?;
        let (initial_distribution, slice) = CategoricalDistributionRaw::decode(slice)?;
        let (initial_hint, slice) = CategoricalL2HintRaw::decode(slice)?;
        let (overrides, slice) = MarketDeployOverridesRaw::decode(slice)?;
        Ok((
            Self {
                profile_id,
                salt,
                metadata_hash,
                initial_distribution,
                initial_hint,
                overrides,
            },
            slice,
        ))
    }
}
