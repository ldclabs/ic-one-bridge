use alloy_consensus::{SignableTransaction, Signed, TxEip1559};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxHash, U256, hex};
use candid::{CandidType, Nat, Principal};
use ciborium::{from_reader, into_writer};
use ic_http_certification::{
    HttpCertification, HttpCertificationPath, HttpCertificationTree, HttpCertificationTreeEntry,
    cel::{DefaultCelBuilder, create_cel_expr},
};
use ic_stable_structures::{
    DefaultMemoryImpl, StableBTreeMap, StableCell, StableLog, Storable,
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    storable::Bound,
};
use icrc_ledger_types::{
    icrc1::{account::Account, transfer::TransferArg},
    icrc2::transfer_from::{TransferFromArgs, TransferFromError},
};
use num_traits::cast::ToPrimitive;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteArray;
use std::{
    borrow::Cow,
    cell::RefCell,
    cmp,
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    time::Duration,
};

use crate::{
    ecdsa::{
        PublicKeyOutput, cost_sign_with_ecdsa, derive_public_key, ecdsa_public_key, sign_with_ecdsa,
    },
    evm::{EvmClient, encode_erc20_transfer},
    helper::{call, convert_amount, format_error},
    outcall::DefaultHttpOutcall,
};

type Memory = VirtualMemory<DefaultMemoryImpl>;

const MAX_ERROR_ROUNDS: u64 = 42;

#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    pub key_name: String,
    pub icp_address: Principal,
    pub evm_address: Address,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_logo: String,
    pub token_ledger: Principal,
    #[serde(default)]
    pub token_bridge_fee: u128, // with the same decimals as token
    pub min_threshold_to_bridge: u128,
    // chain_name => (contract_address, decimals, chain_id)
    pub evm_token_contracts: HashMap<String, (Address, u8, u64)>,
    // chain_name => (gas_updated_at, gas_price, max_priority_fee_per_gas)
    pub evm_latest_gas: HashMap<String, (u64, u128, u128)>,
    // chain_name => (max_confirmations, [provider_url])
    pub evm_providers: HashMap<String, (u64, Vec<String>)>,
    pub ecdsa_public_key: PublicKeyOutput,
    pub governance_canister: Option<Principal>,
    pub pending: VecDeque<BridgeLog>,
    // (round, running)
    pub finalize_bridging_round: (u64, bool),
    #[serde(default)]
    pub total_bridged_tokens: u128,
    #[serde(default)]
    pub total_collected_fees: u128,
    #[serde(default)]
    pub total_withdrawn_fees: u128,
    #[serde(default)]
    pub sub_bridges: BTreeSet<Principal>,
    #[serde(default)]
    pub error_rounds: u64,
}

#[derive(CandidType, Serialize, Deserialize)]
pub struct StateInfo {
    pub key_name: String,
    pub icp_address: Principal,
    pub evm_address: String,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_logo: String,
    pub token_ledger: Principal,
    pub token_bridge_fee: u128,
    pub min_threshold_to_bridge: u128,
    pub evm_token_contracts: HashMap<String, (String, u8, u64)>,
    pub evm_latest_gas: HashMap<String, (u64, u128, u128)>,
    pub evm_providers: HashMap<String, (u64, Vec<String>)>,
    pub finalize_bridging_round: (u64, bool),
    pub total_bridged_tokens: u128,
    pub total_collected_fees: u128,
    pub total_withdrawn_fees: u128,
    pub total_bridge_count: u64,
    pub sub_bridges: BTreeSet<Principal>,
    pub error_rounds: u64,
    pub governance_canister: Option<Principal>,
}

impl From<&State> for StateInfo {
    fn from(s: &State) -> Self {
        Self {
            key_name: s.key_name.clone(),
            icp_address: s.icp_address,
            evm_address: s.evm_address.to_string(),
            token_name: s.token_name.clone(),
            token_symbol: s.token_symbol.clone(),
            token_decimals: s.token_decimals,
            token_logo: s.token_logo.clone(),
            token_ledger: s.token_ledger,
            token_bridge_fee: s.token_bridge_fee,
            min_threshold_to_bridge: s.min_threshold_to_bridge,
            evm_token_contracts: s
                .evm_token_contracts
                .iter()
                .map(|(k, v)| (k.clone(), (v.0.to_string(), v.1, v.2)))
                .collect(),

            evm_latest_gas: s
                .evm_latest_gas
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            evm_providers: s
                .evm_providers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            finalize_bridging_round: s.finalize_bridging_round,
            total_bridged_tokens: s.total_bridged_tokens,
            total_collected_fees: s.total_collected_fees,
            total_withdrawn_fees: s.total_withdrawn_fees,
            total_bridge_count: 0,
            sub_bridges: s.sub_bridges.clone(),
            error_rounds: s.error_rounds,
            governance_canister: s.governance_canister,
        }
    }
}

