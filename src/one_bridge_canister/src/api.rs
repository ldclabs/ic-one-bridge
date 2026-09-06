use std::str::FromStr;

use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::Bytes;
use candid::Principal;
use ic_auth_types::ByteBufB64;
use serde_bytes::ByteBuf;

use crate::{
    helper::{check_auth, msg_caller, now_ms, parse_evm_address},
    store::{self, Funding},
    svm::Pubkey,
};

#[ic_cdk::query]
fn info() -> Result<store::StateInfo, String> {
    Ok(store::state::info())
}

#[ic_cdk::query]
fn evm_address(user: Option<Principal>) -> Result<String, String> {
    let user = user.unwrap_or_else(ic_cdk::api::msg_caller);
    check_auth(&user)?;
    let addr = store::state::evm_address(&user)?;
    Ok(addr.to_string())
}

#[ic_cdk::query]
fn svm_address(user: Option<Principal>) -> Result<String, String> {
    let user = user.unwrap_or_else(ic_cdk::api::msg_caller);
    check_auth(&user)?;
    let addr = store::state::svm_address(&user)?;
    Ok(addr.to_string())
}

#[ic_cdk::query]
fn my_pending_logs() -> Result<Vec<store::BridgeLog>, String> {
    let caller = msg_caller()?;
    let rt = store::pending::page(Some(caller), store::PENDING_LOGS_LIMIT, None);
    Ok(rt)
}

#[ic_cdk::query]
fn my_finalized_logs(take: u32, prev: Option<u64>) -> Result<Vec<store::BridgeLog>, String> {
    let caller = msg_caller()?;
    let take = take.clamp(2, 100) as usize;
    let rt = store::state::user_logs(caller, take, prev);
    Ok(rt)
}

#[ic_cdk::query]
fn my_bridge_log(from_tx: store::BridgeTx) -> Result<store::BridgeLog, String> {
    let caller = msg_caller()?;
    let log = store::state::my_bridge_log(caller, from_tx);
    // The archive scan is bounded, so a miss does not prove the transaction was
    // never bridged — only that it is not among the recent records.
    log.ok_or_else(|| {
        "tx log not found in pending tasks or recent archived logs; \
         page my_finalized_logs for older history"
            .to_string()
    })
}

/// The oldest pending tasks, at most `PENDING_LOGS_LIMIT` of them.
#[ic_cdk::query]
fn pending_logs() -> Result<Vec<store::BridgeLog>, String> {
    let rt = store::pending::page(None, store::PENDING_LOGS_LIMIT, None);
    Ok(rt)
}

#[ic_cdk::query]
fn finalized_logs(take: u32, prev: Option<u64>) -> Result<Vec<store::BridgeLog>, String> {
    let take = take.clamp(2, 100) as usize;
    let rt = store::state::logs(take, prev);
    Ok(rt)
}

#[ic_cdk::update(guard = "admit_request")]
async fn bridge(
    from_chain: String,
    to_chain: String,
    icp_amount: u128,
    to: Option<String>,
) -> Result<store::BridgeTx, String> {
    let caller = msg_caller()?;
    store::state::bridge(from_chain, to_chain, icp_amount, to, caller, now_ms()).await
}

#[ic_cdk::update(guard = "admit_request")]
async fn erc20_transfer_tx(chain: String, to: String, icp_amount: u128) -> Result<String, String> {
    let to_addr = parse_evm_address(&to)?;
    let caller = msg_caller()?;
    let _signing = store::acquire_active_bridge_user(caller)?;
    let (_, signed_tx) = store::state::build_erc20_transfer_tx(
        &chain,
        &caller,
        &to_addr,
        icp_amount,
        now_ms(),
        Funding::Verify,
    )
    .await?;
    let data = signed_tx.encoded_2718();
    Ok(Bytes::from(data).to_string())
}

#[ic_cdk::update(guard = "admit_request")]
async fn erc20_transfer(chain: String, to: String, icp_amount: u128) -> Result<String, String> {
    let to_addr = parse_evm_address(&to)?;
    let caller = msg_caller()?;
    let _signing = store::acquire_active_bridge_user(caller)?;
    let (cli, signed_tx) = store::state::build_erc20_transfer_tx(
        &chain,
        &caller,
        &to_addr,
        icp_amount,
        now_ms(),
        Funding::Verify,
    )
    .await?;
    let tx_hash = signed_tx.hash().to_string();

    let data = signed_tx.encoded_2718();
    let _ = cli
        .send_raw_transaction(Bytes::from(data).to_string())
        .await?;

    Ok(tx_hash)
}

