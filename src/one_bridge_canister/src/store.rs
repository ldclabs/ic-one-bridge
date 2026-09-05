use alloy_consensus::{SignableTransaction, Signed, TxEip1559};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxHash, U256, hex};
use candid::{CandidType, Nat, Principal};
use ic_auth_types::{cbor_from_slice, cbor_into_vec};
use ic_http_certification::{
    HttpCertification, HttpCertificationPath, HttpCertificationTree, HttpCertificationTreeEntry,
    cel::{DefaultCelBuilder, create_cel_expr},
};
use ic_stable_structures::{
    DefaultMemoryImpl, Memory as _, StableBTreeMap, StableCell, StableLog, Storable,
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
    storable::Bound,
};
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{Memo, TransferArg, TransferError},
    },
    icrc2::transfer_from::{TransferFromArgs, TransferFromError},
};
use serde::{Deserialize, Serialize};
use serde_bytes::ByteArray;
use solana_instruction::Instruction;
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp,
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    future::Future,
    rc::Rc,
    sync::LazyLock,
    time::Duration,
};

use crate::{
    ecdsa::{cost_sign_with_ecdsa, derive_public_key, ecdsa_public_key, sign_with_ecdsa},
    evm::{EvmClient, encode_erc20_transfer},
    helper::{bridge_amount_after_fee, call, convert_amount, format_error},
    outcall::{DefaultHttpOutcall, HttpOutcall},
    schnorr::{derive_schnorr_public_key, schnorr_public_key, sign_with_schnorr},
    svm::{
        Message, Pubkey, Signature as SvmSignature, SignatureStatus, SvmClient, Transaction,
        create_associated_token_account_idempotent, get_associated_token_address,
        system_transfer_instruction, transfer_checked_instruction,
    },
    types::PublicKeyOutput,
};

type Memory = VirtualMemory<DefaultMemoryImpl>;

const MAX_ERROR_ROUNDS: u64 = 42;

/// How far back through a user's archive `my_bridge_log` looks.
///
/// There is no index from an incoming transaction to the log that recorded it,
/// so the lookup is a scan, and every step of it decodes a record out of stable
/// memory. A user asks about a transaction right after making it, when it is
/// still pending or sits at the very front of the archive, so the cap costs a
/// real lookup nothing — it only stops a query for a transaction that was never
/// there from reading a whole history to say so.
const MAX_LOG_LOOKBACK: usize = 100;

/// A finalization round that traps can never clear `finalize_bridging_round.1`,
/// which would stop finalization forever. A lock held for longer than any round
/// can plausibly take is therefore treated as stale and taken over.
const FINALIZE_BRIDGING_LOCK_TIMEOUT_MS: u64 = 10 * 60 * 1000;

/// Gas limit of an ERC-20 `transfer` unless the token sets its own: an
/// OpenZeppelin transfer uses ~54k, and the headroom is for a token with a
/// little more logic in it.
const DEFAULT_ERC20_GAS_LIMIT: u64 = 84_000;

/// Gas limit of a native transfer. A plain one costs exactly 21k, and the
/// headroom is for a recipient contract with a small receive hook.
const NATIVE_TRANSFER_GAS_LIMIT: u64 = 32_000;

/// Sanity bounds on a configured ERC-20 gas limit: below the 21k intrinsic
/// cost no transaction is valid, and above a million the value is a typo.
const ERC20_GAS_LIMIT_RANGE: std::ops::RangeInclusive<u64> = 21_000..=1_000_000;

fn default_erc20_gas_limit() -> u64 {
    DEFAULT_ERC20_GAS_LIMIT
}

pub fn validate_erc20_gas_limit(gas_limit: u64) -> Result<(), String> {
    if ERC20_GAS_LIMIT_RANGE.contains(&gas_limit) {
        Ok(())
    } else {
        Err(format!(
            "erc20_gas_limit {gas_limit} must be between {} and {}",
            ERC20_GAS_LIMIT_RANGE.start(),
            ERC20_GAS_LIMIT_RANGE.end()
        ))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    pub key_name: String,
    pub icp_address: Principal,
    pub evm_address: Address,
    #[serde(default)]
    pub svm_address: Pubkey,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_logo: String,
    pub token_ledger: Principal,
    #[serde(default)]
    pub token_bridge_fee: u128, // with the same decimals as token
    pub min_threshold_to_bridge: u128,
    /// Gas limit of this token's ERC-20 `transfer`, on every EVM chain.
    #[serde(default = "default_erc20_gas_limit")]
    pub erc20_gas_limit: u64,
    // chain_name => (contract_address, decimals, chain_id)
    pub evm_token_contracts: HashMap<String, (Address, u8, u64)>,
    // chain_name => (gas_updated_at, gas_price, max_priority_fee_per_gas)
    pub evm_latest_gas: HashMap<String, (u64, u128, u128)>,
    // chain_name => (max_confirmations, [provider_url])
    pub evm_providers: HashMap<String, (u64, Vec<String>)>,
    // (token_address, decimals, token_program)
    #[serde(default)]
    pub svm_token_address: (Pubkey, u8, Pubkey),
    #[serde(default)]
    pub svm_providers: Vec<String>,
    pub ecdsa_public_key: PublicKeyOutput,
    #[serde(default)]
    pub ed25519_public_key: PublicKeyOutput,
    pub governance_canister: Option<Principal>,
    pub pending: VecDeque<BridgeLog>,
    // (round, running)
    pub finalize_bridging_round: (u64, bool),
    // when the running round took the lock, in ms; 0 when no round is running
    #[serde(default)]
    pub finalize_bridging_started_at: u64,
    // consecutive finalization rounds in which no pending task advanced
    #[serde(default)]
    pub idle_rounds: u64,
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
    pub svm_address: String,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_logo: String,
    pub token_ledger: Principal,
    pub token_bridge_fee: u128,
    pub min_threshold_to_bridge: u128,
    pub erc20_gas_limit: u64,
    pub evm_token_contracts: HashMap<String, (String, u8, u64)>,
    pub evm_latest_gas: HashMap<String, (u64, u128, u128)>,
    pub evm_providers: HashMap<String, (u64, Vec<String>)>,
    pub svm_token_address: (String, u8, String),
    pub svm_providers: Vec<String>,
    pub finalize_bridging_round: (u64, bool),
    pub total_bridged_tokens: u128,
    pub total_collected_fees: u128,
    pub total_withdrawn_fees: u128,
    pub total_bridge_count: u64,
    pub sub_bridges: BTreeSet<Principal>,
    pub error_rounds: u64,
    pub governance_canister: Option<Principal>,
}

impl StateInfo {
    fn new(s: &State, total_bridge_count: u64) -> Self {
        Self {
            key_name: s.key_name.clone(),
            icp_address: s.icp_address,
            evm_address: s.evm_address.to_string(),
            svm_address: s.svm_address.to_string(),
            token_name: s.token_name.clone(),
            token_symbol: s.token_symbol.clone(),
            token_decimals: s.token_decimals,
            token_logo: s.token_logo.clone(),
            token_ledger: s.token_ledger,
            token_bridge_fee: s.token_bridge_fee,
            min_threshold_to_bridge: s.min_threshold_to_bridge,
            erc20_gas_limit: s.erc20_gas_limit,
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
            svm_token_address: (
                s.svm_token_address.0.to_string(),
                s.svm_token_address.1,
                s.svm_token_address.2.to_string(),
            ),
            svm_providers: s.svm_providers.clone(),
            finalize_bridging_round: s.finalize_bridging_round,
            total_bridged_tokens: s.total_bridged_tokens,
            total_collected_fees: s.total_collected_fees,
            total_withdrawn_fees: s.total_withdrawn_fees,
            total_bridge_count,
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
            svm_address: Pubkey::default(), // 11111111111111111111111111111111
            token_name: "ICPanda".to_string(),
            token_symbol: "PANDA".to_string(),
            token_decimals: 8,
            token_logo: "https://532er-faaaa-aaaaj-qncpa-cai.icp0.io/f/374?inline&filename=1734188626561.webp".to_string(),
            token_ledger: Principal::from_text("druyg-tyaaa-aaaaq-aactq-cai").unwrap(), // mainnet ledger
            token_bridge_fee: 0,
            min_threshold_to_bridge: 100_000_000, // 1 Token (8 decimals)
            erc20_gas_limit: DEFAULT_ERC20_GAS_LIMIT,
            evm_token_contracts: HashMap::new(),
            evm_providers: HashMap::new(),
            evm_latest_gas: HashMap::new(),
            svm_token_address: (Pubkey::default(), 0, Pubkey::default()),
            svm_providers: Vec::new(),
            ecdsa_public_key: PublicKeyOutput::default(),
            ed25519_public_key: PublicKeyOutput::default(),
            governance_canister: None,
            pending: VecDeque::new(),
            finalize_bridging_round: (0, false),
            finalize_bridging_started_at: 0,
            idle_rounds: 0,
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
    Sol,
    Evm(String), // chain_name
}

#[derive(Clone, CandidType, Serialize, Deserialize)]
pub enum BridgeTx {
    Icp(bool, u64),           // (finalized, block_height)
    Evm(bool, ByteArray<32>), // (finalized, tx_hash)
    Sol(bool, ByteArray<64>), // (finalized, tx_signature)
}

/// Two records of one transaction are the same transaction whether or not
/// either of them has seen it finalize, so equality ignores the flag.
impl cmp::PartialEq for BridgeTx {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BridgeTx::Icp(_, tx1), BridgeTx::Icp(_, tx2)) => tx1 == tx2,
            (BridgeTx::Evm(_, tx1), BridgeTx::Evm(_, tx2)) => tx1 == tx2,
            (BridgeTx::Sol(_, tx1), BridgeTx::Sol(_, tx2)) => tx1 == tx2,
            _ => false,
        }
    }
}

impl BridgeTarget {
    /// The name errors raised against this chain are prefixed with.
    pub fn name(&self) -> &str {
        match self {
            BridgeTarget::Icp => "ICP",
            BridgeTarget::Sol => "SOL",
            BridgeTarget::Evm(chain) => chain,
        }
    }
}