impl State {
    fn new() -> Self {
        Self {
            key_name: "dfx_test_key".to_string(),
            icp_address: ic_cdk::api::canister_self(),
            evm_address: [0u8; 20].into(),
            token_name: "ICPanda".to_string(),
            token_symbol: "PANDA".to_string(),
            token_decimals: 8,
            token_logo: "https://532er-faaaa-aaaaj-qncpa-cai.icp0.io/f/374?inline&filename=1734188626561.webp".to_string(),
            token_ledger: Principal::from_text("druyg-tyaaa-aaaaq-aactq-cai").unwrap(), // mainnet ledger
            token_bridge_fee: 0,
            min_threshold_to_bridge: 100_000_000, // 1 Token (8 decimals)
            evm_token_contracts: HashMap::new(),
            evm_providers: HashMap::new(),
            evm_latest_gas: HashMap::new(),
            ecdsa_public_key: PublicKeyOutput {
                public_key: vec![].into(),
                chain_code: vec![].into(),
            },
            governance_canister: None,
            pending: VecDeque::new(),
            finalize_bridging_round: (0, false),
            total_bridged_tokens: 0,
            total_collected_fees: 0,
            total_withdrawn_fees: 0,
            sub_bridges: BTreeSet::new(),
            error_rounds: 0,
        }
    }
}

#[derive(Clone, CandidType, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BridgeTarget {
    Icp,
    Evm(String), // chain_name
}

#[derive(Clone, CandidType, Serialize, Deserialize)]
pub enum BridgeTx {
    Icp(bool, u64),           // (finalized, block_height)
    Evm(bool, ByteArray<32>), // (finalized, tx_hash)
}

impl cmp::PartialEq for BridgeTx {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BridgeTx::Icp(_, tx1), BridgeTx::Icp(_, tx2)) => tx1 == tx2,
            (BridgeTx::Evm(_, tx1), BridgeTx::Evm(_, tx2)) => tx1 == tx2,
            _ => false,
        }
    }
}

impl BridgeTx {
    pub fn is_finalized(&self) -> bool {
        match self {
            BridgeTx::Icp(finalized, _) => *finalized,
            BridgeTx::Evm(finalized, _) => *finalized,
        }
    }

    pub fn same_with(&self, other: &BridgeTx) -> bool {
        match (self, other) {
            (BridgeTx::Icp(_, tx1), BridgeTx::Icp(_, tx2)) => tx1 == tx2,
            (BridgeTx::Evm(_, tx1), BridgeTx::Evm(_, tx2)) => tx1 == tx2,
            _ => false,
        }
    }
}

#[derive(Clone, CandidType, Serialize, Deserialize)]
pub struct BridgeLog {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub user: Principal,
    pub from: BridgeTarget,
    pub to: BridgeTarget,
    pub icp_amount: u128,
    pub fee: u128,
    pub from_tx: BridgeTx,
    pub to_tx: Option<BridgeTx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_addr: Option<String>,
    pub created_at: u64,
    pub finalized_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, CandidType, Serialize, Deserialize)]
pub struct BridgeLogLocal {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(rename = "u", alias = "user")]
    pub user: Principal,
    #[serde(rename = "f", alias = "from")]
    pub from: BridgeTarget,
    #[serde(rename = "t", alias = "to")]
    pub to: BridgeTarget,
    #[serde(rename = "a", alias = "icp_amount")]
    pub icp_amount: u128,
    #[serde(default, rename = "e", alias = "fee")]
    pub fee: u128,
    #[serde(rename = "ft", alias = "from_tx")]
    pub from_tx: BridgeTx,
    #[serde(rename = "tt", alias = "to_tx")]
    pub to_tx: Option<BridgeTx>,
    #[serde(
        rename = "ta",
        alias = "to_addr",
        skip_serializing_if = "Option::is_none"
    )]
    pub to_addr: Option<String>,
    #[serde(rename = "ca", alias = "created_at")]
    pub created_at: u64,
    #[serde(rename = "fa", alias = "finalized_at")]
    pub finalized_at: u64,
    #[serde(
        rename = "er",
        alias = "error",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<String>,
}

impl From<BridgeLogLocal> for BridgeLog {
    fn from(log: BridgeLogLocal) -> Self {
        Self {
            id: log.id,
            user: log.user,
            from: log.from,
            to: log.to,
            icp_amount: log.icp_amount,
            fee: log.fee,
            from_tx: log.from_tx,
            to_tx: log.to_tx,
            to_addr: log.to_addr,
            created_at: log.created_at,
            finalized_at: log.finalized_at,
            error: log.error,
        }
    }
}

impl From<BridgeLog> for BridgeLogLocal {
    fn from(log: BridgeLog) -> Self {
        Self {
            id: log.id,
            user: log.user,
            from: log.from,
            to: log.to,
            icp_amount: log.icp_amount,
            fee: log.fee,
            from_tx: log.from_tx,
            to_tx: log.to_tx,
            to_addr: log.to_addr,
            created_at: log.created_at,
            finalized_at: log.finalized_at,
            error: log.error,
        }
    }
}

impl BridgeLog {
    pub fn is_finalized(&self) -> bool {
        self.from_tx.is_finalized() && self.to_tx.as_ref().is_some_and(|tx| tx.is_finalized())
    }

    pub fn same_with(&self, other: &BridgeLog) -> bool {
        self.user == other.user
            && self.from == other.from
            && self.to == other.to
            && self.icp_amount == other.icp_amount
            && self.from_tx.same_with(&other.from_tx)
    }
}