#[ic_cdk::update(guard = "admit_request")]
async fn evm_transfer_tx(chain: String, to: String, evm_amount: u128) -> Result<String, String> {
    let to_addr = parse_evm_address(&to)?;
    let caller = msg_caller()?;
    let _signing = store::acquire_active_bridge_user(caller)?;
    let (_, signed_tx) = store::state::build_evm_transfer_tx(
        &chain,
        &caller,
        &to_addr,
        evm_amount,
        now_ms(),
        Funding::Verify,
    )
    .await?;
    let data = signed_tx.encoded_2718();
    Ok(Bytes::from(data).to_string())
}

#[ic_cdk::update(guard = "admit_request")]
async fn spl_transfer_tx(to: String, icp_amount: u128) -> Result<String, String> {
    let to_addr = Pubkey::from_str(&to).map_err(|err| format!("invalid to address: {}", err))?;
    let caller = msg_caller()?;
    let _signing = store::acquire_active_bridge_user(caller)?;
    let (_, signed_tx, _) =
        store::state::build_spl_transfer_tx(&caller, &to_addr, icp_amount, Funding::Verify).await?;
    let data = bincode::serialize(&signed_tx)
        .map_err(|err| format!("failed to serialize signed tx: {}", err))?;
    Ok(ByteBufB64::from(data).to_base64())
}

#[ic_cdk::update(guard = "admit_request")]
async fn sol_transfer_tx(to: String, sol_amount: u64) -> Result<String, String> {
    let to_addr = Pubkey::from_str(&to).map_err(|err| format!("invalid to address: {}", err))?;
    let caller = msg_caller()?;
    let _signing = store::acquire_active_bridge_user(caller)?;
    let (_, signed_tx, _) =
        store::state::build_sol_transfer_tx(&caller, &to_addr, sol_amount, Funding::Verify).await?;
    let data = bincode::serialize(&signed_tx)
        .map_err(|err| format!("failed to serialize signed tx: {}", err))?;
    Ok(ByteBufB64::from(data).to_base64())
}

#[ic_cdk::update(guard = "admit_request")]
async fn evm_sign(message_hash: ByteBuf) -> Result<ByteBuf, String> {
    let caller = msg_caller()?;
    if message_hash.len() != 32 {
        return Err("message_hash must be 32 bytes".to_string());
    }

    let sig = store::state::evm_sign(&caller, message_hash.into_vec()).await?;
    Ok(sig.into())
}

fn admit_request() -> Result<(), String> {
    let caller = msg_caller()?;
    if ic_cdk::api::is_controller(&caller)
        || store::state::with(|s| s.governance_canister == Some(caller))
    {
        return Ok(());
    }
    store::budget::admit(caller)
}

#[ic_cdk::query]
fn my_pending_logs_page(take: u32, after: Option<u64>) -> Result<Vec<store::BridgeLog>, String> {
    Ok(store::pending::page(
        Some(msg_caller()?),
        take.clamp(1, 100) as usize,
        after,
    ))
}

#[ic_cdk::query]
fn pending_logs_page(take: u32, after: Option<u64>) -> Result<Vec<store::BridgeLog>, String> {
    Ok(store::pending::page(
        None,
        take.clamp(1, 100) as usize,
        after,
    ))
}

#[ic_cdk::query]
fn my_operations(take: u32, before: Option<u64>) -> Result<Vec<store::OperationInfo>, String> {
    Ok(store::state::operations(
        msg_caller()?,
        take as usize,
        before,
    ))
}

#[ic_cdk::update(guard = "admit_request")]
async fn bridge_with_id(
    from_chain: String,
    to_chain: String,
    amount: u128,
    to: Option<String>,
    request_id: ByteBuf,
) -> Result<store::BridgeTx, String> {
    store::state::bridge_with_id(
        from_chain,
        to_chain,
        amount,
        to,
        msg_caller()?,
        now_ms(),
        Some(request_id),
    )
    .await
}

#[ic_cdk::update(guard = "admit_request")]
async fn resume_deposit(operation_id: u64) -> Result<store::BridgeTx, String> {
    store::state::resume_deposit(operation_id, msg_caller()?).await
}

#[ic_cdk::update(guard = "admit_request")]
async fn fund_ledger_fees(amount: u128) -> Result<store::BridgeTx, String> {
    store::state::fund_ledger_fees(msg_caller()?, amount).await
}

#[ic_cdk::update(guard = "admit_request")]
async fn resume_operation(id: u64) -> Result<store::BridgeTx, String> {
    store::state::resume_operation(id, msg_caller()?).await
}
#[ic_cdk::update]
fn cancel_operation(id: u64) -> Result<(), String> {
    store::state::cancel_operation(id, msg_caller()?)
}

#[ic_cdk::update(guard = "admit_request")]
fn recheck_task(from_tx: store::BridgeTx) -> Result<(), String> {
    store::state::recheck_task(&from_tx, msg_caller()?, false)
}
