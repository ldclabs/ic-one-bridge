//! A minimal writer of the pre-fee State/pending/archive layout. Its upgrade
//! hook appends a legacy log without updating the new implementation's cursor.
use alloy_primitives::Address;
use candid::Principal;
use ic_stable_structures::{
    DefaultMemoryImpl, StableCell, StableLog,
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
};
use serde::Serialize;
use serde_bytes::{ByteArray, ByteBuf};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
};
type Memory = VirtualMemory<DefaultMemoryImpl>;
thread_local! {
    static MANAGER:RefCell<MemoryManager<DefaultMemoryImpl>>=RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));
}
fn mem(id: u8) -> Memory {
    MANAGER.with_borrow(|m| m.get(MemoryId::new(id)))
}
#[derive(Clone, Serialize)]
enum Target {
    Icp,
    Evm(String),
}
#[derive(Clone, Serialize)]
enum Tx {
    Icp(bool, u64),
    Evm(bool, ByteArray<32>),
}
#[derive(Clone, Serialize)]
struct Log {
    id: Option<u64>,
    user: Principal,
    from: Target,
    to: Target,
    icp_amount: u128,
    from_tx: Tx,
    to_tx: Option<Tx>,
    to_addr: Option<String>,
    created_at: u64,
    finalized_at: u64,
    error: Option<String>,
}
#[derive(Serialize)]
struct Key {
    public_key: ByteBuf,
    chain_code: ByteBuf,
}
#[derive(Serialize)]
struct OldState {
    key_name: String,
    icp_address: Principal,
    evm_address: Address,
    token_name: String,
    token_symbol: String,
    token_decimals: u8,
    token_logo: String,
    token_ledger: Principal,
    min_threshold_to_bridge: u128,
    evm_token_contracts: HashMap<String, (Address, u8, u64)>,
    evm_latest_gas: HashMap<String, (u64, u128, u128)>,
    evm_providers: HashMap<String, (u64, Vec<String>)>,
    ecdsa_public_key: Key,
    governance_canister: Option<Principal>,
    pending: VecDeque<Log>,
    finalize_bridging_round: (u64, bool),
}
fn owner() -> Principal {
    Principal::self_authenticating([7; 32])
}
fn log(id: u64) -> Log {
    Log {
        id: None,
        user: owner(),
        from: Target::Icp,
        to: Target::Evm("ETH".into()),
        icp_amount: 100,
        from_tx: Tx::Icp(true, id),
        to_tx: None,
        to_addr: None,
        created_at: 1,
        finalized_at: 0,
        error: None,
    }
}
fn append_old() {
    let logs: StableLog<Vec<u8>, Memory, Memory> = StableLog::init(mem(2), mem(3));
    let mut item = log(100 + logs.len());
    item.to_tx = Some(Tx::Evm(true, [1; 32].into()));
    item.finalized_at = 2;
    logs.append(&ic_auth_types::cbor_into_vec(&item).unwrap())
        .unwrap();
}
#[ic_cdk::init]
fn init(ledger: Principal, scenario: Option<u8>) {
    let scenario = scenario.unwrap_or(0);
    let archive_count = if scenario == 2 { 150 } else { 1 };
    let pending = if scenario == 3 {
        // A confirmed external deposit awaiting its first ICP payout, with no
        // historical ICP fee income or any of the unreleased budget fields.
        let mut item = log(7);
        item.from = Target::Evm("BNB".into());
        item.to = Target::Icp;
        item.from_tx = Tx::Evm(true, [7; 32].into());
        item
    } else {
        log(if scenario == 0 {
            7
        } else {
            100 + archive_count - 1
        })
    };
    let state = OldState {
        key_name: "test_key_1".into(),
        icp_address: ic_cdk::api::canister_self(),
        evm_address: Address::ZERO,
        token_name: "Fixture".into(),
        token_symbol: "TEST".into(),
        token_decimals: 8,
        token_logo: "".into(),
        token_ledger: ledger,
        min_threshold_to_bridge: 1,
        evm_token_contracts: HashMap::new(),
        evm_latest_gas: HashMap::new(),
        evm_providers: HashMap::new(),
        ecdsa_public_key: Key {
            public_key: vec![].into(),
            chain_code: vec![].into(),
        },
        governance_canister: None,
        pending: VecDeque::from([pending]),
        finalize_bridging_round: (0, false),
    };
    let mut cell = StableCell::init(mem(0), Vec::new());
    cell.set(ic_auth_types::cbor_into_vec(&state).unwrap());
    for _ in 0..archive_count {
        append_old();
    }
}
#[ic_cdk::post_upgrade]
fn upgraded() {
    append_old();
}