impl Storable for BridgeLogLocal {
    const BOUND: Bound = Bound::Unbounded;

    fn into_bytes(self) -> Vec<u8> {
        let mut buf = vec![];
        into_writer(&self, &mut buf).expect("failed to encode BridgeLogLocal data");
        buf
    }

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = vec![];
        into_writer(&self, &mut buf).expect("failed to encode BridgeLogLocal data");
        Cow::Owned(buf)
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        from_reader(&bytes[..]).expect("failed to decode BridgeLogLocal data")
    }
}

#[derive(Clone, CandidType, Default, Serialize, Deserialize)]
pub struct UserLogs {
    pub logs: BTreeSet<u64>,
}

impl Storable for UserLogs {
    const BOUND: Bound = Bound::Unbounded;

    fn into_bytes(self) -> Vec<u8> {
        let mut buf = vec![];
        into_writer(&self, &mut buf).expect("failed to encode UserLogs data");
        buf
    }

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = vec![];
        into_writer(&self, &mut buf).expect("failed to encode UserLogs data");
        Cow::Owned(buf)
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        from_reader(&bytes[..]).expect("failed to decode UserLogs data")
    }
}

const STATE_MEMORY_ID: MemoryId = MemoryId::new(0);
const USER_LOGS_MEMORY_ID: MemoryId = MemoryId::new(1);
const BRIDGE_LOGS_INDEX_MEMORY_ID: MemoryId = MemoryId::new(2);
const BRIDGE_LOGS_DATA_MEMORY_ID: MemoryId = MemoryId::new(3);

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::new());
    static HTTP_TREE: RefCell<HttpCertificationTree> = RefCell::new(HttpCertificationTree::default());

    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static STATE_STORE: RefCell<StableCell<Vec<u8>, Memory>> = RefCell::new(
        StableCell::init(
            MEMORY_MANAGER.with_borrow(|m| m.get(STATE_MEMORY_ID)),
            Vec::new()
        )
    );

    static USER_LOGS: RefCell<StableBTreeMap<Principal, UserLogs, Memory>> = RefCell::new(
        StableBTreeMap::init(
            MEMORY_MANAGER.with_borrow(|m| m.get(USER_LOGS_MEMORY_ID)),
        )
    );

    static BRIDGE_LOGS: RefCell<StableLog<BridgeLogLocal, Memory, Memory>> = RefCell::new(
        StableLog::init(
            MEMORY_MANAGER.with_borrow(|m| m.get(BRIDGE_LOGS_INDEX_MEMORY_ID)),
            MEMORY_MANAGER.with_borrow(|m| m.get(BRIDGE_LOGS_DATA_MEMORY_ID)),
        )
    );
}

pub mod state {
    use super::*;

    use lazy_static::lazy_static;
    use once_cell::sync::Lazy;

    lazy_static! {
        pub static ref DEFAULT_EXPR_PATH: HttpCertificationPath<'static> =
            HttpCertificationPath::wildcard("");
        pub static ref DEFAULT_CERTIFICATION: HttpCertification = HttpCertification::skip();
        pub static ref DEFAULT_CEL_EXPR: String =
            create_cel_expr(&DefaultCelBuilder::skip_certification());
    }

    pub static DEFAULT_CERT_ENTRY: Lazy<HttpCertificationTreeEntry> =
        Lazy::new(|| HttpCertificationTreeEntry::new(&*DEFAULT_EXPR_PATH, *DEFAULT_CERTIFICATION));

    pub async fn init_public_key() {
        let key_name = STATE.with_borrow(|r| r.key_name.clone());
        match ecdsa_public_key(key_name, vec![]).await {
            Ok(root_pk) => {
                STATE.with_borrow_mut(|s| {
                    let self_pk =
                        derive_public_key(&root_pk, vec![s.icp_address.as_slice().to_vec()])
                            .expect("derive_public_key failed");
                    s.ecdsa_public_key = root_pk;
                    s.evm_address = pubkey_bytes_to_address(&self_pk.public_key);
                });
            }
            Err(err) => {
                ic_cdk::api::debug_print(format!("failed to retrieve ECDSA public key: {err}"));
            }
        }
    }

    pub fn with<R>(f: impl FnOnce(&State) -> R) -> R {
        STATE.with_borrow(f)
    }

    pub fn with_mut<R>(f: impl FnOnce(&mut State) -> R) -> R {
        STATE.with_borrow_mut(f)
    }

    pub fn http_tree_with<R>(f: impl FnOnce(&HttpCertificationTree) -> R) -> R {
        HTTP_TREE.with(|r| f(&r.borrow()))
    }

    pub fn init_http_certified_data() {
        HTTP_TREE.with(|r| {
            let mut tree = r.borrow_mut();
            tree.insert(&DEFAULT_CERT_ENTRY);
            ic_cdk::api::certified_data_set(tree.root_hash())
        });
    }

    pub fn load() {
        STATE_STORE.with_borrow(|r| {
            STATE.with_borrow_mut(|h| {
                let bytes = r.get();
                if bytes.is_empty() {
                    return;
                }
                let v: State = from_reader(&bytes[..]).expect("failed to decode STATE_STORE data");
                *h = v;
            });
        });
    }

