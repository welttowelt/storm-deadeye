//! Restricted-collateral-token helpers (used as a test ERC20).
//!
//! The restricted collateral token contract is the project's preferred
//! collateral type for markets. Its constructor is
//! `(owner, name, symbol, decimals, market_registry)`. We pre-mint to a
//! single owner and then transfer to participants.

use starknet_accounts::{Account, ConnectedAccount};
use starknet_core::{
    types::{Call, Felt, FunctionCall},
    utils::get_selector_from_name,
};
use starknet_providers::Provider;
use thiserror::Error;

/// Errors emitted by ERC20 helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Erc20Error {
    /// Submission failed.
    #[error("erc20 op failed: {0}")]
    Submit(String),
    /// Provider call failed.
    #[error("provider failure: {0}")]
    Provider(String),
}

/// Read the ERC20 `balance_of(addr)`. Returns a u256 packed into two felts.
pub async fn balance_of<P: Provider + Sync>(
    provider: &P,
    token: Felt,
    holder: Felt,
) -> Result<u128, Erc20Error> {
    let result = provider
        .call(
            FunctionCall {
                contract_address: token,
                entry_point_selector: get_selector_from_name("balance_of").expect("selector valid"),
                calldata: vec![holder],
            },
            starknet_core::types::BlockId::Tag(starknet_core::types::BlockTag::PreConfirmed),
        )
        .await
        .map_err(|e| Erc20Error::Provider(format!("{e}")))?;
    if result.len() < 2 {
        return Err(Erc20Error::Provider(format!(
            "balance_of returned {} felts",
            result.len()
        )));
    }
    // u256 = { low: felt, high: felt }. We only care about low for test
    // sizes.
    let low_bytes = result[0].to_bytes_be();
    let (high, low) = low_bytes.split_at(16);
    if high.iter().any(|b| *b != 0) {
        return Err(Erc20Error::Provider("balance overflows u128".into()));
    }
    let mut buf = [0_u8; 16];
    buf.copy_from_slice(low);
    #[expect(
        clippy::big_endian_bytes,
        reason = "Felt::to_bytes_be is big-endian by spec"
    )]
    Ok(u128::from_be_bytes(buf))
}

/// Send a `transfer(recipient, amount)` call. `amount` is u128 widened to u256.
pub async fn transfer<A>(
    account: A,
    token: Felt,
    recipient: Felt,
    amount: u128,
) -> Result<(), Erc20Error>
where
    A: Account + ConnectedAccount + Sync + Send,
{
    let calldata = vec![recipient, Felt::from(amount), Felt::ZERO];
    let call = Call {
        to: token,
        selector: get_selector_from_name("transfer").expect("selector valid"),
        calldata,
    };
    account
        .execute_v3(vec![call])
        .send()
        .await
        .map_err(|e| Erc20Error::Submit(format!("transfer: {e}")))?;
    Ok(())
}

/// Send an `approve(spender, amount)` call.
pub async fn approve<A>(
    account: A,
    token: Felt,
    spender: Felt,
    amount: u128,
) -> Result<(), Erc20Error>
where
    A: Account + ConnectedAccount + Sync + Send,
{
    let calldata = vec![spender, Felt::from(amount), Felt::ZERO];
    let call = Call {
        to: token,
        selector: get_selector_from_name("approve").expect("selector valid"),
        calldata,
    };
    account
        .execute_v3(vec![call])
        .send()
        .await
        .map_err(|e| Erc20Error::Submit(format!("approve: {e}")))?;
    Ok(())
}

/// Call `operator_mint(recipient, amount)` on the restricted-collateral
/// token. Caller must be owner or operator.
pub async fn operator_mint<A>(
    account: A,
    token: Felt,
    recipient: Felt,
    amount: u128,
) -> Result<(), Erc20Error>
where
    A: Account + ConnectedAccount + Sync + Send,
{
    let calldata = vec![recipient, Felt::from(amount), Felt::ZERO];
    let call = Call {
        to: token,
        selector: get_selector_from_name("operator_mint").expect("selector valid"),
        calldata,
    };
    account
        .execute_v3(vec![call])
        .send()
        .await
        .map_err(|e| Erc20Error::Submit(format!("operator_mint: {e}")))?;
    Ok(())
}

/// Call `set_factory(factory)` on the restricted-collateral token. Used
/// during bootstrap so the token recognises our factory.
pub async fn set_token_factory<A>(account: A, token: Felt, factory: Felt) -> Result<(), Erc20Error>
where
    A: Account + ConnectedAccount + Sync + Send,
{
    let call = Call {
        to: token,
        selector: get_selector_from_name("set_factory").expect("selector valid"),
        calldata: vec![factory],
    };
    account
        .execute_v3(vec![call])
        .send()
        .await
        .map_err(|e| Erc20Error::Submit(format!("set_factory: {e}")))?;
    Ok(())
}

/// Call `set_system_transfer_address(account, enabled)`.
///
/// Grants the target an allowlist bypass on the registered-market
/// transfer restriction.
pub async fn set_system_transfer_address<A>(
    account: A,
    token: Felt,
    target: Felt,
    enabled: bool,
) -> Result<(), Erc20Error>
where
    A: Account + ConnectedAccount + Sync + Send,
{
    let mut calldata = Vec::with_capacity(2);
    calldata.push(target);
    calldata.push(if enabled { Felt::ONE } else { Felt::ZERO });
    let call = Call {
        to: token,
        selector: get_selector_from_name("set_system_transfer_address").expect("selector valid"),
        calldata,
    };
    account
        .execute_v3(vec![call])
        .send()
        .await
        .map_err(|e| Erc20Error::Submit(format!("set_system_transfer_address: {e}")))?;
    Ok(())
}
