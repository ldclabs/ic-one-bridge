use alloy_primitives::Address;
use candid::{CandidType, IDLValue, Principal, pretty::candid::value::pp_value};
use url::Url;

use crate::store;

#[ic_cdk::update(guard = "is_controller")]
async fn admin_add_evm_contract(
    chain_name: String,
    chain_id: u64,
    address: String,
) -> Result<(), String> {
    let address = check_admin_add_evm_contract(&chain_name, chain_id, &address)?;
    let cli = store::state::evm_client(&chain_name);
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let (cid, gas_price, max_priority_fee_per_gas, symbol, decimals) = futures::future::try_join5(
        cli.chain_id(now_ms),
        cli.gas_price(now_ms),
        cli.max_priority_fee_per_gas(now_ms),
        cli.erc20_symbol(now_ms, &address),
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
        if s.token_symbol != symbol {
            return Err(format!(
                "token_symbol mismatch, got {}, expected {}",
                symbol, s.token_symbol
            ));
        }

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
fn admin_set_evm_providers(
    chain_name: String,
    max_confirmations: u64,
    providers: Vec<String>,
) -> Result<(), String> {
    for url in &providers {
        let v = Url::parse(url).map_err(|err| format!("invalid url {url}, error: {err}"))?;
        if v.scheme() != "https" {
            return Err(format!("url scheme must be https, got: {url}"));
        }
    }
    if max_confirmations < 2 {
        return Err("max_confirmations must be at least 2".to_string());
    }

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
    for url in &providers {
        let v = Url::parse(url).map_err(|err| format!("invalid url {url}, error: {err}"))?;
        if v.scheme() != "https" {
            return Err(format!("url scheme must be https, got: {url}"));
        }
    }
    if max_confirmations < 2 {
        return Err("max_confirmations must be at least 2".to_string());
    }
    pretty_format(&(chain_name, max_confirmations, providers))
}

#[ic_cdk::update(guard = "is_controller")]
async fn admin_collect_fees(to: Principal, icp_amount: u128) -> Result<store::BridgeTx, String> {
    let ledger = store::state::with(|s| {
        if icp_amount == 0 {
            return Err("amount must be greater than 0".to_string());
        }
        if icp_amount + s.total_withdrawn_fees > s.total_collected_fees {
            return Err(format!(
                "amount {} exceeds available fees {}",
                icp_amount,
                s.total_collected_fees - s.total_withdrawn_fees
            ));
        }

        Ok(s.token_ledger)
    })?;

    let tx = store::state::to_icp(ledger, to, icp_amount).await?;
    store::state::with_mut(|s| {
        s.total_withdrawn_fees += icp_amount;
    });
    Ok(tx)
}

#[ic_cdk::update]
async fn validate_admin_collect_fees(to: Principal, icp_amount: u128) -> Result<String, String> {
    store::state::with(|s| {
        if icp_amount == 0 {
            return Err("icp_amount must be greater than 0".to_string());
        }
        if icp_amount > s.total_collected_fees - s.total_withdrawn_fees {
            return Err(format!(
                "icp_amount {} exceeds available fees {}",
                icp_amount,
                s.total_collected_fees - s.total_withdrawn_fees
            ));
        }
        Ok(())
    })?;
    pretty_format(&(to, icp_amount))
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

fn pretty_format<T>(data: &T) -> Result<String, String>
where
    T: CandidType,
{
    let val = IDLValue::try_from_candid_type(data).map_err(|err| format!("{err:?}"))?;
    let doc = pp_value(7, &val);

    Ok(format!("{}", doc.pretty(120)))
}