    pub fn save() {
        STATE.with_borrow(|h| {
            STATE_STORE.with_borrow_mut(|r| {
                let mut buf = vec![];
                into_writer(h, &mut buf).expect("failed to encode STATE_STORE data");
                r.set(buf);
            });
        });
    }

    pub fn info() -> StateInfo {
        let mut info = STATE.with_borrow(|s| StateInfo::from(s));
        info.total_bridge_count = BRIDGE_LOGS.with_borrow(|r| r.len());
        info
    }

    pub fn evm_address(user: &Principal) -> Address {
        STATE.with_borrow(|s| {
            let pk = derive_public_key(&s.ecdsa_public_key, vec![user.as_slice().to_vec()])
                .expect("derive_public_key failed");
            pubkey_bytes_to_address(&pk.public_key)
        })
    }

    pub fn evm_client(chain: &str) -> EvmClient<DefaultHttpOutcall> {
        STATE.with_borrow(|s| {
            s.evm_providers
                .get(chain)
                .map(|(max_confirmations, providers)| {
                    EvmClient::new(
                        providers.clone(),
                        *max_confirmations,
                        None,
                        DefaultHttpOutcall::new(s.icp_address),
                    )
                })
                .unwrap_or_else(|| {
                    EvmClient::new(vec![], 1, None, DefaultHttpOutcall::new(s.icp_address))
                })
        })
    }

    pub async fn evm_sign(user: &Principal, message_hash: Vec<u8>) -> Result<Vec<u8>, String> {
        let key_name = STATE.with_borrow(|s| {
            if !s.sub_bridges.contains(user) {
                Err("user is not authorized to sign".to_string())
            } else {
                Ok(s.key_name.clone())
            }
        })?;

        let cycles = cost_sign_with_ecdsa(key_name.clone())?;
        let received = ic_cdk::api::msg_cycles_accept(cycles);
        if received < cycles {
            return Err(format!(
                "insufficient cycles: required {}, accepted {}",
                cycles, received
            ));
        }

        let derivation_path = vec![user.as_slice().to_vec()];
        sign_with_ecdsa(key_name, derivation_path, message_hash).await
    }

    pub async fn bridge(
        from_chain: String,
        to_chain: String,
        icp_amount: u128,
        to_addr: Option<String>,
        user: Principal,
        now_ms: u64,
    ) -> Result<BridgeTx, String> {
        if from_chain == to_chain {
            return Err("from_chain and to_chain cannot be the same".to_string());
        }

        let (from, to, token_ledger, token_bridge_fee) = STATE.with_borrow(|s| {
            if s.error_rounds >= MAX_ERROR_ROUNDS {
                return Err("the bridge is temporarily disabled due to errors, please contact the administrator".to_string());
            }

            for log in s.pending.iter() {
                if let Some(err) = &log.error
                    && (err.starts_with(from_chain.as_str()) || err.starts_with(to_chain.as_str()))
                {
                    return Err(format!(
                        "there is a pending bridging task with error, please retry later:\n{}",
                        err
                    ));
                }
            }
            if icp_amount < s.min_threshold_to_bridge {
                return Err(format!(
                    "amount {} is below the minimum threshold to bridge {}",
                    icp_amount, s.min_threshold_to_bridge
                ));
            }
            let from = if from_chain == "ICP" {
                BridgeTarget::Icp
            } else {
                if !s.evm_token_contracts.contains_key(&from_chain) {
                    return Err(format!(
                        "from_chain {} not found or not supported",
                        from_chain
                    ));
                }
                BridgeTarget::Evm(from_chain)
            };
            let to = if to_chain == "ICP" {
                if let Some(to_addr) = &to_addr {
                    let _ = Principal::from_text(to_addr)
                        .map_err(|_| format!("invalid ICP address {to_addr}"))?;
                }
                BridgeTarget::Icp
            } else {
                if !s.evm_token_contracts.contains_key(&to_chain) {
                    return Err(format!("to_chain {} not found or not supported", to_chain));
                }
                if let Some(to_addr) = &to_addr {
                    let _ = to_addr
                        .parse::<Address>()
                        .map_err(|_| format!("invalid EVM address: {}", to_addr))?;
                }

                BridgeTarget::Evm(to_chain)
            };

            for log in s.pending.iter() {
                if log.user == user
                    && log.from == from
                    && matches!(log.from_tx, BridgeTx::Evm(false, _))
                {
                    return Err(format!(
                        "there is already a pending bridging task from {:?} for user {:?}",
                        log.from, log.user
                    ));
                }
            }

            Ok((from, to, s.token_ledger, s.token_bridge_fee))
        })?;

        let from_tx = match &from {
            BridgeTarget::Icp => from_icp(token_ledger, user, icp_amount).await?,
            BridgeTarget::Evm(chain) => from_evm(chain, user, icp_amount, now_ms).await?,
        };

        let delay = if from == BridgeTarget::Icp { 0 } else { 5 };
        let round = STATE.with_borrow_mut(|s| {
            s.pending.push_back(BridgeLog {
                id: None,
                user,
                from,
                to,
                icp_amount,
                fee: token_bridge_fee,
                from_tx: from_tx.clone(),
                to_tx: None,
                to_addr,
                created_at: now_ms,
                finalized_at: 0,
                error: None,
            });
            s.finalize_bridging_round.0
        });

        ic_cdk_timers::set_timer(Duration::from_secs(delay), finalize_bridging(round));

        Ok(from_tx)
    }

