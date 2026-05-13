//! Oracle facade.

use deadeye_starknet::{Felt, OracleClient, Provider};

/// Handle to the oracle extension contract.
#[derive(Debug)]
pub struct Oracle<'p, P>
where
    P: Provider,
{
    client: OracleClient<&'p P>,
}

impl<'p, P> Oracle<'p, P>
where
    P: Provider,
{
    /// Bind to an oracle address.
    pub fn new(provider: &'p P, address: Felt) -> Self {
        Self {
            client: OracleClient::new(provider, address),
        }
    }

    /// Borrow the underlying client.
    pub const fn client(&self) -> &OracleClient<&'p P> {
        &self.client
    }

    /// Contract address.
    pub const fn address(&self) -> Felt {
        self.client.address()
    }
}
