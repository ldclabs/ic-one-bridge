use alloy_primitives::Address;
use candid::Principal;
use http::Uri;
use std::collections::BTreeSet;

use crate::{
    helper::{pretty_format, validate_principals},
    store,
    svm::{Pubkey, get_mint_decimals},
};

#[ic_cdk::update(guard = "is_controller")]
fn admin_add_bridges(args: BTreeSet<Principal>) -> Result<(), String> {
    validate_principals(&args)?;
    let mut args = args;
    store::state::with_mut(|s| {
        s.sub_bridges.append(&mut args);
        Ok(())
    })
}

#[ic_cdk::update]
fn validate_admin_add_bridges(args: BTreeSet<Principal>) -> Result<String, String> {
    validate_principals(&args)?;
    pretty_format(&(args,))
}

#[ic_cdk::update(guard = "is_controller")]
fn admin_remove_bridges(args: BTreeSet<Principal>) -> Result<(), String> {
    validate_principals(&args)?;
    store::state::with_mut(|s| {
        s.sub_bridges.retain(|p| !args.contains(p));
        Ok(())
    })
}

#[ic_cdk::update]
fn validate_admin_remove_bridges(args: BTreeSet<Principal>) -> Result<String, String> {
    validate_principals(&args)?;
    pretty_format(&(args,))
}

#[ic_cdk::update(guard = "is_controller")]
async fn admin_add_evm_contract(
    chain_name: String,
    chain_id: u64,
    address: String,
) -> Result<(), String> {
    let address = check_admin_add_evm_contract(&chain_name, chain_id, &address)?;
    let cli = store::state::evm_client(&chain_name);
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let (cid, gas_price, max_priority_fee_per_gas, decimals) = futures::future::try_join4(
        cli.chain_id(now_ms),
        cli.gas_price(now_ms),
        cli.max_priority_fee_per_gas(now_ms),
        cli.erc20_decimals(now_ms, &address),
    )
    .await?;

    if chain_id != cid {
        return Err(format!(
            "chain_id mismatch, got {}, expected {}",
            cid, chain_id
        ));
    }

    store::state::with_mut(|s| {
        s.evm_token_contracts
            .insert(chain_name.clone(), (address, decimals, chain_id));
        s.evm_latest_gas
            .insert(chain_name, (now_ms, gas_price, max_priority_fee_per_gas));
        Ok(())
    })
}

#[ic_cdk::update]
fn validate_admin_add_evm_contract(
    chain_name: String,
    chain_id: u64,
    address: String,
) -> Result<String, String> {
    check_admin_add_evm_contract(&chain_name, chain_id, &address)?;
    pretty_format(&(chain_name, chain_id, address))
}

fn check_admin_add_evm_contract(
    chain_name: &str,
    chain_id: u64,
    address: &str,
) -> Result<Address, String> {
    if chain_name.trim().to_ascii_uppercase() != chain_name
        || chain_name.is_empty()
        || chain_name.len() > 8
    {
        return Err("chain_name must be non-empty, up to 8 chars, and all uppercase".to_string());
    }

    let addr = Address::parse_checksummed(address, None)
        .map_err(|err| format!("invalid address {address}: {err:?}"))?;

    store::state::with(|s| {
        if s.evm_token_contracts.contains_key(chain_name) {
            return Err("chain_id already exists".to_string());
        }

        if s.evm_token_contracts
            .values()
            .any(|(_, _, cid)| *cid == chain_id)
        {
            return Err("chain_id already exists".to_string());
        }
        Ok(())
    })?;
    Ok(addr)
}

#[ic_cdk::update(guard = "is_controller")]
async fn admin_add_svm_contract(address: String) -> Result<(), String> {
    let addr = check_admin_add_svm_contract(&address)?;
    let cli = store::state::svm_client();
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let account = cli.get_account_info(now_ms, &address).await?;
    let account = account.ok_or_else(|| format!("account {address} does not exist"))?;
    let token_program = Pubkey::try_from(account.owner.as_str())
        .map_err(|err| format!("invalid token program address {}: {:?}", account.owner, err))?;
    let decimals = get_mint_decimals(&account)
        .map_err(|err| format!("account {address} is not a token mint account: {err}"))?;

    store::state::with_mut(|s| {
        s.svm_token_address = (addr, decimals, token_program);
    });
    Ok(())
}

