use std::str::FromStr;

use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes};
use candid::Principal;
use ic_auth_types::ByteBufB64;
use serde_bytes::ByteBuf;

use crate::{
    helper::{check_auth, msg_caller},
    store,
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
    let addr = store::state::evm_address(&user);
    Ok(addr.to_string())
}

#[ic_cdk::query]
fn svm_address(user: Option<Principal>) -> Result<String, String> {
    let user = user.unwrap_or_else(ic_cdk::api::msg_caller);
    check_auth(&user)?;
    let addr = store::state::svm_address(&user);
    Ok(addr.to_string())
}

#[ic_cdk::query]
fn my_pending_logs() -> Result<Vec<store::BridgeLog>, String> {
    let caller = msg_caller()?;
    let rt = store::state::with(|s| {
        s.pending
            .iter()
            .filter_map(|item| {
                if item.user == caller {
                    Some(item.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<store::BridgeLog>>()
    });
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
    log.ok_or_else(|| "tx log not found".to_string())
}

#[ic_cdk::query]
fn pending_logs() -> Result<Vec<store::BridgeLog>, String> {
    let rt = store::state::with(|s| s.pending.iter().cloned().collect::<Vec<store::BridgeLog>>());
    Ok(rt)
}

#[ic_cdk::query]
fn finalized_logs(take: u32, prev: Option<u64>) -> Result<Vec<store::BridgeLog>, String> {
    let take = take.clamp(2, 100) as usize;
    let rt = store::state::logs(take, prev);
    Ok(rt)
}

#[ic_cdk::update]
async fn bridge(
    from_chain: String,
    to_chain: String,
    icp_amount: u128,
    to: Option<String>,
) -> Result<store::BridgeTx, String> {
    let caller = msg_caller()?;
    let now_ms = ic_cdk::api::time() / 1_000_000;
    store::state::bridge(from_chain, to_chain, icp_amount, to, caller, now_ms).await
}

#[ic_cdk::update]
async fn erc20_transfer_tx(chain: String, to: String, icp_amount: u128) -> Result<String, String> {
    let to_addr = to
        .parse::<Address>()
        .map_err(|err| format!("invalid to address: {}", err))?;
    let caller = msg_caller()?;
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let (_, signed_tx) =
        store::state::build_erc20_transfer_tx(&chain, &caller, &to_addr, icp_amount, now_ms)
            .await?;
    let data = signed_tx.encoded_2718();
    Ok(Bytes::from(data).to_string())
}

#[ic_cdk::update]
async fn erc20_transfer(chain: String, to: String, icp_amount: u128) -> Result<String, String> {
    let to_addr = to
        .parse::<Address>()
        .map_err(|err| format!("invalid to address: {}", err))?;
    let caller = msg_caller()?;
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let (cli, signed_tx) =
        store::state::build_erc20_transfer_tx(&chain, &caller, &to_addr, icp_amount, now_ms)
            .await?;
    let tx_hash = signed_tx.hash().to_string();

    let data = signed_tx.encoded_2718();
    let _ = cli
        .send_raw_transaction(now_ms, Bytes::from(data).to_string())
        .await?;

    Ok(tx_hash)
}

#[ic_cdk::update]
async fn evm_transfer_tx(chain: String, to: String, evm_amount: u128) -> Result<String, String> {
    let to_addr = to
        .parse::<Address>()
        .map_err(|err| format!("invalid to address: {}", err))?;
    let caller = msg_caller()?;
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let (_, signed_tx) =
        store::state::build_evm_transfer_tx(&chain, &caller, &to_addr, evm_amount, now_ms).await?;
    let data = signed_tx.encoded_2718();
    Ok(Bytes::from(data).to_string())
}

#[ic_cdk::update]
async fn spl_transfer_tx(to: String, icp_amount: u128) -> Result<String, String> {
    let to_addr = Pubkey::from_str(&to).map_err(|err| format!("invalid to address: {}", err))?;
    let caller = msg_caller()?;
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let (_, signed_tx) =
        store::state::build_spl_transfer_tx(&caller, &to_addr, icp_amount, now_ms).await?;
    let data = bincode::serialize(&signed_tx)
        .map_err(|err| format!("failed to serialize signed tx: {}", err))?;
    Ok(ByteBufB64::from(data).to_base64())
}

#[ic_cdk::update]
async fn sol_transfer_tx(to: String, sol_amount: u64) -> Result<String, String> {
    let to_addr = Pubkey::from_str(&to).map_err(|err| format!("invalid to address: {}", err))?;
    let caller = msg_caller()?;
    let now_ms = ic_cdk::api::time() / 1_000_000;
    let (_, signed_tx) =
        store::state::build_sol_transfer_tx(&caller, &to_addr, sol_amount, now_ms).await?;
    let data = bincode::serialize(&signed_tx)
        .map_err(|err| format!("failed to serialize signed tx: {}", err))?;
    Ok(ByteBufB64::from(data).to_base64())
}

#[ic_cdk::update]
async fn evm_sign(message_hash: ByteBuf) -> Result<ByteBuf, String> {
    let caller = msg_caller()?;
    if message_hash.len() != 32 {
        return Err("message_hash must be 32 bytes".to_string());
    }

    let sig = store::state::evm_sign(&caller, message_hash.into_vec()).await?;
    Ok(sig.into())
}