impl BridgeTx {
    pub fn is_finalized(&self) -> bool {
        match self {
            BridgeTx::Icp(finalized, _) => *finalized,
            BridgeTx::Evm(finalized, _) => *finalized,
            BridgeTx::Sol(finalized, _) => *finalized,
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
            && self.from_tx == other.from_tx
    }
}

impl Storable for BridgeLogLocal {
    const BOUND: Bound = Bound::Unbounded;

    fn into_bytes(self) -> Vec<u8> {
        cbor_into_vec(&self).expect("failed to encode BridgeLogLocal data")
    }

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(cbor_into_vec(self).expect("failed to encode BridgeLogLocal data"))
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        cbor_from_slice(&bytes).expect("failed to decode BridgeLogLocal data")
    }
}

/// A user's archive index in the layout that predates [`UserLogKey`]: all of
/// the user's log ids in one value, rewritten whole on every append. Only read
/// now, to copy it into the current layout.
#[derive(Clone, Default, Serialize, Deserialize)]
struct LegacyUserLogs {
    logs: BTreeSet<u64>,
}

impl Storable for LegacyUserLogs {
    const BOUND: Bound = Bound::Unbounded;

    fn into_bytes(self) -> Vec<u8> {
        cbor_into_vec(&self).expect("failed to encode LegacyUserLogs data")
    }

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(cbor_into_vec(self).expect("failed to encode LegacyUserLogs data"))
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        cbor_from_slice(&bytes).expect("failed to decode LegacyUserLogs data")
    }
}

/// Key of the per-user archive index: a user, then one of their log ids.
///
/// The map orders keys by their bytes, so the encoding leads with the
/// principal's length and pads the principal to its maximum length: every id of
/// one user is then contiguous and in id order, and no other principal's keys
/// fall in between, so a user's history is one range scan. Appending a log
/// inserts one fixed-size key instead of rewriting the user's whole id set.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UserLogKey([u8; UserLogKey::SIZE]);

impl UserLogKey {
    const SIZE: usize = 1 + Principal::MAX_LENGTH_IN_BYTES + 8;

    fn new(user: &Principal, log_id: u64) -> Self {
        let mut bytes = [0u8; Self::SIZE];
        let user = user.as_slice();
        bytes[0] = user.len() as u8;
        bytes[1..1 + user.len()].copy_from_slice(user);
        bytes[Self::SIZE - 8..].copy_from_slice(&log_id.to_be_bytes());
        Self(bytes)
    }

    fn log_id(&self) -> u64 {
        let mut id = [0u8; 8];
        id.copy_from_slice(&self.0[Self::SIZE - 8..]);
        u64::from_be_bytes(id)
    }
}

impl Storable for UserLogKey {
    const BOUND: Bound = Bound::Bounded {
        max_size: Self::SIZE as u32,
        is_fixed_size: true,
    };

    fn into_bytes(self) -> Vec<u8> {
        self.0.to_vec()
    }

    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(&self.0)
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        Self(
            bytes
                .as_ref()
                .try_into()
                .expect("UserLogKey has a fixed size"),
        )
    }
}

/// The ids of `user`'s archived logs below `before`, newest first, at most
/// `take` of them.
fn user_log_ids<M: ic_stable_structures::Memory>(
    index: &StableBTreeMap<UserLogKey, (), M>,
    user: &Principal,
    before: u64,
    take: usize,
) -> Vec<u64> {
    index
        .keys_range(UserLogKey::new(user, 0)..UserLogKey::new(user, before))
        .rev()
        .take(take)
        .map(|key| key.log_id())
        .collect()
}

/// Copies every id of the legacy per-user index into `index`, unless `index`
/// already holds something, and returns how many ids were copied.
fn copy_legacy_user_log_index<M: ic_stable_structures::Memory>(
    legacy: &StableBTreeMap<Principal, LegacyUserLogs, M>,
    index: &mut StableBTreeMap<UserLogKey, (), M>,
) -> u64 {
    if !index.is_empty() {
        return 0;
    }

    let mut copied = 0;
    for entry in legacy.iter() {
        let (user, logs) = entry.into_pair();
        for log_id in logs.logs {
            index.insert(UserLogKey::new(&user, log_id), ());
            copied += 1;
        }
    }
    copied
}

const STATE_MEMORY_ID: MemoryId = MemoryId::new(0);
const LEGACY_USER_LOGS_MEMORY_ID: MemoryId = MemoryId::new(1);
const BRIDGE_LOGS_INDEX_MEMORY_ID: MemoryId = MemoryId::new(2);
const BRIDGE_LOGS_DATA_MEMORY_ID: MemoryId = MemoryId::new(3);
const USER_LOG_INDEX_MEMORY_ID: MemoryId = MemoryId::new(4);

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::new());
    static HTTP_TREE: RefCell<HttpCertificationTree> = RefCell::new(HttpCertificationTree::default());
    static ACTIVE_BRIDGE_USERS: RefCell<BTreeSet<Principal>> = const { RefCell::new(BTreeSet::new()) };
    static FINALIZE_TIMER: RefCell<Option<ScheduledFinalize>> = const { RefCell::new(None) };
    static FINALIZE_RUN_GENERATION: Cell<u64> = const { Cell::new(0) };

    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));

    static STATE_STORE: RefCell<StableCell<Vec<u8>, Memory>> = RefCell::new(
        StableCell::init(
            MEMORY_MANAGER.with_borrow(|m| m.get(STATE_MEMORY_ID)),
            Vec::new()
        )
    );

    static USER_LOG_INDEX: RefCell<StableBTreeMap<UserLogKey, (), Memory>> = RefCell::new(
        StableBTreeMap::init(
            MEMORY_MANAGER.with_borrow(|m| m.get(USER_LOG_INDEX_MEMORY_ID)),
        )
    );

    static BRIDGE_LOGS: RefCell<StableLog<BridgeLogLocal, Memory, Memory>> = RefCell::new(
        StableLog::init(
            MEMORY_MANAGER.with_borrow(|m| m.get(BRIDGE_LOGS_INDEX_MEMORY_ID)),
            MEMORY_MANAGER.with_borrow(|m| m.get(BRIDGE_LOGS_DATA_MEMORY_ID)),
        )
    );
}

struct ScheduledFinalize {
    id: ic_cdk_timers::TimerId,
    deadline_ms: u64,
}

struct ActiveBridgeUserGuard(Principal);

impl Drop for ActiveBridgeUserGuard {
    fn drop(&mut self) {
        ACTIVE_BRIDGE_USERS.with_borrow_mut(|users| {
            users.remove(&self.0);
        });
    }
}

fn acquire_active_bridge_user(user: Principal) -> Result<ActiveBridgeUserGuard, String> {
    ACTIVE_BRIDGE_USERS.with_borrow_mut(|users| {
        if users.insert(user) {
            Ok(ActiveBridgeUserGuard(user))
        } else {
            Err("there is already a bridge request in progress for this user".to_string())
        }
    })
}

/// Whether a new finalization round may take the lock.
///
/// A round that trapped leaves `running` set forever, so a lock held for longer
/// than `FINALIZE_BRIDGING_LOCK_TIMEOUT_MS` is considered abandoned.
fn finalize_lock_available(running: bool, started_at: u64, now_ms: u64) -> bool {
    !running || now_ms.saturating_sub(started_at) >= FINALIZE_BRIDGING_LOCK_TIMEOUT_MS
}

fn next_finalize_run_generation() -> u64 {
    FINALIZE_RUN_GENERATION.with(|value| {
        let next = value.get().wrapping_add(1);
        value.set(next);
        next
    })
}

fn finalize_run_matches(expected: u64, current: u64, running: bool) -> bool {
    expected == current && running
}

fn finalize_run_is_current(expected: u64) -> bool {
    let current = FINALIZE_RUN_GENERATION.with(Cell::get);
    STATE.with_borrow(|state| {
        finalize_run_matches(expected, current, state.finalize_bridging_round.1)
    })
}

fn finalize_timer_deadline_ms(now_ms: u64, delay: Duration, running: bool, started_at: u64) -> u64 {
    let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
    let requested = now_ms.saturating_add(delay_ms);
    if running {
        requested.max(started_at.saturating_add(FINALIZE_BRIDGING_LOCK_TIMEOUT_MS))
    } else {
        requested
    }
}

/// The state of an EVM transaction the canister is waiting on.
enum EvmTxStatus {
    /// Not mined yet, or not yet confirmed deeply enough.
    Pending,
    /// Mined, succeeded, and confirmed.
    Confirmed,
    /// Mined and reverted. It will never finalize, and it moved no funds.
    Reverted,
}

/// What a finalization round decided to do with a pending task.
enum TaskOutcome {
    /// Keep working on it: write the updated task back into the queue.
    Retained(BridgeLog),
    /// Its incoming transfer provably failed on chain, so the bridge received
    /// nothing and owes nothing. Archive it and drop it from the queue, rather
    /// than leaving an error behind that blocks every other user of that chain.
    Abandoned(BridgeLog),
}

/// Result of atomically reserving the outgoing transaction slot of a pending
/// task before handing a signed transaction to an external provider.
enum PayoutClaim {
    /// This round filled the empty slot and is the only one allowed to broadcast
    /// the candidate transaction.
    Claimed,
    /// Another (possibly stale-overlapping) round filled the slot first. Reuse
    /// that transaction and never broadcast the candidate.
    Existing(BridgeTx),
    /// This round lost the stale-lock race before it could reserve the slot.
    RunSuperseded,
    /// The task was finalized or removed while this round was building its
    /// candidate transaction.
    TaskGone,
}

fn claim_payout_in(
    pending: &mut VecDeque<BridgeLog>,
    from_tx: &BridgeTx,
    candidate: &BridgeTx,
) -> PayoutClaim {
    let Some(task) = pending.iter_mut().find(|task| task.from_tx == *from_tx) else {
        return PayoutClaim::TaskGone;
    };

    match &task.to_tx {
        Some(existing) => PayoutClaim::Existing(existing.clone()),
        None => {
            task.to_tx = Some(candidate.clone());
            PayoutClaim::Claimed
        }
    }
}

fn claim_pending_payout_with(
    running: bool,
    pending: &mut VecDeque<BridgeLog>,
    current: u64,
    run_generation: u64,
    from_tx: &BridgeTx,
    candidate: &BridgeTx,
) -> PayoutClaim {
    if !finalize_run_matches(run_generation, current, running) {
        return PayoutClaim::RunSuperseded;
    }
    claim_payout_in(pending, from_tx, candidate)
}