    pub fn my_bridge_log(user: Principal, from_tx: BridgeTx) -> Option<BridgeLog> {
        let mut log = STATE.with_borrow(|s| {
            s.pending
                .iter()
                .find(|item| item.user == user && item.from_tx == from_tx)
                .cloned()
        });

        if log.is_none() {
            log = USER_LOGS.with_borrow(|r| {
                let item = r.get(&user).unwrap_or_default();
                if item.logs.is_empty() {
                    return None;
                }
                let ids = item.logs.iter().rev().cloned().collect::<Vec<u64>>();

                if ids.is_empty() {
                    return None;
                }

                BRIDGE_LOGS.with_borrow(|log_store| {
                    for id in ids {
                        if let Some(mut log) = log_store.get(id)
                            && log.from_tx == from_tx
                        {
                            log.id = Some(id);
                            return Some(log.into());
                        }
                    }
                    None
                })
            });
        }

        log
    }

    pub fn user_logs(user: Principal, take: usize, prev: Option<u64>) -> Vec<BridgeLog> {
        USER_LOGS.with_borrow(|r| {
            let item = r.get(&user).unwrap_or_default();
            if item.logs.is_empty() {
                return vec![];
            }
            let ids = item
                .logs
                .range(..prev.unwrap_or(u64::MAX))
                .rev()
                .take(take)
                .cloned()
                .collect::<Vec<u64>>();

            if ids.is_empty() {
                return vec![];
            }

            BRIDGE_LOGS.with_borrow(|log_store| {
                let mut logs: Vec<BridgeLog> = Vec::with_capacity(ids.len());
                for id in ids {
                    if let Some(mut log) = log_store.get(id) {
                        log.id = Some(id);
                        logs.push(log.into());
                    }
                }
                logs
            })
        })
    }

    pub fn logs(take: usize, prev: Option<u64>) -> Vec<BridgeLog> {
        BRIDGE_LOGS.with_borrow(|log_store| {
            let max_id = log_store.len();
            let mut idx = prev.unwrap_or(max_id).min(max_id);
            let mut logs: Vec<BridgeLog> = Vec::with_capacity(take);
            while idx > 0 && logs.len() < take {
                idx -= 1;
                if let Some(mut log) = log_store.get(idx) {
                    log.id = Some(idx);
                    logs.push(log.into());
                }
            }
            logs
        })
    }

    pub async fn finalize_bridging(round: u64) {
        let tasks = STATE.with_borrow_mut(|s| {
            if s.finalize_bridging_round.1 || round < s.finalize_bridging_round.0 {
                // already running or old round
                return None;
            }

            if s.pending.is_empty() {
                return None;
            }

            s.finalize_bridging_round.1 = true;
            // take up to 3 pending tasks to process in parallel
            let mut tasks = Vec::with_capacity(3);
            // 针对 EVM 出口，按链互斥，避免同一 from 地址的 nonce 冲突
            let mut evm_outgoing_locked: HashSet<String> = HashSet::new();
            for task in s.pending.iter() {
                if let BridgeTarget::Evm(chain) = &task.to
                    && !evm_outgoing_locked.insert(chain.clone())
                {
                    // 已有同链任务在本轮处理，跳过以避免 nonce 冲突
                    continue;
                }

                tasks.push(task.clone());
                if tasks.len() == 3 {
                    break;
                }
            }
            Some(tasks)
        });

        if let Some(tasks) = tasks {
            let tasks = try_finalize_tasks(tasks).await;
            let now_ms = ic_cdk::api::time() / 1_000_000;
            let next = STATE.with_borrow_mut(|s| {
                let mut has_error = false;
                for task in tasks {
                    has_error = has_error || task.error.is_some();
                    for t in s.pending.iter_mut() {
                        if t.same_with(&task) {
                            *t = task;
                            if t.to_tx.as_ref().is_some_and(|tx| tx.is_finalized()) {
                                t.error = None;
                                t.finalized_at = now_ms;
                                s.total_bridged_tokens =
                                    s.total_bridged_tokens.saturating_add(t.icp_amount);
                                s.total_collected_fees =
                                    s.total_collected_fees.saturating_add(t.fee);

                                let idx = BRIDGE_LOGS
                                    .with_borrow_mut(|r| r.append(&t.clone().into()))
                                    .expect("failed to append to BRIDGE_LOGS");
                                USER_LOGS.with_borrow_mut(|r| {
                                    let mut logs = r.get(&t.user).unwrap_or_default();
                                    logs.logs.insert(idx);
                                    r.insert(t.user, logs);
                                });
                            }
                            break;
                        }
                    }
                }

                s.pending.retain(|t| !t.is_finalized());
                s.finalize_bridging_round = (s.finalize_bridging_round.0 + 1, false);

                if s.pending.is_empty() {
                    None
                } else if has_error {
                    s.error_rounds += 1;
                    if s.error_rounds >= MAX_ERROR_ROUNDS {
                        None
                    } else {
                        Some((5 * s.error_rounds, s.finalize_bridging_round.0))
                    }
                } else {
                    s.error_rounds = 0;
                    Some((1, s.finalize_bridging_round.0))
                }
            });

            if let Some((delay, round)) = next {
                ic_cdk_timers::set_timer(Duration::from_secs(delay), finalize_bridging(round));
            }
        }
    }

