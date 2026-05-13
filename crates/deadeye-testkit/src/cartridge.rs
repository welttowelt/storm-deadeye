//! Cartridge RPC discovery helpers.
//!
//! Cartridge hosts public Starknet RPC endpoints suitable for stable
//! integration testing against Sepolia or mainnet. We expose them as a
//! typed enum so tests can opt into a network without hardcoding URLs.

use url::Url;

/// Known Cartridge RPC networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CartridgeNetwork {
    /// Public Sepolia testnet.
    Sepolia,
    /// Mainnet — use with extreme caution.
    Mainnet,
}

impl CartridgeNetwork {
    /// Returns the JSON-RPC URL for this network.
    #[must_use]
    pub fn url(self) -> Url {
        let raw = match self {
            Self::Sepolia => "https://api.cartridge.gg/x/starknet/sepolia",
            Self::Mainnet => "https://api.cartridge.gg/x/starknet/mainnet",
        };
        Url::parse(raw).expect("static Cartridge URLs parse")
    }

    /// Picks a network from the environment variable `DEADEYE_TEST_NETWORK`.
    ///
    /// Defaults to [`CartridgeNetwork::Sepolia`] when the variable is unset
    /// or unrecognised.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("DEADEYE_TEST_NETWORK").as_deref() {
            Ok("mainnet") => Self::Mainnet,
            _ => Self::Sepolia,
        }
    }
}
