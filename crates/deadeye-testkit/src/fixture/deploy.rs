//! UDC-based contract instantiation.

use starknet_accounts::{Account, ConnectedAccount};
use starknet_contract::{ContractFactory, UdcSelector};
use starknet_core::types::Felt;
use thiserror::Error;

/// Errors emitted by the deployer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DeployError {
    /// UDC deploy submission failed.
    #[error("deploy failed: {0}")]
    Submit(String),
}

/// Deploy a contract via the legacy UDC (the one starknet-devnet-rs
/// predeploys).
///
/// Uses `unique = false` so the address is deterministic from the salt; salt
/// of `Felt::ZERO` makes the address content-addressed.
pub async fn udc_deploy<A>(
    account: A,
    class_hash: Felt,
    salt: Felt,
    constructor_calldata: Vec<Felt>,
) -> Result<Felt, DeployError>
where
    A: Account + ConnectedAccount + Sync + Send,
{
    let factory = ContractFactory::new_with_udc(class_hash, account, UdcSelector::Legacy);
    let deployment = factory.deploy_v3(constructor_calldata, salt, false);
    let deployed_address = deployment.deployed_address();
    deployment
        .send()
        .await
        .map_err(|e| DeployError::Submit(format!("{e}")))?;
    Ok(deployed_address)
}
