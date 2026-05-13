//! Distribution factory facade.

use deadeye_starknet::{Account, FactoryReader, FactoryWriter, Felt, Provider};

use crate::error::SdkResult;

/// Read-only handle to the factory.
#[derive(Debug)]
pub struct Factory<'p, P>
where
    P: Provider,
{
    reader: FactoryReader<&'p P>,
}

impl<'p, P> Factory<'p, P>
where
    P: Provider,
{
    /// Construct a handle.
    pub fn new(provider: &'p P, address: Felt) -> Self {
        Self {
            reader: FactoryReader::new(provider, address),
        }
    }

    /// Borrow the underlying reader.
    pub const fn reader(&self) -> &FactoryReader<&'p P> {
        &self.reader
    }

    /// Factory contract address.
    pub const fn address(&self) -> Felt {
        self.reader.address()
    }

    /// Reads the total number of deployed markets.
    pub async fn market_count(&self) -> SdkResult<u64> {
        Ok(self.reader.market_count().await?)
    }

    /// Reads the address of the market at `index`.
    pub async fn market_at(&self, index: u64) -> SdkResult<Felt> {
        Ok(self.reader.market_at(index).await?)
    }

    /// Bind an account for writes.
    pub fn with_account<A>(self, account: A) -> FactorySigned<'p, P, A>
    where
        A: Account,
    {
        FactorySigned {
            writer: FactoryWriter::new(self.reader, account),
        }
    }
}

/// Account-bound companion.
#[derive(Debug)]
pub struct FactorySigned<'p, P, A>
where
    P: Provider,
    A: Account,
{
    writer: FactoryWriter<&'p P, A>,
}

impl<P, A> FactorySigned<'_, P, A>
where
    P: Provider,
    A: Account,
{
    /// Borrow the underlying writer.
    pub const fn writer(&self) -> &FactoryWriter<&P, A> {
        &self.writer
    }
}