    async fn try_finalize_tasks(tasks: Vec<BridgeLog>) -> Vec<BridgeLog> {
        let now_ms = ic_cdk::api::time() / 1_000_000;
        futures::future::join_all(tasks.into_iter().map(|task| process_task(task, now_ms))).await
    }

    async fn process_task(mut task: BridgeLog, now_ms: u64) -> BridgeLog {
        let rt = async {
            let from_finalized = match (&task.from, &mut task.from_tx) {
                (BridgeTarget::Evm(chain), BridgeTx::Evm(finalized, tx_hash)) if !*finalized => {
                    let tx_hash: TxHash = (**tx_hash).into();
                    let from_finalized = check_evm_tx_finalized(chain, &tx_hash, now_ms).await?;
                    if from_finalized {
                        *finalized = true;
                    }
                    from_finalized
                }
                _ => true,
            };

            if from_finalized {
                match (&task.to, &mut task.to_tx) {
                    (BridgeTarget::Icp, None) => {
                        let token_ledger = STATE.with_borrow(|s| s.token_ledger);
                        let to_addr = if let Some(addr) = &task.to_addr {
                            Principal::from_text(addr)
                                .map_err(|_| format!("ICP: invalid to_addr principal: {}", addr))?
                        } else {
                            task.user
                        };
                        let to_tx = to_icp(
                            token_ledger,
                            to_addr,
                            task.icp_amount.saturating_sub(task.fee),
                        )
                        .await?;
                        task.to_tx = Some(to_tx);
                    }
                    (BridgeTarget::Evm(chain), None) => {
                        let to_addr = if let Some(addr) = &task.to_addr {
                            addr.parse::<Address>()
                                .map_err(|_| format!("EVM: invalid to_addr address: {}", addr))?
                        } else {
                            state::evm_address(&task.user)
                        };
                        let to_tx = to_evm(
                            chain,
                            to_addr,
                            task.icp_amount.saturating_sub(task.fee),
                            now_ms,
                        )
                        .await?;
                        task.to_tx = Some(to_tx);
                    }
                    (BridgeTarget::Evm(chain), Some(BridgeTx::Evm(finalized, tx_hash)))
                        if !*finalized =>
                    {
                        let tx_hash: TxHash = (**tx_hash).into();
                        let to_finalized = check_evm_tx_finalized(chain, &tx_hash, now_ms).await?;
                        if to_finalized {
                            *finalized = true;
                        }
                    }
                    _ => {}
                }
            }

            Ok::<(), String>(())
        }
        .await;

        task.error = rt.err();
        if let Some(err) = &task.error {
            ic_cdk::api::debug_print(format!("finalize_tasks failed: {err}"));
        }

        task
    }

    async fn from_icp(
        token_ledger: Principal,
        user: Principal,
        icp_amount: u128,
    ) -> Result<BridgeTx, String> {
        let res: Result<Nat, TransferFromError> = call(
            token_ledger,
            "icrc2_transfer_from",
            (TransferFromArgs {
                spender_subaccount: None,
                from: Account {
                    owner: user,
                    subaccount: None,
                },
                to: Account {
                    owner: ic_cdk::api::canister_self(),
                    subaccount: None,
                },
                fee: None,
                created_at_time: None,
                memo: None,
                amount: icp_amount.into(),
            },),
            0,
        )
        .await?;
        let res = res
            .map_err(|err| format!("ICP: failed to transfer token from user, error: {:?}", err))?;
        let idx = res
            .0
            .to_u64()
            .ok_or_else(|| "ICP: block height too large".to_string())?;
        Ok(BridgeTx::Icp(true, idx))
    }

    pub async fn to_icp(
        token_ledger: Principal,
        to_addr: Principal,
        icp_amount: u128,
    ) -> Result<BridgeTx, String> {
        let res: Result<Nat, TransferFromError> = call(
            token_ledger,
            "icrc1_transfer",
            (TransferArg {
                from_subaccount: None,
                to: Account {
                    owner: to_addr,
                    subaccount: None,
                },
                fee: None,
                created_at_time: None,
                memo: None,
                amount: icp_amount.into(),
            },),
            0,
        )
        .await?;
        let res =
            res.map_err(|err| format!("ICP: failed to transfer token to user, error: {:?}", err))?;
        let idx = res
            .0
            .to_u64()
            .ok_or_else(|| "ICP: block height too large".to_string())?;
        Ok(BridgeTx::Icp(true, idx))
    }

