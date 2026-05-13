//! Top-level entry point: [`DeadeyeClient`].
//!
//! A client owns a [`deadeye_starknet::Provider`] and is the
//! handle through which all market-specific accessors are spawned. It does
//! not own key material — write paths take an explicit signer / account
//! so the same client can be shared between read-only dashboards and
//! signing services.

use deadeye_starknet::Provider;
use starknet_core::types::Felt;

/// SDK client — generic over the [`Provider`] implementation so users can
/// plug in custom RPC transports, racers, or mocks.
#[derive(Debug)]
pub struct DeadeyeClient<P>
where
    P: Provider,
{
    provider: P,
}

impl<P> DeadeyeClient<P>
where
    P: Provider,
{
    /// Construct a client over an arbitrary [`Provider`].
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    /// Borrow the underlying provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// Open a [`NormalMarket`](crate::normal::NormalMarket) handle for the
    /// given AMM contract address.
    #[cfg(feature = "normal-market")]
    pub fn normal_market(&self, address: Felt) -> crate::normal::NormalMarket<'_, P> {
        crate::normal::NormalMarket::new(&self.provider, address)
    }

    /// Open a [`MultinoulliMarket`](crate::multinoulli::MultinoulliMarket)
    /// handle for the given AMM contract address.
    #[cfg(feature = "multinoulli-market")]
    pub fn multinoulli_market(
        &self,
        address: Felt,
    ) -> crate::multinoulli::MultinoulliMarket<'_, P> {
        crate::multinoulli::MultinoulliMarket::new(&self.provider, address)
    }

    /// Open a [`LognormalMarket`](crate::lognormal::LognormalMarket) handle.
    #[cfg(feature = "lognormal-market")]
    pub fn lognormal_market(&self, address: Felt) -> crate::lognormal::LognormalMarket<'_, P> {
        crate::lognormal::LognormalMarket::new(&self.provider, address)
    }

    /// Open a [`BivariateMarket`](crate::bivariate::BivariateMarket) handle.
    #[cfg(feature = "bivariate-market")]
    pub fn bivariate_market(&self, address: Felt) -> crate::bivariate::BivariateMarket<'_, P> {
        crate::bivariate::BivariateMarket::new(&self.provider, address)
    }

    /// Open a [`Factory`](crate::factory::Factory) handle.
    pub fn factory(&self, address: Felt) -> crate::factory::Factory<'_, P> {
        crate::factory::Factory::new(&self.provider, address)
    }

    /// Open an [`Oracle`](crate::oracle::Oracle) handle.
    pub fn oracle(&self, address: Felt) -> crate::oracle::Oracle<'_, P> {
        crate::oracle::Oracle::new(&self.provider, address)
    }
}