fn claim_pending_payout(
    run_generation: u64,
    from_tx: &BridgeTx,
    candidate: &BridgeTx,
) -> PayoutClaim {
    let current = FINALIZE_RUN_GENERATION.with(Cell::get);
    STATE.with_borrow_mut(|state| {
        claim_pending_payout_with(
            state.finalize_bridging_round.1,
            &mut state.pending,
            current,
            run_generation,
            from_tx,
            candidate,
        )
    })
}

/// RPC values shared by every task in one finalization round.
///
/// Several receipts on the same EVM chain need the same latest block height.
/// Keeping one async slot per chain prevents concurrent tasks from paying for
/// duplicate `eth_blockNumber` outcalls while still allowing different chains
/// to progress in parallel. Errors are cached for the round too: the slot's
/// lock is held across the fetch, so if the leader's full provider sweep just
/// failed, the tasks parked behind it inherit that failure instead of each
/// serially repeating the same sweep — which could otherwise stretch a round
/// toward the stale-lock takeover.
type EvmBlockSlot = Rc<futures::lock::Mutex<Option<Result<u64, String>>>>;
type EvmBlockCache = Rc<RefCell<HashMap<String, EvmBlockSlot>>>;

#[derive(Clone, Default)]
struct FinalizeContext {
    evm_blocks: EvmBlockCache,
}

impl FinalizeContext {
    async fn evm_block_number<H: HttpOutcall>(
        &self,
        chain: &str,
        client: &EvmClient<H>,
        now_ms: u64,
    ) -> Result<u64, String> {
        let slot = self
            .evm_blocks
            .borrow_mut()
            .entry(chain.to_string())
            .or_insert_with(|| Rc::new(futures::lock::Mutex::new(None)))
            .clone();
        let mut cached = slot.lock().await;
        if let Some(result) = cached.clone() {
            return result;
        }

        let result = client.block_number(now_ms).await;
        *cached = Some(result.clone());
        result
    }
}

impl TaskOutcome {
    fn into_log(self) -> BridgeLog {
        match self {
            Self::Retained(log) | Self::Abandoned(log) => log,
        }
    }
}

/// How far a task has got, used to tell a round that advanced something from one
/// that only re-polled the same unchanged transactions.
fn task_progress(log: &BridgeLog) -> (bool, bool, bool) {
    (
        log.from_tx.is_finalized(),
        log.to_tx.is_some(),
        log.to_tx.as_ref().is_some_and(|tx| tx.is_finalized()),
    )
}

/// Delay before the next finalization round, given how many consecutive rounds
/// have left every pending task unchanged.
///
/// A transaction that was dropped or replaced never produces a receipt, and
/// polling for one is not an error, so without a backoff the chain re-queries
/// the RPC providers forever — enough to spend a canister's entire cycle balance
/// on a single abandoned task.
///
/// The first tier is where healthy bridging lives, and it is paced by what the
/// chains can actually do: two confirmations take ~3s on BNB Chain and ~25s on
/// Ethereum, and Solana needs ~15s to finalize. Polling faster than that only
/// pays to re-read the same answer, so the tier covers a full minute at a
/// three-second cadence and every tier after it backs off hard.
fn finalize_poll_delay_secs(idle_rounds: u64) -> u64 {
    match idle_rounds {
        0..=19 => 3,   // ~1min: normal EVM/Solana finality lands here
        20..=39 => 15, // ~6min
        40..=99 => 60, // ~1h
        _ => 300,
    }
}

fn bump_priority_fee(value: u128) -> Result<u128, String> {
    value
        .checked_add(value / 5)
        .ok_or_else(|| "max_priority_fee_per_gas overflow".to_string())
}

fn calculate_max_fee_per_gas(
    gas_price: u128,
    max_priority_fee_per_gas: u128,
) -> Result<u128, String> {
    gas_price
        .checked_mul(2)
        .and_then(|value| value.checked_add(max_priority_fee_per_gas))
        .ok_or_else(|| "max_fee_per_gas overflow".to_string())
}

pub mod state {
    use std::str::FromStr;

    use super::*;

    pub static DEFAULT_EXPR_PATH: LazyLock<HttpCertificationPath<'static>> =
        LazyLock::new(|| HttpCertificationPath::wildcard(""));
    pub static DEFAULT_CERTIFICATION: LazyLock<HttpCertification> =
        LazyLock::new(HttpCertification::skip);
    pub static DEFAULT_CEL_EXPR: LazyLock<String> =
        LazyLock::new(|| create_cel_expr(&DefaultCelBuilder::skip_certification()));
    pub static DEFAULT_CERT_ENTRY: LazyLock<HttpCertificationTreeEntry> = LazyLock::new(|| {
        HttpCertificationTreeEntry::new(&*DEFAULT_EXPR_PATH, *DEFAULT_CERTIFICATION)
    });

    /// Fetches the subnet master keys and derives the bridge's own addresses.
    ///
    /// Every user address is derived from these, so a failure is logged and left
    /// for the next upgrade to retry rather than trapping the install.
    pub async fn init_public_keys() {
        let key_name = STATE.with_borrow(|s| s.key_name.clone());

        match ecdsa_public_key(key_name.clone(), vec![]).await {
            Ok(root_pk) => {
                STATE.with_borrow_mut(|s| match derive_evm_address(&root_pk, &s.icp_address) {
                    Ok(evm_address) => {
                        s.ecdsa_public_key = root_pk;
                        s.evm_address = evm_address;
                    }
                    Err(err) => {
                        ic_cdk::api::debug_print(format!("failed to derive EVM address: {err}"))
                    }
                })
            }
            Err(err) => {
                ic_cdk::api::debug_print(format!("failed to retrieve ECDSA public key: {err}"));
            }
        }

        init_ed25519_public_key(key_name).await;
    }

    /// Fills in the Ed25519 key on an upgrade from a version that predates
    /// Solana support; a no-op once it is there.
    pub async fn try_init_ed25519_public_key() {
        let (key_name, missing) = STATE.with_borrow(|s| {
            (
                s.key_name.clone(),
                s.ed25519_public_key.public_key.is_empty(),
            )
        });

        if missing {
            init_ed25519_public_key(key_name).await;
        }
    }