    async fn from_evm(
        chain: &str,
        user: Principal,
        icp_amount: u128,
        now_ms: u64,
    ) -> Result<BridgeTx, String> {
        let to_addr = STATE.with_borrow(|s| s.evm_address);
        let (client, signed_tx) =
            build_erc20_transfer_tx(chain, &user, &to_addr, icp_amount, now_ms)
                .await
                .map_err(|err| format!("{chain}: {err}"))?;
        let tx_hash: [u8; 32] = (*signed_tx.hash()).into();
        let data = signed_tx.encoded_2718();

        let _ = client
            .send_raw_transaction(now_ms, Bytes::from(data).to_string())
            .await
            .map_err(|err| format!("{chain}: {err}"))?;
        Ok(BridgeTx::Evm(false, tx_hash.into()))
    }

    async fn to_evm(
        chain: &str,
        to_addr: Address,
        icp_amount: u128,
        now_ms: u64,
    ) -> Result<BridgeTx, String> {
        // let to_addr = evm_address(&user);
        let (client, signed_tx) = build_erc20_transfer_tx(
            chain,
            &ic_cdk::api::canister_self(),
            &to_addr,
            icp_amount,
            now_ms,
        )
        .await
        .map_err(|err| format!("{chain}: {err}"))?;

        let tx_hash: [u8; 32] = (*signed_tx.hash()).into();
        let data = signed_tx.encoded_2718();

        let _ = client
            .send_raw_transaction(now_ms, Bytes::from(data).to_string())
            .await
            .map_err(|err| format!("{chain}: {err}"))?;
        Ok(BridgeTx::Evm(false, tx_hash.into()))
    }

    pub async fn build_erc20_transfer_tx(
        chain: &str,
        from: &Principal,
        to_addr: &Address,
        icp_amount: u128,
        now_ms: u64,
    ) -> Result<(EvmClient<DefaultHttpOutcall>, Signed<TxEip1559>), String> {
        let (key_name, from_pk, mut tx, gas_updated_at) = STATE.with_borrow(|s| {
            let (contract, decimals, chain_id) = s
                .evm_token_contracts
                .get(chain)
                .cloned()
                .ok_or_else(|| format!("chain {chain} not found"))?;

            let value = convert_amount(icp_amount, s.token_decimals, decimals)?;
            let from_pk = derive_public_key(&s.ecdsa_public_key, vec![from.as_slice().to_vec()])
                .map_err(|_e| format!("{chain}: derive_public_key failed"))?;

            let input = encode_erc20_transfer(to_addr, value);
            let (gas_updated_at, gas_price, max_priority_fee_per_gas) =
                s.evm_latest_gas.get(chain).cloned().unwrap_or_default();
            let max_priority_fee_per_gas = max_priority_fee_per_gas + max_priority_fee_per_gas / 5;
            Ok::<_, String>((
                s.key_name.clone(),
                from_pk,
                TxEip1559 {
                    chain_id,
                    nonce: 0u64,
                    gas_limit: 84_000u64, // sample: ~53,696
                    max_fee_per_gas: gas_price * 2 + max_priority_fee_per_gas,
                    max_priority_fee_per_gas,
                    to: contract.into(),
                    input: input.into(),
                    ..Default::default()
                },
                gas_updated_at,
            ))
        })?;

        let from_addr = pubkey_bytes_to_address(&from_pk.public_key);
        if &from_addr == to_addr {
            return Err("from and to cannot be the same".to_string());
        }

        let client = evm_client(chain);
        if gas_updated_at + 120_000 >= now_ms {
            let nonce = client.get_transaction_count(now_ms, &from_addr).await?;
            tx.nonce = nonce;
        } else {
            let (nonce, gas_price, max_priority_fee_per_gas) = futures::future::try_join3(
                client.get_transaction_count(now_ms, &from_addr),
                client.gas_price(now_ms),
                client.max_priority_fee_per_gas(now_ms),
            )
            .await?;
            tx.nonce = nonce;
            tx.max_priority_fee_per_gas = max_priority_fee_per_gas + max_priority_fee_per_gas / 5;
            tx.max_fee_per_gas = gas_price * 2 + tx.max_priority_fee_per_gas;
            STATE.with_borrow_mut(|s| {
                s.evm_latest_gas.insert(
                    chain.to_string(),
                    (now_ms, gas_price, max_priority_fee_per_gas),
                );
            })
        }

        let msg_hash = tx.signature_hash();
        let sig =
            sign_with_ecdsa(key_name, vec![from.as_slice().to_vec()], msg_hash.to_vec()).await?;
        let signature = Signature::new(
            U256::from_be_slice(&sig[0..32]),  // r
            U256::from_be_slice(&sig[32..64]), // s
            y_parity(msg_hash.as_slice(), &sig, from_pk.public_key.as_slice())?,
        );

        let signed_tx = tx.into_signed(signature);
        Ok((client, signed_tx))
    }

