use alloy_primitives::Address;
use candid::Principal;
use http::Uri;
use std::collections::BTreeSet;

use crate::{
    helper::{now_ms, pretty_format, validate_principals},
    store,
    svm::{Pubkey, get_mint_decimals},
};

/// Chain names are keys of the EVM configuration and prefixes of the errors
/// the chain gate in `bridge()` matches on, so they are short, uppercase and
/// never one of the built-in targets.
const EVM_CHAIN_NAME_MAX_LEN: usize = 8;
const RESERVED_CHAIN_NAMES: [&str; 2] = ["ICP", "SOL"];

/// Every read a payout depends on is asked of two providers and acted on only
/// when they agree, so a chain needs at least two; three keep it working while
/// one is down.
const MIN_PROVIDERS: usize = 2;

#[ic_cdk::update(guard = "is_controller")]
fn admin_add_bridges(args: BTreeSet<Principal>) -> Result<(), String> {
    validate_principals(&args)?;
    let mut args = args;
    store::state::with_mut(|s| {
        s.sub_bridges.append(&mut args);
        Ok(())
    })
}

#[ic_cdk::update(guard = "is_controller")]
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

#[ic_cdk::update(guard = "is_controller")]
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
    let cli = store::state::evm_client(&chain_name)?;
    let now_ms = now_ms();
    let (cid, gas_price, max_priority_fee_per_gas, decimals) = futures::future::try_join4(
        cli.chain_id(),
        cli.gas_price(),
        cli.max_priority_fee_per_gas(),
        cli.erc20_decimals(&address),
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

#[ic_cdk::update(guard = "is_controller")]
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
    check_evm_chain_name(chain_name)?;

    let addr = Address::parse_checksummed(address, None)
        .map_err(|err| format!("invalid address {address}: {err:?}"))?;

    store::state::with(|s| {
        if s.evm_token_contracts.contains_key(chain_name) {
            return Err("chain_name already exists".to_string());
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
    let account = cli.get_account_info(&address).await?;
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

#[ic_cdk::update(guard = "is_controller")]
fn validate_admin_add_svm_contract(address: String) -> Result<String, String> {
    check_admin_add_svm_contract(&address)?;
    pretty_format(&(address,))
}

fn check_evm_chain_name(chain_name: &str) -> Result<(), String> {
    if chain_name.is_empty()
        || chain_name.len() > EVM_CHAIN_NAME_MAX_LEN
        || !chain_name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(format!(
            "chain_name must be 1 to {EVM_CHAIN_NAME_MAX_LEN} uppercase letters or digits, got: {chain_name}"
        ));
    }
    if RESERVED_CHAIN_NAMES.contains(&chain_name) {
        return Err(format!("chain_name {chain_name} is reserved"));
    }
    Ok(())
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

/// Sets the RPC providers of an EVM chain and when a transaction on it counts
/// as final: after `max_confirmations` blocks, or, when it is `0`, once the
/// chain's own `finalized` block tag has passed it. The tag is the safer
/// choice on every chain that supports it.
#[ic_cdk::update(guard = "is_controller")]
fn admin_set_evm_providers(
    chain_name: String,
    max_confirmations: u64,
    providers: Vec<String>,
) -> Result<(), String> {
    check_evm_providers(&chain_name, max_confirmations, &providers)?;

    store::state::with_mut(|s| {
        s.evm_providers
            .insert(chain_name, (max_confirmations, providers));
        Ok(())
    })
}

#[ic_cdk::update(guard = "is_controller")]
fn validate_admin_set_evm_providers(
    chain_name: String,
    max_confirmations: u64,
    providers: Vec<String>,
) -> Result<String, String> {
    check_evm_providers(&chain_name, max_confirmations, &providers)?;
    pretty_format(&(chain_name, max_confirmations, providers))
}

fn check_evm_providers(
    chain_name: &str,
    max_confirmations: u64,
    providers: &[String],
) -> Result<(), String> {
    check_evm_chain_name(chain_name)?;
    check_providers(providers)?;
    if max_confirmations == 1 {
        return Err(
            "max_confirmations must be at least 2, or 0 to rely on the chain's finalized block tag"
                .to_string(),
        );
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

#[ic_cdk::update(guard = "is_controller")]
fn validate_admin_set_svm_providers(providers: Vec<String>) -> Result<String, String> {
    check_providers(&providers)?;
    pretty_format(&(providers,))
}

fn check_providers(providers: &[String]) -> Result<(), String> {
    if providers.len() < MIN_PROVIDERS {
        return Err(format!(
            "at least {MIN_PROVIDERS} providers are needed, got {}",
            providers.len()
        ));
    }
    let distinct: BTreeSet<&String> = providers.iter().collect();
    if distinct.len() != providers.len() {
        return Err("providers must be distinct".to_string());
    }
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

/// Withdraws collected fees. Only the fees that sit on the ICP ledger — those
/// of tasks deposited on ICP — can be taken from it; a task deposited on
/// another chain left its fee there.
#[ic_cdk::update(guard = "is_controller")]
async fn admin_collect_fees(to: Principal, icp_amount: u128) -> Result<store::BridgeTx, String> {
    let ledger = store::state::with_mut(|s| {
        if icp_amount == 0 {
            return Err("amount must be greater than 0".to_string());
        }
        let available = available_fees(s);
        if icp_amount > available {
            return Err(format!(
                "amount {} exceeds the fees available on the ICP ledger {}",
                icp_amount, available
            ));
        }
        s.total_withdrawn_fees = s
            .total_withdrawn_fees
            .checked_add(icp_amount)
            .ok_or_else(|| "total_withdrawn_fees overflow".to_string())?;

        Ok(s.token_ledger)
    })?;

    match store::state::to_icp(ledger, to, icp_amount, None).await {
        Ok(tx) => Ok(tx),
        Err(err) => {
            store::state::with_mut(|s| {
                s.total_withdrawn_fees = s.total_withdrawn_fees.saturating_sub(icp_amount);
            });
            Err(err.to_string())
        }
    }
}

#[ic_cdk::update(guard = "is_controller")]
fn validate_admin_collect_fees(to: Principal, icp_amount: u128) -> Result<String, String> {
    store::state::with(|s| {
        if icp_amount == 0 {
            return Err("icp_amount must be greater than 0".to_string());
        }
        let available = available_fees(s);
        if icp_amount > available {
            return Err(format!(
                "icp_amount {} exceeds the fees available on the ICP ledger {}",
                icp_amount, available
            ));
        }
        Ok(())
    })?;
    pretty_format(&(to, icp_amount))
}

/// Resets the error circuit breaker and re-arms the finalization timer chain.
///
/// Once `error_rounds` reaches its limit, new tasks are refused and the rounds
/// slow to an hourly cooldown that lifts the pause by itself after a clean
/// round. Use this to lift it right away once the cause has been dealt with.
#[ic_cdk::update(guard = "is_controller")]
fn admin_restart_bridging() -> Result<u64, String> {
    Ok(store::state::restart_finalize_bridging())
}

#[ic_cdk::update(guard = "is_controller")]
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

/// Drops the outgoing transaction and the error of a stuck bridging task so
/// the next finalization round pays it out afresh, optionally somewhere else.
///
/// `to` and `to_addr` replace the task's target: a corrected address, the
/// user's own address on the same chain (`to_addr = null`), or the chain the
/// deposit came from — a refund. They are vetted like a `bridge()` call's.
///
/// Only use this after verifying on chain that the recorded outgoing
/// transaction moved no funds (an EVM transaction that reverted, or a Solana
/// transaction whose blockhash expired without landing). Retrying a payout that
/// did go through pays the recipient twice.
#[ic_cdk::update(guard = "is_controller")]
fn admin_retry_bridging_task(
    from_tx: store::BridgeTx,
    to: Option<store::BridgeTarget>,
    to_addr: Option<String>,
) -> Result<store::BridgeLog, String> {
    store::state::retry_pending_task(&from_tx, to, to_addr)
}

#[ic_cdk::update(guard = "is_controller")]
fn validate_admin_retry_bridging_task(
    from_tx: store::BridgeTx,
    to: Option<store::BridgeTarget>,
    to_addr: Option<String>,
) -> Result<String, String> {
    let log = store::state::pending_task(&from_tx)?;
    if log.to_tx.as_ref().is_some_and(|tx| tx.is_finalized()) {
        return Err("the outgoing transaction is already finalized, nothing to retry".to_string());
    }
    store::state::with(|s| {
        store::state::plan_retry_redirect(s, &log, to.as_ref(), to_addr.as_deref())
    })?;
    pretty_format(&(log, to, to_addr))
}

/// Removes a stuck bridging task from the pending queue and archives it with its
/// error preserved, unblocking the chains it references.
///
/// The task is recorded as not bridged: the amount and the fee are left out of
/// the totals, and settling with the user is up to the administrator — prefer
/// `admin_retry_bridging_task` with a refund target. A task whose payout is
/// broadcast but not confirmed is refused unless `force` is set.
#[ic_cdk::update(guard = "is_controller")]
fn admin_close_bridging_task(
    from_tx: store::BridgeTx,
    force: Option<bool>,
) -> Result<store::BridgeLog, String> {
    store::state::close_pending_task(&from_tx, now_ms(), force.unwrap_or(false))
}

#[ic_cdk::update(guard = "is_controller")]
fn validate_admin_close_bridging_task(
    from_tx: store::BridgeTx,
    force: Option<bool>,
) -> Result<String, String> {
    let log = store::state::pending_task(&from_tx)?;
    if log.is_finalized() {
        return Err(
            "the bridging task is already finalized and will be archived automatically".to_string(),
        );
    }
    if log.payout_in_flight() && !force.unwrap_or(false) {
        return Err(
            "the payout has been broadcast and is not confirmed yet; pass force = true only once it is certain it can no longer land"
                .to_string(),
        );
    }
    pretty_format(&(log, force))
}

/// Fetches whichever of the subnet master keys the canister is still
/// missing, and returns the bridge's EVM and Solana addresses. Bridging that
/// needs a missing key is refused until it is there.
#[ic_cdk::update(guard = "is_controller")]
async fn admin_init_public_keys() -> Result<(String, String), String> {
    store::state::try_init_public_keys().await;
    Ok(store::state::with(|s| {
        (s.evm_address.to_string(), s.svm_address.to_string())
    }))
}

#[ic_cdk::update(guard = "is_controller")]
fn validate_admin_init_public_keys() -> Result<String, String> {
    let (ecdsa, ed25519) = store::state::with(|s| {
        (
            !s.ecdsa_public_key.public_key.is_empty(),
            !s.ed25519_public_key.public_key.is_empty(),
        )
    });
    pretty_format(&(ecdsa, ed25519))
}

fn available_fees(s: &store::State) -> u128 {
    s.icp_collected_fees.saturating_sub(s.total_withdrawn_fees)
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
    fn provider_lists_are_two_or_more_distinct_https_uris() {
        let two = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
        assert!(
            check_providers(&two(
                "https://rpc.example/v1/key?network=mainnet",
                "https://rpc2.example"
            ))
            .is_ok()
        );
        assert!(check_providers(&["https://rpc.example".to_string()]).is_err());
        assert!(check_providers(&two("https://rpc.example", "https://rpc.example")).is_err());
        assert!(check_providers(&two("http://rpc.example", "https://rpc2.example")).is_err());
        assert!(check_providers(&two("/relative/rpc", "https://rpc2.example")).is_err());
        assert!(check_providers(&two("https:/missing-host", "https://rpc2.example")).is_err());
    }

    #[test]
    fn evm_finality_is_a_depth_of_two_or_the_finalized_tag() {
        let providers = vec!["https://a".to_string(), "https://b".to_string()];
        assert!(check_evm_providers("BNB", 0, &providers).is_ok());
        assert!(check_evm_providers("BNB", 1, &providers).is_err());
        assert!(check_evm_providers("BNB", 2, &providers).is_ok());
        assert!(check_evm_providers("BNB", 12, &providers).is_ok());
    }

    #[test]
    fn evm_chain_names_are_short_uppercase_and_not_built_in() {
        assert!(check_evm_chain_name("BNB").is_ok());
        assert!(check_evm_chain_name("ARB1").is_ok());
        assert!(check_evm_chain_name("ABCDEFGH").is_ok());

        assert!(check_evm_chain_name("").is_err());
        assert!(check_evm_chain_name("ABCDEFGHI").is_err());
        assert!(check_evm_chain_name("bnb").is_err());
        assert!(check_evm_chain_name("BNB ").is_err());
        assert!(check_evm_chain_name("B-NB").is_err());
        assert!(check_evm_chain_name("ICP").is_err());
        assert!(check_evm_chain_name("SOL").is_err());
    }
}