    async fn init_ed25519_public_key(key_name: String) {
        match schnorr_public_key(key_name, vec![]).await {
            Ok(root_pk) => {
                STATE.with_borrow_mut(|s| match derive_svm_address(&root_pk, &s.icp_address) {
                    Ok(svm_address) => {
                        s.ed25519_public_key = root_pk;
                        s.svm_address = svm_address;
                    }
                    Err(err) => {
                        ic_cdk::api::debug_print(format!("failed to derive SVM address: {err}"))
                    }
                })
            }
            Err(err) => {
                ic_cdk::api::debug_print(format!("failed to retrieve Schnorr public key: {err}"));
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
                let v: State = cbor_from_slice(bytes).expect("failed to decode STATE_STORE data");
                *h = v;
            });
        });
    }

    pub fn save() {
        STATE.with_borrow(|h| {
            STATE_STORE.with_borrow_mut(|r| {
                let buf = cbor_into_vec(h).expect("failed to encode STATE_STORE data");
                r.set(buf);
            });
        });
    }

    pub fn info() -> StateInfo {
        let total_bridge_count = BRIDGE_LOGS.with_borrow(|r| r.len());
        STATE.with_borrow(|s| StateInfo::new(s, total_bridge_count))
    }

    /// Copies the per-user archive index into its current layout, once, on
    /// the first upgrade that has the layout. The legacy map is left as it
    /// is, so an earlier version can still be reinstalled over this one.
    pub fn migrate_user_log_index() -> u64 {
        let legacy_memory = MEMORY_MANAGER.with_borrow(|m| m.get(LEGACY_USER_LOGS_MEMORY_ID));
        if legacy_memory.size() == 0 {
            // never written: a canister installed after the legacy layout
            return 0;
        }
        let legacy: StableBTreeMap<Principal, LegacyUserLogs, Memory> =
            StableBTreeMap::init(legacy_memory);
        USER_LOG_INDEX.with_borrow_mut(|index| copy_legacy_user_log_index(&legacy, index))
    }

    fn derive_evm_address(
        public_key: &PublicKeyOutput,
        user: &Principal,
    ) -> Result<Address, String> {
        let pk = derive_public_key(public_key, vec![user.as_slice().to_vec()])
            .map_err(|err| format!("derive_public_key failed: {err}"))?;
        pk.to_evm_address()
    }

    pub fn evm_address(user: &Principal) -> Result<Address, String> {
        STATE.with_borrow(|s| derive_evm_address(&s.ecdsa_public_key, user))
    }

    pub fn evm_client(chain: &str) -> Result<EvmClient<DefaultHttpOutcall>, String> {
        STATE.with_borrow(|s| {
            let (max_confirmations, providers) = s
                .evm_providers
                .get(chain)
                .cloned()
                .ok_or_else(|| format!("no RPC providers configured for chain {chain}"))?;
            Ok(EvmClient::new(
                providers,
                max_confirmations,
                DefaultHttpOutcall::new(s.icp_address),
            ))
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

    fn derive_svm_address(
        public_key: &PublicKeyOutput,
        user: &Principal,
    ) -> Result<Pubkey, String> {
        let pk = derive_schnorr_public_key(public_key, vec![user.as_slice().to_vec()])
            .map_err(|err| format!("derive_schnorr_public_key failed: {err}"))?;
        pk.to_svm_pubkey()
    }

    pub fn svm_address(user: &Principal) -> Result<Pubkey, String> {
        STATE.with_borrow(|s| derive_svm_address(&s.ed25519_public_key, user))
    }

    pub fn svm_client() -> SvmClient<DefaultHttpOutcall> {
        STATE.with_borrow(|s| {
            SvmClient::new(
                s.svm_providers.clone(),
                DefaultHttpOutcall::new(s.icp_address),
            )
        })
    }

    /// Keeps a single effective finalization timer.
    ///
    /// A newly accepted task may bring a distant backoff timer forward, but an
    /// equal or earlier timer is reused. If a round is currently running, the
    /// timer becomes its stale-lock recovery instead of immediately firing a
    /// self-call that can only observe the lock and return.
    ///
    /// The timer carries no round of its own: `finalize_bridging` reads the
    /// current round when it fires. A surviving timer scheduled before other
    /// rounds completed therefore still runs a valid round instead of being
    /// rejected as stale — which would leave the slot empty and the queue
    /// unserved until the next deposit or an admin restart.
    pub fn schedule_finalize(delay: Duration) {
        let now_ms = ic_cdk::api::time() / 1_000_000;
        let (running, started_at) =
            STATE.with_borrow(|s| (s.finalize_bridging_round.1, s.finalize_bridging_started_at));
        let deadline_ms = finalize_timer_deadline_ms(now_ms, delay, running, started_at);

        let should_replace = FINALIZE_TIMER.with_borrow(|timer| {
            timer
                .as_ref()
                .is_none_or(|scheduled| deadline_ms < scheduled.deadline_ms)
        });
        if !should_replace {
            return;
        }

        if let Some(scheduled) = FINALIZE_TIMER.with_borrow_mut(Option::take) {
            ic_cdk_timers::clear_timer(scheduled.id);
        }

        let actual_delay = Duration::from_millis(deadline_ms.saturating_sub(now_ms));
        let id = ic_cdk_timers::set_timer(actual_delay, async move {
            FINALIZE_TIMER.with_borrow_mut(Option::take);
            finalize_bridging().await;
        });
        FINALIZE_TIMER.with_borrow_mut(|timer| {
            *timer = Some(ScheduledFinalize { id, deadline_ms });
        });
    }

    fn clear_finalize_timer() {
        if let Some(scheduled) = FINALIZE_TIMER.with_borrow_mut(Option::take) {
            ic_cdk_timers::clear_timer(scheduled.id);
        }
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
        let _active_bridge_user = acquire_active_bridge_user(user)?;

        let (from, to, token_ledger, token_bridge_fee) = STATE.with_borrow(|s| {
            if s.error_rounds >= MAX_ERROR_ROUNDS {
                return Err("the bridge is temporarily disabled due to errors, please contact the administrator".to_string());
            }

            if icp_amount < s.min_threshold_to_bridge {
                return Err(format!(
                    "amount {} is below the minimum threshold to bridge {}",
                    icp_amount, s.min_threshold_to_bridge
                ));
            }
            bridge_amount_after_fee(icp_amount, s.token_bridge_fee)?;
            let from = if from_chain == "ICP" {
                BridgeTarget::Icp
            } else if from_chain == "SOL" {
                if s.svm_token_address.0 == Pubkey::default() {
                    return Err("SOL token is not supported".to_string());
                }
                BridgeTarget::Sol
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
            } else if to_chain == "SOL" {
                if s.svm_token_address.0 == Pubkey::default() {
                    return Err("SOL token is not supported".to_string());
                }
                if let Some(to_addr) = &to_addr {
                    let _ = Pubkey::from_str(to_addr)
                        .map_err(|_| format!("invalid SOL address: {}", to_addr))?;
                }
                BridgeTarget::Sol
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
                // A task stuck on one of the two chains blocks it for everyone:
                // its deposit is already in, and letting more in behind it only
                // deepens the hole.
                if let Some(err) = &log.error
                    && (err.starts_with(from.name()) || err.starts_with(to.name()))
                {
                    return Err(format!(
                        "there is a pending bridging task with error, please retry later:\n{}",
                        err
                    ));
                }

                // A second unconfirmed EVM deposit from the same user would reuse
                // the nonce the first one is waiting on.
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
            BridgeTarget::Sol => from_svm(user, icp_amount, now_ms).await?,
            BridgeTarget::Evm(chain) => from_evm(chain, user, icp_amount, now_ms).await?,
        };

        let delay = if from == BridgeTarget::Icp { 0 } else { 5 };
        STATE.with_borrow_mut(|s| {
            s.idle_rounds = 0;
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
        });

        schedule_finalize(Duration::from_secs(delay));

        Ok(from_tx)
    }

    /// Finds the log recording `from_tx`, whether it is still being worked on or
    /// already archived.
    pub fn my_bridge_log(user: Principal, from_tx: BridgeTx) -> Option<BridgeLog> {
        let pending = STATE.with_borrow(|s| {
            s.pending
                .iter()
                .find(|item| item.user == user && item.from_tx == from_tx)
                .cloned()
        });
        if pending.is_some() {
            return pending;
        }

        let recent = USER_LOG_INDEX
            .with_borrow(|index| user_log_ids(index, &user, u64::MAX, MAX_LOG_LOOKBACK));
        BRIDGE_LOGS.with_borrow(|log_store| {
            for id in recent {
                if let Some(mut log) = log_store.get(id)
                    && log.from_tx == from_tx
                {
                    log.id = Some(id);
                    return Some(log.into());
                }
            }
            None
        })
    }

    pub fn user_logs(user: Principal, take: usize, prev: Option<u64>) -> Vec<BridgeLog> {
        let ids = USER_LOG_INDEX
            .with_borrow(|index| user_log_ids(index, &user, prev.unwrap_or(u64::MAX), take));
        BRIDGE_LOGS.with_borrow(|log_store| {
            ids.into_iter()
                .filter_map(|id| {
                    let mut log = log_store.get(id)?;
                    log.id = Some(id);
                    Some(log.into())
                })
                .collect()
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

    pub async fn finalize_bridging() {
        let started_at = ic_cdk::api::time() / 1_000_000;
        let tasks = STATE.with_borrow_mut(|s| {
            if !finalize_lock_available(
                s.finalize_bridging_round.1,
                s.finalize_bridging_started_at,
                started_at,
            ) {
                // already running
                return None;
            }

            if s.pending.is_empty() {
                return None;
            }

            s.finalize_bridging_round.1 = true;
            s.finalize_bridging_started_at = started_at;
            // take up to 3 pending tasks to process in parallel
            let mut tasks = Vec::with_capacity(3);
            // Every EVM payout spends from the bridge's one address per
            // chain, so a round takes at most one task per destination chain
            // to keep their nonces apart.
            let mut evm_outgoing_locked: HashSet<&str> = HashSet::new();
            for task in s.pending.iter() {
                if let BridgeTarget::Evm(chain) = &task.to
                    && !evm_outgoing_locked.insert(chain.as_str())
                {
                    // another task already pays out on this chain this round
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
            let run_generation = next_finalize_run_generation();
            // Progress must be measured against how far each task had got when
            // this round cloned it: `claim_pending_payout` writes a broadcast
            // payout into the live task before the round merges, so comparing
            // against the live queue at merge time would no longer see that
            // transition and the round would be miscounted as idle.
            let baselines: Vec<_> = tasks.iter().map(task_progress).collect();
            let tasks = try_finalize_tasks(tasks, run_generation).await;
            let now_ms = ic_cdk::api::time() / 1_000_000;
            let current_generation = FINALIZE_RUN_GENERATION.with(Cell::get);
            let next = STATE.with_borrow_mut(|s| {
                if !finalize_run_matches(
                    run_generation,
                    current_generation,
                    s.finalize_bridging_round.1,
                ) {
                    // A stale-lock takeover owns the queue now. The superseded
                    // round must not merge results, release its lock, or touch
                    // the replacement round's timer.
                    return None;
                }

                let mut has_error = false;
                let mut has_progress = false;
                let mut abandoned: Vec<BridgeTx> = Vec::new();

                // `join_all` keeps the input order, so outcomes line up with
                // the baselines captured before processing.
                for (outcome, baseline) in tasks.into_iter().zip(baselines) {
                    if let TaskOutcome::Abandoned(task) = outcome {
                        // The deposit failed on chain: nothing arrived, so nothing is
                        // owed. Archiving instead of erroring keeps the queue — and
                        // therefore the whole chain — clear for everyone else. The
                        // amount and the fee stay out of the totals: nothing bridged.
                        archive_bridge_log(&task).expect("failed to append to BRIDGE_LOGS");
                        ic_cdk::api::debug_print(format!(
                            "abandoned bridging task: {}",
                            task.error.as_deref().unwrap_or_default()
                        ));
                        abandoned.push(task.from_tx);
                        has_progress = true;
                        continue;
                    }

                    let task = outcome.into_log();
                    has_error = has_error || task.error.is_some();
                    has_progress = has_progress || baseline != task_progress(&task);
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

                                archive_bridge_log(t).expect("failed to append to BRIDGE_LOGS");
                            }
                            break;
                        }
                    }
                }

                s.pending.retain(|t| !abandoned.contains(&t.from_tx));
                s.pending.retain(|t| !t.is_finalized());
                s.finalize_bridging_round = (s.finalize_bridging_round.0.saturating_add(1), false);
                s.finalize_bridging_started_at = 0;

                let next_delay = if s.pending.is_empty() {
                    None
                } else if has_error {
                    s.idle_rounds = 0;
                    s.error_rounds = s.error_rounds.saturating_add(1);
                    if s.error_rounds >= MAX_ERROR_ROUNDS {
                        None
                    } else {
                        Some(5_u64.saturating_mul(s.error_rounds))
                    }
                } else {
                    s.error_rounds = 0;
                    s.idle_rounds = if has_progress {
                        0
                    } else {
                        s.idle_rounds.saturating_add(1)
                    };
                    Some(finalize_poll_delay_secs(s.idle_rounds))
                };
                Some(next_delay)
            });

            let Some(next) = next else {
                return;
            };

            if let Some(delay) = next {
                schedule_finalize(Duration::from_secs(delay));
            } else {
                clear_finalize_timer();
            }
        }
    }

    /// Moves a log into the permanent store and indexes it under its user,
    /// returning the id it was stored under.
    fn archive_bridge_log(log: &BridgeLog) -> Result<u64, String> {
        let log_id = BRIDGE_LOGS
            .with_borrow_mut(|r| r.append(&log.clone().into()))
            .map_err(|err| format!("failed to append to BRIDGE_LOGS: {}", format_error(err)))?;
        USER_LOG_INDEX.with_borrow_mut(|index| {
            index.insert(UserLogKey::new(&log.user, log_id), ());
        });
        Ok(log_id)
    }

    /// Re-arms the finalization timer chain and clears the error circuit breaker.
    ///
    /// The chain stops scheduling itself once `error_rounds` reaches
    /// `MAX_ERROR_ROUNDS`, so without this the canister has to be upgraded to
    /// resume bridging after a task got stuck.
    ///
    /// The in-progress flag is deliberately left alone: forcing it would let two
    /// rounds process the same task and pay a recipient twice. A flag left set by
    /// a round that trapped clears itself after
    /// `FINALIZE_BRIDGING_LOCK_TIMEOUT_MS`.
    pub fn restart_finalize_bridging() -> u64 {
        let round = STATE.with_borrow_mut(|s| {
            s.error_rounds = 0;
            s.idle_rounds = 0;
            s.finalize_bridging_round.0
        });

        schedule_finalize(Duration::from_secs(0));
        round
    }

    /// Rejects manual edits to `pending` while a finalization round is in flight.
    ///
    /// A round works on clones and writes them back when it completes, so an edit
    /// made in the meantime is silently overwritten — or, worse, would undo a
    /// payout the round has already broadcast. A lock that looks abandoned is not
    /// a reason to refuse: recovering from exactly that case is the point. A
    /// successful edit invalidates its generation before this update returns so
    /// a late callback cannot merge or broadcast afterward.
    fn ensure_finalize_bridging_idle() -> Result<bool, String> {
        let now_ms = ic_cdk::api::time() / 1_000_000;
        STATE.with_borrow(|s| {
            if finalize_lock_available(
                s.finalize_bridging_round.1,
                s.finalize_bridging_started_at,
                now_ms,
            ) {
                Ok(s.finalize_bridging_round.1)
            } else {
                Err("a finalization round is in progress, please retry in a moment".to_string())
            }
        })
    }

    /// Returns the pending task whose incoming transaction matches `from_tx`.
    pub fn pending_task(from_tx: &BridgeTx) -> Result<BridgeLog, String> {
        STATE.with_borrow(|s| {
            s.pending
                .iter()
                .find(|t| t.from_tx == *from_tx)
                .cloned()
                .ok_or_else(|| "no pending bridging task matches the given from_tx".to_string())
        })
    }

    /// Clears the outgoing transaction of a stuck task so that the next
    /// finalization round builds and broadcasts a fresh one.
    ///
    /// The caller must have verified on chain that the recorded outgoing
    /// transaction moved no funds — a reverted EVM transaction, or a Solana
    /// transaction whose blockhash expired without landing. Retrying a payout
    /// that did go through pays the recipient twice.
    pub fn retry_pending_task(from_tx: &BridgeTx) -> Result<BridgeLog, String> {
        let stale_running = ensure_finalize_bridging_idle()?;

        let log = STATE.with_borrow_mut(|s| {
            let task = s
                .pending
                .iter_mut()
                .find(|t| t.from_tx == *from_tx)
                .ok_or_else(|| "no pending bridging task matches the given from_tx".to_string())?;

            if task.to_tx.as_ref().is_some_and(|tx| tx.is_finalized()) {
                return Err(
                    "the outgoing transaction is already finalized, nothing to retry".to_string(),
                );
            }

            task.to_tx = None;
            task.error = None;
            Ok(task.clone())
        })?;

        if stale_running {
            next_finalize_run_generation();
        }

        restart_finalize_bridging();
        Ok(log)
    }

    /// Removes a stuck task from the pending queue and archives it with its
    /// error preserved, so the user can still find it through `my_bridge_log`
    /// (while it is within its lookback) or by paging `my_finalized_logs`.
    ///
    /// The bridging did not complete, so the amount and the fee are not added to
    /// `total_bridged_tokens` / `total_collected_fees`. Settling with the user is
    /// left to the administrator.
    ///
    /// Before settling an ICP-bound task manually, check the ledger for the
    /// payout this task would have sent (its dedup key: `created_at` and the
    /// memo derived from `from_tx`). A stale round's `icrc1_transfer` can still
    /// be in flight when the task is closed and land afterward — the EVM/SOL
    /// paths record their claimed transaction in `to_tx` before broadcasting,
    /// but an ICP block height is unknown until the ledger answers, so the
    /// archived record cannot carry it.
    pub fn close_pending_task(from_tx: &BridgeTx, now_ms: u64) -> Result<BridgeLog, String> {
        let stale_running = ensure_finalize_bridging_idle()?;

        let log = STATE.with_borrow_mut(|s| {
            let idx = s
                .pending
                .iter()
                .position(|t| t.from_tx == *from_tx)
                .ok_or_else(|| "no pending bridging task matches the given from_tx".to_string())?;

            if s.pending[idx].is_finalized() {
                return Err(
                    "the bridging task is already finalized and will be archived automatically"
                        .to_string(),
                );
            }

            let mut log = s.pending[idx].clone();
            log.finalized_at = now_ms;
            if log.error.is_none() {
                log.error = Some("closed by the administrator".to_string());
            }

            // Archive before removing: an update method that returns an error still
            // commits its state changes, so dropping the task first could lose it.
            let log_id = archive_bridge_log(&log)?;

            s.pending.remove(idx);
            log.id = Some(log_id);
            Ok(log)
        })?;

        if stale_running {
            next_finalize_run_generation();
        }

        restart_finalize_bridging();
        Ok(log)
    }

    async fn try_finalize_tasks(tasks: Vec<BridgeLog>, run_generation: u64) -> Vec<TaskOutcome> {
        let now_ms = ic_cdk::api::time() / 1_000_000;
        let context = FinalizeContext::default();
        futures::future::join_all(
            tasks
                .into_iter()
                .map(|task| process_task(task, now_ms, context.clone(), run_generation)),
        )
        .await
    }

    async fn process_task(
        mut task: BridgeLog,
        now_ms: u64,
        context: FinalizeContext,
        run_generation: u64,
    ) -> TaskOutcome {
        // Set when the incoming transfer failed on chain. Nothing reached the
        // bridge, so the task is finished — unsuccessfully — and must not be left
        // in the queue holding an error against its chain.
        let mut incoming_failed = false;

        let rt = async {
            let from_finalized = match (&task.from, &mut task.from_tx) {
                (BridgeTarget::Evm(chain), BridgeTx::Evm(finalized, tx_hash)) if !*finalized => {
                    let tx_hash: TxHash = (**tx_hash).into();
                    match check_evm_tx_finalized(&context, chain, &tx_hash, now_ms).await? {
                        EvmTxStatus::Confirmed => {
                            *finalized = true;
                            true
                        }
                        EvmTxStatus::Pending => false,
                        EvmTxStatus::Reverted => {
                            incoming_failed = true;
                            return Err(format!(
                                "{chain}: incoming transaction {tx_hash} reverted on chain, \
                                 nothing was received"
                            ));
                        }
                    }
                }
                (BridgeTarget::Sol, BridgeTx::Sol(finalized, tx_hash)) if !*finalized => {
                    match check_sol_tx_finalized(tx_hash, now_ms).await? {
                        Some(status) if status.is_error() => {
                            incoming_failed = true;
                            return Err(format!(
                                "SOL: incoming transaction failed on chain, \
                                 nothing was received: {:?}",
                                status.err
                            ));
                        }
                        Some(status) if status.is_finalized() => {
                            *finalized = true;
                            true
                        }
                        _ => false,
                    }
                }
                _ => true,
            };

            if from_finalized {
                if !finalize_run_is_current(run_generation) {
                    return Ok(());
                }
                match (&task.to, &mut task.to_tx) {
                    (BridgeTarget::Icp, None) => {
                        let token_ledger = STATE.with_borrow(|s| s.token_ledger);
                        let to_addr = if let Some(addr) = &task.to_addr {
                            Principal::from_text(addr)
                                .map_err(|_| format!("ICP: invalid to_addr principal: {}", addr))?
                        } else {
                            task.user
                        };
                        let to_amount = bridge_amount_after_fee(task.icp_amount, task.fee)?;
                        let dedup = (task.created_at, payout_memo(&task.from_tx));
                        let to_tx = to_icp(token_ledger, to_addr, to_amount, Some(dedup)).await?;
                        task.to_tx = Some(to_tx);
                    }
                    (BridgeTarget::Evm(chain), None) => {
                        let to_addr = if let Some(addr) = &task.to_addr {
                            addr.parse::<Address>().map_err(|_| {
                                format!("{chain}: invalid to_addr address: {}", addr)
                            })?
                        } else {
                            // Prefix with the chain name so the stuck-task gate
                            // in `bridge()` recognizes which chain the error
                            // belongs to.
                            state::evm_address(&task.user)
                                .map_err(|err| format!("{chain}: {err}"))?
                        };
                        let to_amount = bridge_amount_after_fee(task.icp_amount, task.fee)?;
                        match to_evm(
                            run_generation,
                            &task.from_tx,
                            chain,
                            to_addr,
                            to_amount,
                            now_ms,
                        )
                        .await
                        {
                            Ok(Some(to_tx)) => task.to_tx = Some(to_tx),
                            // This round was superseded, or a concurrent round
                            // removed the task while the transaction was built.
                            Ok(None) => return Ok(()),
                            Err((sent_tx, err)) => {
                                task.to_tx = sent_tx;
                                return Err(err);
                            }
                        }
                    }
                    (BridgeTarget::Evm(chain), Some(BridgeTx::Evm(finalized, tx_hash)))
                        if !*finalized =>
                    {
                        let tx_hash: TxHash = (**tx_hash).into();
                        match check_evm_tx_finalized(&context, chain, &tx_hash, now_ms).await? {
                            EvmTxStatus::Confirmed => *finalized = true,
                            EvmTxStatus::Pending => {}
                            // Unlike a failed deposit, a failed payout leaves the bridge
                            // owing the user, and rebuilding it automatically would burn
                            // gas on every attempt. Leave it for an administrator.
                            EvmTxStatus::Reverted => {
                                return Err(format!(
                                    "{chain}: outgoing transaction {tx_hash} reverted on chain, \
                                     an administrator must retry or close this task"
                                ));
                            }
                        }
                    }
                    (BridgeTarget::Sol, None) => {
                        let to_addr = if let Some(addr) = &task.to_addr {
                            Pubkey::from_str(addr)
                                .map_err(|_| format!("SOL: invalid to_addr address: {}", addr))?
                        } else {
                            state::svm_address(&task.user).map_err(|err| format!("SOL: {err}"))?
                        };
                        let to_amount = bridge_amount_after_fee(task.icp_amount, task.fee)?;
                        match to_svm(run_generation, &task.from_tx, to_addr, to_amount, now_ms)
                            .await
                        {
                            Ok(Some(to_tx)) => task.to_tx = Some(to_tx),
                            // This round was superseded, or a concurrent round
                            // removed the task while the transaction was built.
                            Ok(None) => return Ok(()),
                            Err((sent_tx, err)) => {
                                task.to_tx = sent_tx;
                                return Err(err);
                            }
                        }
                    }
                    (BridgeTarget::Sol, Some(BridgeTx::Sol(finalized, tx_hash))) if !*finalized => {
                        match check_sol_tx_finalized(tx_hash, now_ms).await? {
                            // The transaction was rejected on chain, so it moved no
                            // funds and a fresh one can safely be built next round.
                            Some(status) if status.is_error() => {
                                task.to_tx = None; // reset to_tx to retry
                                return Err(format!("SOL: transaction failed: {:?}", status.err));
                            }
                            Some(status) if status.is_finalized() => {
                                *finalized = true;
                            }
                            // Either still confirming, or the RPC node has not indexed the
                            // signature yet. Keep waiting: dropping to_tx here would send a
                            // second transfer and pay the recipient twice.
                            _ => {}
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

        if incoming_failed {
            task.finalized_at = now_ms;
            TaskOutcome::Abandoned(task)
        } else {
            TaskOutcome::Retained(task)
        }
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
        )
        .await?;
        let res = res
            .map_err(|err| format!("ICP: failed to transfer token from user, error: {:?}", err))?;
        let idx = u64::try_from(&res.0).map_err(|_| "ICP: block height too large".to_string())?;
        Ok(BridgeTx::Icp(true, idx))
    }

    /// A stable, unique, ≤32-byte identifier of a task's incoming transaction,
    /// used as the ledger dedup memo for the outgoing ICP payout.
    fn payout_memo(from_tx: &BridgeTx) -> Memo {
        match from_tx {
            BridgeTx::Icp(_, idx) => Memo::from(*idx),
            BridgeTx::Evm(_, hash) => Memo::from((**hash).to_vec()),
            BridgeTx::Sol(_, sig) => Memo::from((**sig)[..32].to_vec()),
        }
    }

    /// Transfers tokens out of the bridge on the ICP side.
    ///
    /// `dedup` carries the task's creation time and the memo derived from its
    /// incoming transaction. Passing it makes the ledger itself reject a second
    /// transfer for the same task: a payout re-attempted by a stale-lock
    /// takeover round while the first call is still in flight — or retried
    /// after an error whose outcome was unknown — comes back as `Duplicate`
    /// instead of paying the recipient twice. A task stuck past the ledger's
    /// ~24h dedup window fails with `TooOld` and is left for an administrator,
    /// which is the safe direction. `None` (admin fee collection) keeps the
    /// ledger's default behavior.
    pub async fn to_icp(
        token_ledger: Principal,
        to_addr: Principal,
        icp_amount: u128,
        dedup: Option<(u64, Memo)>,
    ) -> Result<BridgeTx, String> {
        if icp_amount == 0 {
            return Err("ICP: amount must be greater than 0".to_string());
        }

        let (created_at_time, memo) = match dedup {
            Some((created_at_ms, memo)) => {
                (Some(created_at_ms.saturating_mul(1_000_000)), Some(memo))
            }
            None => (None, None),
        };
        let res: Result<Nat, TransferError> = call(
            token_ledger,
            "icrc1_transfer",
            (TransferArg {
                from_subaccount: None,
                to: Account {
                    owner: to_addr,
                    subaccount: None,
                },
                fee: None,
                created_at_time,
                memo,
                amount: icp_amount.into(),
            },),
        )
        .await
        .map_err(|err| format!("ICP: {err}"))?;
        let res = match res {
            Ok(idx) => idx,
            // An identical transfer for this task already landed, so the payout
            // is done; adopt its block height.
            Err(TransferError::Duplicate { duplicate_of }) => duplicate_of,
            Err(err) => {
                return Err(format!(
                    "ICP: failed to transfer token to user, error: {:?}",
                    err
                ));
            }
        };
        let idx = u64::try_from(&res.0).map_err(|_| "ICP: block height too large".to_string())?;
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

    /// Broadcasts an outgoing payout transaction.
    ///
    /// A successful `Some` is the transaction this task must poll. `None` means
    /// the round was superseded or the task disappeared while its candidate was
    /// being built. On failure the error's `Option<BridgeTx>` carries the
    /// transaction that was atomically recorded before it was handed to the
    /// provider, if any: the provider may have accepted and propagated it even
    /// though the RPC call itself failed.
    type BroadcastResult = Result<Option<BridgeTx>, (Option<BridgeTx>, String)>;

    async fn to_evm(
        run_generation: u64,
        from_tx: &BridgeTx,
        chain: &str,
        to_addr: Address,
        icp_amount: u128,
        now_ms: u64,
    ) -> BroadcastResult {
        let (client, signed_tx) = build_erc20_transfer_tx(
            chain,
            &ic_cdk::api::canister_self(),
            &to_addr,
            icp_amount,
            now_ms,
        )
        .await
        .map_err(|err| (None, format!("{chain}: {err}")))?;

        let tx_hash: [u8; 32] = (*signed_tx.hash()).into();
        let tx = BridgeTx::Evm(false, tx_hash.into());
        let data = Bytes::from(signed_tx.encoded_2718()).to_string();
        broadcast_payout(run_generation, from_tx, tx, chain, || {
            client.send_raw_transaction(now_ms, data)
        })
        .await
    }

    /// Records `tx` as the task's payout, then hands it to the provider.
    ///
    /// The claim comes first so that a broadcast whose outcome is unknown is
    /// never rebuilt: the error carries the claimed transaction for exactly
    /// that case. A claim that finds the slot taken, the round superseded or
    /// the task gone returns without broadcasting, see [`PayoutClaim`].
    async fn broadcast_payout<F, Fut, T>(
        run_generation: u64,
        from_tx: &BridgeTx,
        tx: BridgeTx,
        chain: &str,
        send: F,
    ) -> BroadcastResult
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        match claim_pending_payout(run_generation, from_tx, &tx) {
            PayoutClaim::Claimed => {}
            PayoutClaim::Existing(existing) => return Ok(Some(existing)),
            PayoutClaim::RunSuperseded | PayoutClaim::TaskGone => return Ok(None),
        }

        send()
            .await
            .map_err(|err| (Some(tx.clone()), format!("{chain}: {err}")))?;
        Ok(Some(tx))
    }

    async fn from_svm(user: Principal, icp_amount: u128, now_ms: u64) -> Result<BridgeTx, String> {
        let to_addr = STATE.with_borrow(|s| s.svm_address);
        let (client, signed_tx) = build_spl_transfer_tx(&user, &to_addr, icp_amount, now_ms)
            .await
            .map_err(|err| format!("SOL: {err}"))?;
        let tx_hash: [u8; 64] = signed_tx.signatures[0].into();
        let data = bincode::serialize(&signed_tx).map_err(|err| format!("SOL: {err}"))?;

        let _ = client
            .send_transaction(now_ms, data.into())
            .await
            .map_err(|err| format!("SOL: {err}"))?;
        Ok(BridgeTx::Sol(false, tx_hash.into()))
    }

    async fn to_svm(
        run_generation: u64,
        from_tx: &BridgeTx,
        to_addr: Pubkey,
        icp_amount: u128,
        now_ms: u64,
    ) -> BroadcastResult {
        let (client, signed_tx) =
            build_spl_transfer_tx(&ic_cdk::api::canister_self(), &to_addr, icp_amount, now_ms)
                .await
                .map_err(|err| (None, format!("SOL: {err}")))?;

        let tx_hash: [u8; 64] = signed_tx.signatures[0].into();
        let tx = BridgeTx::Sol(false, tx_hash.into());
        let data = bincode::serialize(&signed_tx).map_err(|err| (None, format!("SOL: {err}")))?;
        broadcast_payout(run_generation, from_tx, tx, "SOL", || {
            client.send_transaction(now_ms, data.into())
        })
        .await
    }

    /// What an EVM transaction should do, before the nonce, the gas price and
    /// the signature are attached.
    struct EvmTxPlan {
        /// Where the transaction is sent: the token contract, or the recipient
        /// itself for a native transfer.
        to: Address,
        /// The address the funds end up at. It is the transaction destination for
        /// a native transfer and the `transfer` argument for an ERC-20 one, and
        /// paying our own address is always a mistake.
        recipient: Address,
        value: u128,
        input: Vec<u8>,
        gas_limit: u64,
    }

    /// Attaches the nonce, the gas price and the signature to a planned
    /// transaction, refreshing the cached gas price when it has gone stale.
    async fn sign_evm_tx(
        chain: &str,
        from: &Principal,
        plan: EvmTxPlan,
        now_ms: u64,
    ) -> Result<(EvmClient<DefaultHttpOutcall>, Signed<TxEip1559>), String> {
        let EvmTxPlan {
            to,
            recipient,
            value,
            input,
            gas_limit,
        } = plan;

        let (key_name, from_pk, mut tx, gas_updated_at) = STATE.with_borrow(|s| {
            let (_, _, chain_id) = s
                .evm_token_contracts
                .get(chain)
                .ok_or_else(|| format!("chain {chain} not found"))?;
            let from_pk = derive_public_key(&s.ecdsa_public_key, vec![from.as_slice().to_vec()])
                .map_err(|err| format!("derive_public_key failed: {err}"))?;

            let (gas_updated_at, gas_price, max_priority_fee_per_gas) =
                s.evm_latest_gas.get(chain).cloned().unwrap_or_default();
            let max_priority_fee_per_gas = bump_priority_fee(max_priority_fee_per_gas)?;
            let max_fee_per_gas = calculate_max_fee_per_gas(gas_price, max_priority_fee_per_gas)?;

            Ok::<_, String>((
                s.key_name.clone(),
                from_pk,
                TxEip1559 {
                    chain_id: *chain_id,
                    nonce: 0u64,
                    gas_limit,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                    to: to.into(),
                    value: value.try_into().map_err(|_| "invalid amount".to_string())?,
                    input: input.into(),
                    ..Default::default()
                },
                gas_updated_at,
            ))
        })?;

        let from_addr = from_pk.to_evm_address()?;
        if from_addr == recipient {
            return Err("from and to cannot be the same".to_string());
        }

        let client = evm_client(chain)?;
        if gas_updated_at.saturating_add(120_000) >= now_ms {
            tx.nonce = client.get_transaction_count(now_ms, &from_addr).await?;
        } else {
            let (nonce, gas_price, max_priority_fee_per_gas) = futures::future::try_join3(
                client.get_transaction_count(now_ms, &from_addr),
                client.gas_price(now_ms),
                client.max_priority_fee_per_gas(now_ms),
            )
            .await?;
            tx.nonce = nonce;
            tx.max_priority_fee_per_gas = bump_priority_fee(max_priority_fee_per_gas)?;
            tx.max_fee_per_gas = calculate_max_fee_per_gas(gas_price, tx.max_priority_fee_per_gas)?;
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
        if sig.len() != 64 {
            return Err(format!("invalid ECDSA signature length: {}", sig.len()));
        }
        let signature = Signature::new(
            U256::from_be_slice(&sig[0..32]),  // r
            U256::from_be_slice(&sig[32..64]), // s
            y_parity(msg_hash.as_slice(), &sig, from_pk.public_key.as_slice())?,
        );

        Ok((client, tx.into_signed(signature)))
    }

    pub async fn build_erc20_transfer_tx(
        chain: &str,
        from: &Principal,
        to_addr: &Address,
        icp_amount: u128,
        now_ms: u64,
    ) -> Result<(EvmClient<DefaultHttpOutcall>, Signed<TxEip1559>), String> {
        let plan = STATE.with_borrow(|s| {
            let (contract, decimals, _) = s
                .evm_token_contracts
                .get(chain)
                .ok_or_else(|| format!("chain {chain} not found"))?;

            let value = convert_amount(icp_amount, s.token_decimals, *decimals)?;
            if value == 0 {
                return Err(format!(
                    "{chain}: amount {icp_amount} is too small for target token decimals {decimals}"
                ));
            }

            Ok::<_, String>(EvmTxPlan {
                to: *contract,
                recipient: *to_addr,
                value: 0,
                input: encode_erc20_transfer(to_addr, value),
                gas_limit: s.erc20_gas_limit,
            })
        })?;

        sign_evm_tx(chain, from, plan, now_ms).await
    }

    pub async fn build_evm_transfer_tx(
        chain: &str,
        from: &Principal,
        to_addr: &Address,
        amount: u128,
        now_ms: u64,
    ) -> Result<(EvmClient<DefaultHttpOutcall>, Signed<TxEip1559>), String> {
        if amount == 0 {
            return Err("amount must be greater than 0".to_string());
        }

        let plan = EvmTxPlan {
            to: *to_addr,
            recipient: *to_addr,
            value: amount,
            input: Vec::new(),
            gas_limit: NATIVE_TRANSFER_GAS_LIMIT,
        };

        sign_evm_tx(chain, from, plan, now_ms).await
    }

    async fn check_evm_tx_finalized(
        context: &FinalizeContext,
        chain: &str,
        tx_hash: &TxHash,
        now_ms: u64,
    ) -> Result<EvmTxStatus, String> {
        let client = evm_client(chain).map_err(|err| format!("{chain}: {err}"))?;
        let receipt = client
            .get_transaction_receipt(now_ms, tx_hash)
            .await
            .map_err(|err| format!("{chain}: failed to get transaction receipt, error: {err}"))?;

        let receipt = match receipt {
            Some(receipt) if receipt.transaction_hash == *tx_hash => receipt,
            _ => return Ok(EvmTxStatus::Pending),
        };
        let Some(block_number) = receipt.block_number() else {
            return Ok(EvmTxStatus::Pending);
        };

        // The receipt's logs are not inspected: this canister built the
        // transaction, so a successful status is its ERC-20 `transfer` having
        // run. The event it would carry is `Transfer(address,address,uint256)`
        // from the token contract, with the from and to addresses left-padded
        // in topics 1 and 2 and the amount ABI-encoded in the data.
        let latest = context
            .evm_block_number(chain, &client, now_ms)
            .await
            .map_err(|err| format!("{chain}: failed to get block number, error: {err}"))?;

        if latest.saturating_sub(block_number) < client.max_confirmations {
            return Ok(EvmTxStatus::Pending);
        }

        // A mined-and-reverted transaction will never finalize, so it must not be
        // reported as merely unconfirmed — that polls forever.
        Ok(if receipt.succeeded() {
            EvmTxStatus::Confirmed
        } else {
            EvmTxStatus::Reverted
        })
    }

    async fn check_sol_tx_finalized(
        tx_hash: &[u8; 64],
        now_ms: u64,
    ) -> Result<Option<SignatureStatus>, String> {
        let sig = SvmSignature::from(*tx_hash);
        let client = svm_client();
        let status = client
            .get_signature_statuses(now_ms, sig.to_string().as_str())
            .await
            .map_err(|err| format!("SOL: failed to get signature status, error: {}", err))?;
        Ok(status)
    }

    pub async fn build_spl_transfer_tx(
        from: &Principal,
        to_addr: &Pubkey,
        icp_amount: u128,
        now_ms: u64,
    ) -> Result<(SvmClient<DefaultHttpOutcall>, Transaction), String> {
        let (from_addr, ixs) = STATE.with_borrow(|s| {
            let (mint_pubkey, decimals, token_program_id) = s.svm_token_address;

            let amount = convert_amount(icp_amount, s.token_decimals, decimals)?;
            if amount == 0 {
                return Err(format!(
                    "amount {icp_amount} is too small for target token decimals {decimals}"
                ));
            }
            let amount: u64 = amount
                .try_into()
                .map_err(|_| format!("amount is too large: {}", amount))?;
            let from_addr = derive_svm_address(&s.ed25519_public_key, from)?;
            if &from_addr == to_addr {
                return Err("from and to cannot be the same".to_string());
            }

            let from_pubkey =
                get_associated_token_address(&from_addr, &mint_pubkey, &token_program_id);
            let to_pubkey = get_associated_token_address(to_addr, &mint_pubkey, &token_program_id);
            let ix0 = create_associated_token_account_idempotent(
                &from_addr,
                to_addr,
                &mint_pubkey,
                &token_program_id,
            );
            let ix = transfer_checked_instruction(
                &token_program_id,
                &from_pubkey,
                &mint_pubkey,
                &to_pubkey,
                &from_addr,
                &[],
                amount,
                decimals,
            );

            Ok::<_, String>((from_addr, vec![ix0, ix]))
        })?;

        sign_svm_tx(from, from_addr, &ixs, now_ms).await
    }

    pub async fn build_sol_transfer_tx(
        from: &Principal,
        to_addr: &Pubkey,
        sol_amount: u64,
        now_ms: u64,
    ) -> Result<(SvmClient<DefaultHttpOutcall>, Transaction), String> {
        if sol_amount == 0 {
            return Err("amount must be greater than 0".to_string());
        }

        let (from_addr, ixs) = STATE.with_borrow(|s| {
            let from_addr = derive_svm_address(&s.ed25519_public_key, from)?;
            if &from_addr == to_addr {
                return Err("from and to cannot be the same".to_string());
            }

            let ix = system_transfer_instruction(&from_addr, to_addr, sol_amount);
            Ok::<_, String>((from_addr, vec![ix]))
        })?;

        sign_svm_tx(from, from_addr, &ixs, now_ms).await
    }

    /// Attaches a recent blockhash and the signature of `from` to the planned
    /// instructions, with `from_addr` — the address `from` derives to — as
    /// the fee payer.
    async fn sign_svm_tx(
        from: &Principal,
        from_addr: Pubkey,
        ixs: &[Instruction],
        now_ms: u64,
    ) -> Result<(SvmClient<DefaultHttpOutcall>, Transaction), String> {
        let key_name = STATE.with_borrow(|s| s.key_name.clone());
        let client = svm_client();
        let block = client
            .get_latest_blockhash(now_ms)
            .await
            .map_err(|err| format!("failed to get latest blockhash, error: {err}"))?;

        let message = Message::new_with_blockhash(ixs, Some(&from_addr), &block);
        let msg = bincode::serialize(&message).map_err(|err| err.to_string())?;
        let sig = sign_with_schnorr(key_name, vec![from.as_slice().to_vec()], msg).await?;
        let signature: [u8; 64] = sig
            .try_into()
            .map_err(|_| "invalid signature length".to_string())?;
        let transaction = Transaction {
            message,
            signatures: vec![signature.into()],
        };

        Ok((client, transaction))
    }
}

fn y_parity(prehash: &[u8], sig: &[u8], pubkey: &[u8]) -> Result<bool, String> {
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

    let orig_key = VerifyingKey::from_sec1_bytes(pubkey).map_err(format_error)?;
    let signature = Signature::try_from(sig).map_err(format_error)?;
    for parity in [0u8, 1] {
        let recid = RecoveryId::try_from(parity).map_err(format_error)?;
        let recovered_key = match VerifyingKey::recover_from_prehash(prehash, &signature, recid) {
            Ok(k) => k,
            Err(_) => continue, // try the other parity
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcall::tests::{MockHttpOutcall, success_response};
    use ic_stable_structures::VectorMemory;

    fn principal(bytes: &[u8]) -> Principal {
        Principal::from_slice(bytes)
    }

    #[test]
    fn user_log_index_keeps_each_users_ids_contiguous_and_ordered() {
        let mut index: StableBTreeMap<UserLogKey, (), VectorMemory> =
            StableBTreeMap::new(VectorMemory::default());

        // principals of different lengths, one a byte-prefix of another: the
        // length-first encoding must keep their ranges apart
        let short = principal(&[1, 2, 3]);
        let long = principal(&[1, 2, 3, 4, 5]);
        let longest = principal(&[0xff; Principal::MAX_LENGTH_IN_BYTES]);
        for (user, ids) in [
            (&short, vec![1, 5, 9]),
            (&long, vec![2, 3, 8]),
            (&longest, vec![4, 6, 7]),
        ] {
            for id in ids {
                index.insert(UserLogKey::new(user, id), ());
            }
        }

        assert_eq!(user_log_ids(&index, &short, u64::MAX, 10), vec![9, 5, 1]);
        assert_eq!(user_log_ids(&index, &long, u64::MAX, 10), vec![8, 3, 2]);
        assert_eq!(user_log_ids(&index, &longest, u64::MAX, 10), vec![7, 6, 4]);

        // `before` is exclusive and `take` is a page size, as `my_finalized_logs` needs
        assert_eq!(user_log_ids(&index, &short, 9, 10), vec![5, 1]);
        assert_eq!(user_log_ids(&index, &short, u64::MAX, 2), vec![9, 5]);
        assert_eq!(user_log_ids(&index, &short, 0, 10), Vec::<u64>::new());
        assert_eq!(
            user_log_ids(&index, &Principal::anonymous(), u64::MAX, 10),
            Vec::<u64>::new()
        );

        let key = UserLogKey::new(&longest, u64::MAX - 1);
        assert_eq!(key.log_id(), u64::MAX - 1);
        assert_eq!(UserLogKey::from_bytes(key.to_bytes()), key);
    }

    #[test]
    fn legacy_user_log_index_is_copied_exactly_once() {
        let mut legacy: StableBTreeMap<Principal, LegacyUserLogs, VectorMemory> =
            StableBTreeMap::new(VectorMemory::default());
        let mut index: StableBTreeMap<UserLogKey, (), VectorMemory> =
            StableBTreeMap::new(VectorMemory::default());

        let alice = principal(&[1; 29]);
        let bob = principal(&[2; 10]);
        legacy.insert(
            alice,
            LegacyUserLogs {
                logs: BTreeSet::from([0, 2, 5]),
            },
        );
        legacy.insert(
            bob,
            LegacyUserLogs {
                logs: BTreeSet::from([1, 3, 4]),
            },
        );

        assert_eq!(copy_legacy_user_log_index(&legacy, &mut index), 6);
        assert_eq!(user_log_ids(&index, &alice, u64::MAX, 10), vec![5, 2, 0]);
        assert_eq!(user_log_ids(&index, &bob, u64::MAX, 10), vec![4, 3, 1]);

        // a second upgrade finds the index filled and leaves it alone
        index.insert(UserLogKey::new(&bob, 6), ());
        assert_eq!(copy_legacy_user_log_index(&legacy, &mut index), 0);
        assert_eq!(user_log_ids(&index, &bob, u64::MAX, 10), vec![6, 4, 3, 1]);
    }

    #[test]
    fn erc20_gas_limit_must_be_plausible() {
        assert!(validate_erc20_gas_limit(21_000).is_ok());
        assert!(validate_erc20_gas_limit(DEFAULT_ERC20_GAS_LIMIT).is_ok());
        assert!(validate_erc20_gas_limit(1_000_000).is_ok());
        assert!(validate_erc20_gas_limit(20_999).is_err());
        assert!(validate_erc20_gas_limit(1_000_001).is_err());
        assert!(validate_erc20_gas_limit(0).is_err());
    }

    #[test]
    fn finalization_shares_latest_block_outcalls_per_chain() {
        let mock = MockHttpOutcall::new(vec![success_response(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x2a"
        }))]);
        let client = EvmClient::new(vec!["https://rpc".to_string()], 2, mock.clone());
        let context = FinalizeContext::default();

        let (first, second) = futures::executor::block_on(futures::future::join(
            context.evm_block_number("BNB", &client, 1),
            context.evm_block_number("BNB", &client, 1),
        ));

        assert_eq!(first, Ok(42));
        assert_eq!(second, Ok(42));
        assert_eq!(mock.urls(), vec!["https://rpc".to_string()]);
    }

    #[test]
    fn finalization_shares_failed_block_outcalls_per_chain() {
        // A failed sweep is cached for the round too: tasks parked behind the
        // leader inherit its error instead of serially repeating the sweep.
        let mock = MockHttpOutcall::new(vec![Err("all providers down".to_string())]);
        let client = EvmClient::new(vec!["https://rpc".to_string()], 2, mock.clone());
        let context = FinalizeContext::default();

        let (first, second) = futures::executor::block_on(futures::future::join(
            context.evm_block_number("BNB", &client, 1),
            context.evm_block_number("BNB", &client, 1),
        ));

        assert!(first.is_err());
        assert_eq!(first, second);
        assert_eq!(mock.urls(), vec!["https://rpc".to_string()]);
    }

    #[test]
    fn only_the_first_overlapping_round_can_claim_a_payout() {
        let from_tx = BridgeTx::Icp(true, 7);
        let first = BridgeTx::Evm(false, [1_u8; 32].into());
        let replacement = BridgeTx::Evm(false, [2_u8; 32].into());
        let mut pending = VecDeque::from([BridgeLog {
            id: None,
            user: Principal::anonymous(),
            from: BridgeTarget::Icp,
            to: BridgeTarget::Evm("ETH".to_string()),
            icp_amount: 100,
            fee: 1,
            from_tx: from_tx.clone(),
            to_tx: None,
            to_addr: None,
            created_at: 1,
            finalized_at: 0,
            error: None,
        }]);

        assert!(matches!(
            claim_payout_in(&mut pending, &from_tx, &first),
            PayoutClaim::Claimed
        ));
        assert!(pending[0].to_tx.as_ref().is_some_and(|tx| tx == &first));

        let second = claim_payout_in(&mut pending, &from_tx, &replacement);
        match second {
            PayoutClaim::Existing(existing) => assert!(existing == first),
            _ => panic!("the stale round replaced the claimed payout"),
        }
        assert!(pending[0].to_tx.as_ref().is_some_and(|tx| tx == &first));

        assert!(matches!(
            claim_payout_in(&mut pending, &BridgeTx::Icp(true, 8), &replacement),
            PayoutClaim::TaskGone
        ));
    }

    #[test]
    fn superseded_finalize_run_cannot_merge() {
        assert!(finalize_run_matches(2, 2, true));
        assert!(!finalize_run_matches(1, 2, true));
        assert!(!finalize_run_matches(2, 2, false));
    }

    #[test]
    fn superseded_run_cannot_claim_a_payout() {
        let from_tx = BridgeTx::Icp(true, 9);
        let candidate = BridgeTx::Evm(false, [3_u8; 32].into());
        let mut pending = VecDeque::from([BridgeLog {
            id: None,
            user: Principal::anonymous(),
            from: BridgeTarget::Icp,
            to: BridgeTarget::Evm("ETH".to_string()),
            icp_amount: 100,
            fee: 1,
            from_tx: from_tx.clone(),
            to_tx: None,
            to_addr: None,
            created_at: 1,
            finalized_at: 0,
            error: None,
        }]);

        // A run whose generation was bumped away must not touch the slot.
        assert!(matches!(
            claim_pending_payout_with(true, &mut pending, 2, 1, &from_tx, &candidate),
            PayoutClaim::RunSuperseded
        ));
        assert!(pending[0].to_tx.is_none());

        // A released lock refuses the claim even for a matching generation.
        assert!(matches!(
            claim_pending_payout_with(false, &mut pending, 2, 2, &from_tx, &candidate),
            PayoutClaim::RunSuperseded
        ));
        assert!(pending[0].to_tx.is_none());

        // The current generation with the lock held is the only one that can.
        assert!(matches!(
            claim_pending_payout_with(true, &mut pending, 2, 2, &from_tx, &candidate),
            PayoutClaim::Claimed
        ));
        assert!(pending[0].to_tx.as_ref().is_some_and(|tx| tx == &candidate));
    }

    #[test]
    fn finalize_poll_backs_off_once_nothing_advances() {
        // healthy bridging: normal finality resolves inside the tight tier
        assert_eq!(finalize_poll_delay_secs(0), 3);
        assert_eq!(finalize_poll_delay_secs(19), 3);

        // a task that stopped advancing is polled progressively less often
        assert_eq!(finalize_poll_delay_secs(20), 15);
        assert_eq!(finalize_poll_delay_secs(40), 60);
        assert_eq!(finalize_poll_delay_secs(100), 300);
        assert_eq!(finalize_poll_delay_secs(u64::MAX), 300);

        // the tight tier has to outlast the slowest finality bridged against:
        // ~25s for two Ethereum confirmations, ~15s for Solana
        let tight_tier: u64 = (0..20).map(finalize_poll_delay_secs).sum();
        assert!(tight_tier >= 60);

        // an abandoned task must not cost more than a few hundred rounds a day
        assert!(86_400 / finalize_poll_delay_secs(u64::MAX) < 500);
    }

    #[test]
    fn finalize_lock_is_taken_over_only_once_it_looks_abandoned() {
        // free lock
        assert!(finalize_lock_available(false, 0, 1_000));

        // a round that is still plausibly running keeps the lock
        assert!(!finalize_lock_available(true, 1_000, 1_000));
        assert!(!finalize_lock_available(
            true,
            1_000,
            1_000 + FINALIZE_BRIDGING_LOCK_TIMEOUT_MS - 1
        ));

        // a round that trapped can never release it, so it is taken over
        assert!(finalize_lock_available(
            true,
            1_000,
            1_000 + FINALIZE_BRIDGING_LOCK_TIMEOUT_MS
        ));

        // state upgraded from a version without the timestamp
        assert!(finalize_lock_available(true, 0, 1_700_000_000_000));
    }

    #[test]
    fn finalization_timer_waits_for_a_running_round_or_uses_the_requested_delay() {
        assert_eq!(
            finalize_timer_deadline_ms(1_000, Duration::from_secs(3), false, 0),
            4_000
        );
        assert_eq!(
            finalize_timer_deadline_ms(2_000, Duration::from_secs(3), true, 1_000),
            1_000 + FINALIZE_BRIDGING_LOCK_TIMEOUT_MS
        );
        assert_eq!(
            finalize_timer_deadline_ms(
                1_000 + FINALIZE_BRIDGING_LOCK_TIMEOUT_MS,
                Duration::from_secs(3),
                true,
                1_000,
            ),
            1_000 + FINALIZE_BRIDGING_LOCK_TIMEOUT_MS + 3_000
        );
    }
}
