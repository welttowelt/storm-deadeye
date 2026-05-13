//! Cairo Serde shapes for the Deadeye oracle extension.
//!
//! Mirrors `@the-situation/abi`'s `oracle.ts`.

use deadeye_core::{distribution::NormalDistributionRaw, sq128::Sq128Raw};
use starknet_core::types::Felt;

use crate::cairo_serde::{CairoSerde, CairoSerdeError};

/// Cumulative-256 accumulator (same wire shape as `Sq128Raw`, semantically a
/// 256-bit signed running sum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cumulative256Raw {
    /// Bits 0..64.
    pub limb0: u64,
    /// Bits 64..128.
    pub limb1: u64,
    /// Bits 128..192.
    pub limb2: u64,
    /// Bits 192..256.
    pub limb3: u64,
    /// Sign flag.
    pub neg: bool,
}

impl CairoSerde for Cumulative256Raw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.limb0.encode(out);
        self.limb1.encode(out);
        self.limb2.encode(out);
        self.limb3.encode(out);
        self.neg.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (limb0, slice) = u64::decode(slice)?;
        let (limb1, slice) = u64::decode(slice)?;
        let (limb2, slice) = u64::decode(slice)?;
        let (limb3, slice) = u64::decode(slice)?;
        let (neg, slice) = bool::decode(slice)?;
        Ok((
            Self {
                limb0,
                limb1,
                limb2,
                limb3,
                neg,
            },
            slice,
        ))
    }
}

/// Per-snapshot record stored by the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotRaw {
    /// Unix timestamp (seconds) of the snapshot.
    pub block_timestamp: u64,
    /// Cumulative mean to-date.
    pub mean_cumulative: Cumulative256Raw,
    /// Cumulative variance to-date.
    pub variance_cumulative: Cumulative256Raw,
}

impl CairoSerde for SnapshotRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.block_timestamp.encode(out);
        self.mean_cumulative.encode(out);
        self.variance_cumulative.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (block_timestamp, slice) = u64::decode(slice)?;
        let (mean_cumulative, slice) = Cumulative256Raw::decode(slice)?;
        let (variance_cumulative, slice) = Cumulative256Raw::decode(slice)?;
        Ok((
            Self {
                block_timestamp,
                mean_cumulative,
                variance_cumulative,
            },
            slice,
        ))
    }
}

/// Identifier passed to oracle queries — uniquely identifies the AMM the
/// snapshot stream belongs to.
#[derive(Debug, Clone, Copy)]
pub struct MarketKey {
    /// Collateral token contract.
    pub collateral_token: Felt,
    /// Initial distribution at deployment.
    pub initial_distribution: NormalDistributionRaw,
    /// AMM `k`.
    pub k: Sq128Raw,
    /// Initial backing.
    pub initial_backing: Sq128Raw,
    /// Extension contract (typically the oracle itself).
    pub extension: Felt,
    /// Metadata hash.
    pub metadata_hash: Felt,
}

impl CairoSerde for MarketKey {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.collateral_token.encode(out);
        self.initial_distribution.encode(out);
        self.k.encode(out);
        self.initial_backing.encode(out);
        self.extension.encode(out);
        self.metadata_hash.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (collateral_token, slice) = Felt::decode(slice)?;
        let (initial_distribution, slice) = NormalDistributionRaw::decode(slice)?;
        let (k, slice) = Sq128Raw::decode(slice)?;
        let (initial_backing, slice) = Sq128Raw::decode(slice)?;
        let (extension, slice) = Felt::decode(slice)?;
        let (metadata_hash, slice) = Felt::decode(slice)?;
        Ok((
            Self {
                collateral_token,
                initial_distribution,
                k,
                initial_backing,
                extension,
                metadata_hash,
            },
            slice,
        ))
    }
}