#[ic_cdk::update]
fn validate_admin_add_svm_contract(address: String) -> Result<String, String> {
    check_admin_add_svm_contract(&address)?;
    pretty_format(&(address,))
}

fn check_admin_add_svm_contract(address: &str) -> Result<Pubkey, String> {
    let addr =
        Pubkey::try_from(address).map_err(|err| format!("invalid address {address}: {err:?}"))?;

    store::state::with(|s| {
        if s.svm_token_address.0 != Pubkey::default() {
            return Err("address already exists".to_string());
        }
        Ok(())
    })?;
    Ok(addr)
}

#[ic_cdk::update(guard = "is_controller")]
fn admin_set_evm_providers(
    chain_name: String,
    max_confirmations: u64,
    providers: Vec<String>,
) -> Result<(), String> {
    check_evm_providers(max_confirmations, &providers)?;

    store::state::with_mut(|s| {
        s.evm_providers
            .insert(chain_name, (max_confirmations, providers));
        Ok(())
    })
}

#[ic_cdk::update]
fn validate_admin_set_evm_providers(
    chain_name: String,
    max_confirmations: u64,
    providers: Vec<String>,
) -> Result<String, String> {
    check_evm_providers(max_confirmations, &providers)?;
    pretty_format(&(chain_name, max_confirmations, providers))
}

fn check_evm_providers(max_confirmations: u64, providers: &[String]) -> Result<(), String> {
    check_providers(providers)?;
    if max_confirmations < 2 {
        return Err("max_confirmations must be at least 2".to_string());
    }
    Ok(())
}

#[ic_cdk::update(guard = "is_controller")]
fn admin_set_svm_providers(providers: Vec<String>) -> Result<(), String> {
    check_providers(&providers)?;

    store::state::with_mut(|s| {
        s.svm_providers = providers;
        Ok(())
    })
}

#[ic_cdk::update]
fn validate_admin_set_svm_providers(providers: Vec<String>) -> Result<String, String> {
    check_providers(&providers)?;
    pretty_format(&(providers,))
}

fn check_providers(providers: &[String]) -> Result<(), String> {
    for url in providers {
        let uri = url
            .parse::<Uri>()
            .map_err(|err| format!("invalid url {url}, error: {err}"))?;
        if uri.scheme_str() != Some("https") {
            return Err(format!("url scheme must be https, got: {url}"));
        }
        if uri.authority().is_none() {
            return Err(format!("url must include a host, got: {url}"));
        }
    }
    Ok(())
}

#[ic_cdk::update(guard = "is_controller")]
async fn admin_collect_fees(to: Principal, icp_amount: u128) -> Result<store::BridgeTx, String> {
    let ledger = store::state::with_mut(|s| {
        if icp_amount == 0 {
            return Err("amount must be greater than 0".to_string());
        }
        let available = available_fees(s)?;
        if icp_amount > available {
            return Err(format!(
                "amount {} exceeds available fees {}",
                icp_amount, available
            ));
        }
        s.total_withdrawn_fees = s
            .total_withdrawn_fees
            .checked_add(icp_amount)
            .ok_or_else(|| "total_withdrawn_fees overflow".to_string())?;

        Ok(s.token_ledger)
    })?;

    match store::state::to_icp(ledger, to, icp_amount).await {
        Ok(tx) => Ok(tx),
        Err(err) => {
            store::state::with_mut(|s| {
                s.total_withdrawn_fees = s.total_withdrawn_fees.saturating_sub(icp_amount);
            });
            Err(err)
        }
    }
}