    pub async fn build_evm_transfer_tx(
        chain: &str,
        from: &Principal,
        to_addr: &Address,
        amount: u128,
        now_ms: u64,
    ) -> Result<(EvmClient<DefaultHttpOutcall>, Signed<TxEip1559>), String> {
        let (key_name, from_pk, mut tx, gas_updated_at) = STATE.with_borrow(|s| {
            let chain_id = s
                .evm_token_contracts
                .get(chain)
                .map(|(_, _, chain_id)| *chain_id)
                .ok_or_else(|| "chain not found".to_string())?;

            let from_pk = derive_public_key(&s.ecdsa_public_key, vec![from.as_slice().to_vec()])
                .expect("derive_public_key failed");
            let (gas_updated_at, gas_price, max_priority_fee_per_gas) =
                s.evm_latest_gas.get(chain).cloned().unwrap_or_default();
            let max_priority_fee_per_gas = max_priority_fee_per_gas + max_priority_fee_per_gas / 5;
            Ok::<_, String>((
                s.key_name.clone(),
                from_pk,
                TxEip1559 {
                    chain_id,
                    nonce: 0u64,
                    gas_limit: 32_000u64, // sample: ~21,000
                    max_fee_per_gas: gas_price * 2 + max_priority_fee_per_gas,
                    max_priority_fee_per_gas,
                    to: (*to_addr).into(),
                    value: amount
                        .try_into()
                        .map_err(|_| "invalid amount".to_string())?,
                    ..Default::default()
                },
                gas_updated_at,
            ))
        })?;

        let from_addr = pubkey_bytes_to_address(&from_pk.public_key);
        if &from_addr == to_addr {
            return Err("from and to cannot be the same".to_string());
        }

        let client = evm_client(chain);
        if gas_updated_at + 120_000 >= now_ms {
            let nonce = client.get_transaction_count(now_ms, &from_addr).await?;
            tx.nonce = nonce;
        } else {
            let (nonce, gas_price, max_priority_fee_per_gas) = futures::future::try_join3(
                client.get_transaction_count(now_ms, &from_addr),
                client.gas_price(now_ms),
                client.max_priority_fee_per_gas(now_ms),
            )
            .await?;
            tx.nonce = nonce;
            tx.max_priority_fee_per_gas = max_priority_fee_per_gas + max_priority_fee_per_gas / 5;
            tx.max_fee_per_gas = gas_price * 2 + tx.max_priority_fee_per_gas;
            STATE.with_borrow_mut(|s| {
                s.evm_latest_gas.insert(
                    chain.to_string(),
                    (now_ms, gas_price, max_priority_fee_per_gas),
                );
            })
        }

        let msg_hash = tx.signature_hash();
        let sig =
            sign_with_ecdsa(key_name, vec![from.as_slice().to_vec()], msg_hash.to_vec()).await?;
        let signature = Signature::new(
            U256::from_be_slice(&sig[0..32]),  // r
            U256::from_be_slice(&sig[32..64]), // s
            y_parity(msg_hash.as_slice(), &sig, from_pk.public_key.as_slice())?,
        );

        let signed_tx = tx.into_signed(signature);
        Ok((client, signed_tx))
    }

    async fn check_evm_tx_finalized(
        chain: &str,
        tx_hash: &TxHash,
        now_ms: u64,
    ) -> Result<bool, String> {
        let client = evm_client(chain);
        let (latest_block, receipt) = futures::future::join(
            client.block_number(now_ms),
            client.get_transaction_receipt(now_ms, tx_hash),
        )
        .await;
        match (latest_block, receipt) {
            (Ok(latest), Ok(Some(receipt))) => {
                if let Some(block_number) = receipt.block_number
                    && *tx_hash == receipt.transaction_hash
                    && latest >= block_number + client.max_confirmations
                {
                    // We don't need to check the logs here.
                    // log.address == 代币合约地址
                    // log.topics[0] == keccak256("Transfer(address,address,uint256)") = 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef
                    // log.topics[1] == from 地址（32 字节左填充）
                    // log.topics[2] == to 地址（32 字节左填充）
                    // log.data 为 uint256 的转账数量（ABI 编码）
                    return Ok(receipt.status());
                }
                Ok(false)
            }
            (Err(err), _) | (_, Err(err)) => Err(format!(
                "{chain}: failed to check evm tx finalized, error: {err}"
            )),
            _ => Ok(false),
        }
    }
}

pub fn pubkey_bytes_to_address(pubkey_bytes: &[u8]) -> Address {
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    let key = k256::PublicKey::from_sec1_bytes(pubkey_bytes)
        .expect("failed to parse the public key as SEC1");
    let point = key.to_encoded_point(false);
    Address::from_raw_public_key(&point.as_bytes()[1..])
}

fn y_parity(prehash: &[u8], sig: &[u8], pubkey: &[u8]) -> Result<bool, String> {
    use alloy_signer::k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

    let orig_key = VerifyingKey::from_sec1_bytes(pubkey).map_err(format_error)?;
    let signature = Signature::try_from(sig).map_err(format_error)?;
    for parity in [0u8, 1] {
        let recid = RecoveryId::try_from(parity).map_err(format_error)?;
        let recovered_key = match VerifyingKey::recover_from_prehash(prehash, &signature, recid) {
            Ok(k) => k,
            Err(_) => continue, // 尝试另一 parity
        };
        if recovered_key == orig_key {
            return Ok(parity == 1);
        }
    }

    Err(format!(
        "failed to recover the parity bit from a signature; sig: {}, pubkey: {}",
        hex::encode(sig),
        hex::encode(pubkey)
    ))
}
