//! Provider abstraction.
//!
//! The [`Provider`] trait abstracts the operation we actually need —
//! invoking a *view* function on a Starknet contract — so that the SDK can
//! plug in `starknet-providers::JsonRpcClient`, a mocked deterministic
//! provider for testing, or a redundant multi-RPC racer for production
//! latency optimisation without code changes in the contract wrappers.

use async_trait::async_trait;
use starknet_core::types::{BlockId, BlockTag, Felt, FunctionCall};

use crate::error::{ContractError, ContractResult};

/// Read-only view of a Starknet provider.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Invoke a view function and return its raw felt response.
    async fn call(&self, call: FunctionCall, block: BlockId) -> ContractResult<Vec<Felt>>;

    /// Default block selector used by view functions. Production deployments
    /// usually want `pre_confirmed` so the latest state is visible to the
    /// MM loop; integration tests prefer `latest` for determinism.
    fn default_block(&self) -> BlockId {
        BlockId::Tag(BlockTag::PreConfirmed)
    }
}

#[async_trait]
impl<P> Provider for &P
where
    P: Provider + ?Sized,
{
    async fn call(&self, call: FunctionCall, block: BlockId) -> ContractResult<Vec<Felt>> {
        (*self).call(call, block).await
    }

    fn default_block(&self) -> BlockId {
        (*self).default_block()
    }
}

/// Adapter implementing [`Provider`] over `starknet-providers::JsonRpcClient`.
///
/// Only compiled when the `provider` feature is enabled. Bring-your-own
/// provider implementations (e.g. multi-RPC racers in a market-making
/// gateway) can implement the trait directly without touching this module.
#[cfg(feature = "provider")]
pub mod jsonrpc {
    use async_trait::async_trait;
    use starknet_core::types::{BlockId, Felt, FunctionCall};
    use starknet_providers::{JsonRpcClient, Provider as StarknetProvider, jsonrpc::HttpTransport};

    use super::{ContractError, ContractResult, Provider};

    /// JSON-RPC backed [`Provider`].
    #[derive(Debug)]
    pub struct JsonRpcProvider {
        inner: JsonRpcClient<HttpTransport>,
        default_block: BlockId,
    }

    impl JsonRpcProvider {
        /// Construct a provider with default block selector `PreConfirmed`.
        #[must_use]
        pub const fn new(inner: JsonRpcClient<HttpTransport>) -> Self {
            Self {
                inner,
                default_block: BlockId::Tag(starknet_core::types::BlockTag::PreConfirmed),
            }
        }

        /// Override the default block selector used by view functions.
        #[must_use]
        pub const fn with_default_block(mut self, block: BlockId) -> Self {
            self.default_block = block;
            self
        }

        /// Access the underlying JSON-RPC client (e.g. for non-view calls).
        #[must_use]
        pub const fn inner(&self) -> &JsonRpcClient<HttpTransport> {
            &self.inner
        }
    }

    #[async_trait]
    impl Provider for JsonRpcProvider {
        async fn call(&self, call: FunctionCall, block: BlockId) -> ContractResult<Vec<Felt>> {
            self.inner
                .call(call, block)
                .await
                .map_err(|e| ContractError::Provider(format!("{e}")))
        }

        fn default_block(&self) -> BlockId {
            self.default_block
        }
    }
}

#[cfg(feature = "provider")]
pub use jsonrpc::JsonRpcProvider;

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on canned response setup")]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Trivial mock provider used to exercise the trait without a live RPC.
    #[derive(Default)]
    struct MockProvider {
        responses: Mutex<Vec<Vec<Felt>>>,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            self.responses
                .lock()
                .expect("mutex poisoned")
                .pop()
                .ok_or_else(|| ContractError::Provider("no canned response".into()))
        }
    }

    #[tokio::test]
    async fn mock_provider_returns_canned_response() {
        let provider = MockProvider {
            responses: Mutex::new(vec![vec![Felt::from(7_u64)]]),
        };
        let call = FunctionCall {
            contract_address: Felt::ZERO,
            entry_point_selector: Felt::ZERO,
            calldata: vec![],
        };
        let response = provider.call(call, provider.default_block()).await.unwrap();
        assert_eq!(response, vec![Felt::from(7_u64)]);
    }
}