#[ic_cdk::update]
fn validate_admin_collect_fees(to: Principal, icp_amount: u128) -> Result<String, String> {
    store::state::with(|s| {
        if icp_amount == 0 {
            return Err("icp_amount must be greater than 0".to_string());
        }
        let available = available_fees(s)?;
        if icp_amount > available {
            return Err(format!(
                "icp_amount {} exceeds available fees {}",
                icp_amount, available
            ));
        }
        Ok(())
    })?;
    pretty_format(&(to, icp_amount))
}

/// Resets the error circuit breaker and re-arms the finalization timer chain.
///
/// Finalization stops scheduling itself once `error_rounds` reaches its limit,
/// which disables bridging until someone intervenes. Use this once the cause of
/// the failures has been dealt with.
#[ic_cdk::update(guard = "is_controller")]
fn admin_restart_bridging() -> Result<u64, String> {
    Ok(store::state::restart_finalize_bridging())
}

#[ic_cdk::update]
fn validate_admin_restart_bridging() -> Result<String, String> {
    let (round, error_rounds, pending) = store::state::with(|s| {
        (
            s.finalize_bridging_round,
            s.error_rounds,
            s.pending.len() as u64,
        )
    });
    pretty_format(&(round, error_rounds, pending))
}

/// Drops the outgoing transaction of a stuck bridging task so the next
/// finalization round builds and broadcasts a fresh one.
///
/// Only use this after verifying on chain that the recorded outgoing
/// transaction moved no funds (an EVM transaction that reverted, or a Solana
/// transaction whose blockhash expired without landing). Retrying a payout that
/// did go through pays the recipient twice.
#[ic_cdk::update(guard = "is_controller")]
fn admin_retry_bridging_task(from_tx: store::BridgeTx) -> Result<store::BridgeLog, String> {
    store::state::retry_pending_task(&from_tx)
}

#[ic_cdk::update]
fn validate_admin_retry_bridging_task(from_tx: store::BridgeTx) -> Result<String, String> {
    let log = store::state::pending_task(&from_tx)?;
    if log.to_tx.as_ref().is_some_and(|tx| tx.is_finalized()) {
        return Err("the outgoing transaction is already finalized, nothing to retry".to_string());
    }
    pretty_format(&(log,))
}

/// Removes a stuck bridging task from the pending queue and archives it with its
/// error preserved, unblocking the chains it references.
///
/// The task is recorded as not bridged: the amount and the fee are left out of
/// the totals, and settling with the user is up to the administrator.
#[ic_cdk::update(guard = "is_controller")]
fn admin_close_bridging_task(from_tx: store::BridgeTx) -> Result<store::BridgeLog, String> {
    let now_ms = ic_cdk::api::time() / 1_000_000;
    store::state::close_pending_task(&from_tx, now_ms)
}

#[ic_cdk::update]
fn validate_admin_close_bridging_task(from_tx: store::BridgeTx) -> Result<String, String> {
    let log = store::state::pending_task(&from_tx)?;
    if log.is_finalized() {
        return Err(
            "the bridging task is already finalized and will be archived automatically".to_string(),
        );
    }
    pretty_format(&(log,))
}

fn available_fees(s: &store::State) -> Result<u128, String> {
    s.total_collected_fees
        .checked_sub(s.total_withdrawn_fees)
        .ok_or_else(|| "total_withdrawn_fees exceeds total_collected_fees".to_string())
}

fn is_controller() -> Result<(), String> {
    let caller = ic_cdk::api::msg_caller();
    if ic_cdk::api::is_controller(&caller)
        || store::state::with(|s| s.governance_canister == Some(caller))
    {
        Ok(())
    } else {
        Err("user is not a controller".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_urls_must_be_absolute_https_uris() {
        assert!(
            check_providers(&["https://rpc.example/v1/key?network=mainnet".to_string()]).is_ok()
        );
        assert!(check_providers(&["http://rpc.example".to_string()]).is_err());
        assert!(check_providers(&["/relative/rpc".to_string()]).is_err());
        assert!(check_providers(&["https:/missing-host".to_string()]).is_err());
    }
}
