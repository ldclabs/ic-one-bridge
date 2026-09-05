use alloy_consensus::{SignableTransaction, Signed, TxEip1559};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxHash, U256, hex};
use candid::{CandidType, Nat, Principal};
use ic_auth_types::{ByteBufB64, cbor_from_slice, cbor_into_vec};
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
use serde_bytes::{ByteArray, ByteBuf};
use solana_instruction::Instruction;
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp,
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    fmt,
    future::Future,
    rc::Rc,
    str::FromStr,
    sync::LazyLock,
    time::Duration,
};

use crate::{
    ecdsa::{cost_sign_with_ecdsa, derive_public_key, ecdsa_public_key, sign_with_ecdsa},
    evm::{EvmClient, EvmReceipt, encode_erc20_transfer},
    helper::{
        bridge_amount_after_fee, call, convert_amount, format_error, now_ms, parse_evm_address,
    },
    outcall::{DefaultHttpOutcall, HttpOutcall},
    schnorr::{derive_schnorr_public_key, schnorr_public_key, sign_with_schnorr},
    svm::{
        Message, Pubkey, Signature as SvmSignature, SolTxStatus, SvmClient, Transaction,
        create_associated_token_account_idempotent, get_associated_token_address,
        system_transfer_instruction, transfer_checked_instruction,
    },
    types::PublicKeyOutput,
};

type Memory = VirtualMemory<DefaultMemoryImpl>;

/// Consecutive rounds with a transient error after which bridging is paused.
const MAX_ERROR_ROUNDS: u64 = 42;

/// How long the rounds wait between attempts once bridging is paused. A
/// clean round lifts the pause, so an outage that ends on its own does not
/// need a governance proposal to recover from.
const ERROR_COOLDOWN_SECS: u64 = 60 * 60;

/// How many pending tasks a round works on at once.
const ROUND_TASK_LIMIT: usize = 3;

/// Delay before the round that follows one that trapped.
const ROUND_TRAP_RETRY_SECS: u64 = 30;

/// How long a transaction may go unseen by every provider before a round asks
/// whether it can still land, and broadcasts it again while it can.
const UNSEEN_TX_GRACE_MS: u64 = 60 * 1000;

/// Finalized blocks past a Solana transaction's last valid height before it is
/// declared expired, on top of the lag a finalized height already has.
const SOL_EXPIRY_MARGIN_BLOCKS: u64 = 32;

/// The fee a Solana transaction with one signature pays.
const SOL_TX_FEE_LAMPORTS: u64 = 5_000;

/// Rent-exempt minimum of a 165-byte SPL token account, which the fee payer
/// puts up when a transfer has to open the recipient's associated account. A
/// Token-2022 account with extensions costs more, so this is a floor.
const SPL_ACCOUNT_RENT_LAMPORTS: u64 = 2_039_280;

/// How many pending tasks the public queue query lists.
pub const PENDING_LOGS_LIMIT: usize = 100;

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
    /// The part of `total_collected_fees` that sits on the ICP ledger: the
    /// fees of tasks whose deposit came in on ICP. A task deposited on
    /// another chain leaves its fee there, so only this part can be withdrawn
    /// through the ledger without eating into what backs the other chains.
    #[serde(default)]
    pub icp_collected_fees: u128,
    /// Whether `icp_collected_fees` was recovered from the archive already.
    /// Kept apart from the counter because a share of zero is a valid result.
    #[serde(default)]
    pub icp_collected_fees_migrated: bool,
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
    pub icp_collected_fees: u128,
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
            icp_collected_fees: s.icp_collected_fees,
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
            icp_collected_fees: 0,
            icp_collected_fees_migrated: false,
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

#[derive(Clone, CandidType, Debug, Serialize, Deserialize)]
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

/// The point past which a transaction can never be included any more.
#[derive(Clone, CandidType, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TxDeadline {
    /// EVM: the nonce it spends. Once the sender's nonce has moved past it
    /// without it being mined, another transaction took its place.
    Nonce(u64),
    /// Solana: the last block height its blockhash is valid at.
    BlockHeight(u64),
}

/// What a round needs to know about a transaction besides its hash: how to
/// tell that it is dead, and how to broadcast it again while it is not.
#[derive(Clone, CandidType, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxMeta {
    pub deadline: TxDeadline,
    /// The signed transaction, kept while it is unconfirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<ByteBuf>,
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
    /// The error is the task's own and will not clear by itself: an
    /// administrator has to retry or close the task. It blocks nothing else
    /// meanwhile.
    #[serde(default)]
    pub stuck: bool,
    /// When the payout was first attempted, in ms, or 0. The ledger dedup key
    /// of an ICP payout is built from it, so every attempt shares it.
    #[serde(default)]
    pub payout_started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_meta: Option<TxMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_meta: Option<TxMeta>,
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
    #[serde(default, rename = "st", alias = "stuck")]
    pub stuck: bool,
    #[serde(default, rename = "ps", alias = "payout_started_at")]
    pub payout_started_at: u64,
    #[serde(
        default,
        rename = "fm",
        alias = "from_meta",
        skip_serializing_if = "Option::is_none"
    )]
    pub from_meta: Option<TxMeta>,
    #[serde(
        default,
        rename = "tm",
        alias = "to_meta",
        skip_serializing_if = "Option::is_none"
    )]
    pub to_meta: Option<TxMeta>,
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
            stuck: log.stuck,
            payout_started_at: log.payout_started_at,
            from_meta: log.from_meta,
            to_meta: log.to_meta,
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
            stuck: log.stuck,
            payout_started_at: log.payout_started_at,
            from_meta: log.from_meta,
            to_meta: log.to_meta,
        }
    }
}

impl BridgeLog {
    pub fn is_finalized(&self) -> bool {
        self.from_tx.is_finalized() && self.to_tx.as_ref().is_some_and(|tx| tx.is_finalized())
    }

    /// Whether the payout has been handed to a chain and not confirmed yet.
    pub fn payout_in_flight(&self) -> bool {
        self.to_tx.as_ref().is_some_and(|tx| !tx.is_finalized())
    }

    /// A transient error: a provider or ledger problem the next round retries.
    /// Only these gate a chain and count towards the circuit breaker.
    pub fn has_transient_error(&self) -> bool {
        self.error.is_some() && !self.stuck
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

pub struct ActiveBridgeUserGuard(Principal);

impl Drop for ActiveBridgeUserGuard {
    fn drop(&mut self) {
        ACTIVE_BRIDGE_USERS.with_borrow_mut(|users| {
            users.remove(&self.0);
        });
    }
}

/// Serialises the calls that sign with a user's derived keys. Two of them in
/// flight at once would read the same nonce and sign two transactions that
/// can only replace each other.
pub fn acquire_active_bridge_user(user: Principal) -> Result<ActiveBridgeUserGuard, String> {
    ACTIVE_BRIDGE_USERS.with_borrow_mut(|users| {
        if users.insert(user) {
            Ok(ActiveBridgeUserGuard(user))
        } else {
            Err("another request that signs for this user is in progress".to_string())
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

/// Delay before the next round after one with a transient error: a growing
/// backoff, then the cooldown once the circuit breaker has tripped. The rounds
/// never stop by themselves, so an outage that ends on its own is recovered
/// from without anyone's help.
fn error_backoff_secs(error_rounds: u64) -> u64 {
    if error_rounds >= MAX_ERROR_ROUNDS {
        ERROR_COOLDOWN_SECS
    } else {
        5_u64.saturating_mul(error_rounds)
    }
}

/// Releases the round lock and re-arms the timer if the round traps.
///
/// A trap rolls the callback's state back, so the lock stays set and the
/// timer slot, taken when the round started, stays empty: without this
/// nothing would run another round until a deposit or an administrator
/// scheduled one. The cleanup that follows a trap drops the round's locals,
/// this guard among them.
struct RoundGuard {
    run_generation: u64,
}

impl Drop for RoundGuard {
    fn drop(&mut self) {
        if !ic_cdk::futures::is_recovering_from_trap() {
            return;
        }
        let current = FINALIZE_RUN_GENERATION.with(Cell::get);
        let delay = STATE.with_borrow_mut(|s| {
            if !finalize_run_matches(self.run_generation, current, s.finalize_bridging_round.1) {
                return None;
            }
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
            s.error_rounds = s.error_rounds.saturating_add(1);
            Some(error_backoff_secs(s.error_rounds).max(ROUND_TRAP_RETRY_SECS))
        });
        if let Some(delay) = delay {
            ic_cdk::api::debug_print(
                "a finalization round trapped; its lock is released and the next round is scheduled",
            );
            state::schedule_finalize(Duration::from_secs(delay));
        }
    }
}

/// Whether the sender of a transaction is checked to be able to pay for it
/// before a threshold signature is spent on it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Funding {
    /// A user's derived address. Its balances are read first: signing costs
    /// the canister real cycles, and anyone can ask for a signature, so
    /// without the check an empty address could drain the canister.
    Verify,
    /// The bridge's own address, paying out a deposit a round has confirmed.
    Trusted,
}

/// Where a transaction the bridge is waiting on has got to.
enum TxStatus<C> {
    /// Not final. `seen` is whether any provider has it at all: one that
    /// nobody has can be broadcast again.
    Pending { seen: bool },
    /// Executed successfully and deep enough in the chain.
    Confirmed(C),
    /// Executed and failed. It moved nothing, and it burned its fee.
    Failed(String),
    /// Can never execute any more: replaced, or expired.
    Dead(String),
}

/// Why a round could not advance a task.
enum TaskFault {
    /// A provider or a ledger did not answer, or answered inconsistently. The
    /// next round tries again, and the chain is gated meanwhile.
    Transient(String),
    /// The task itself cannot proceed: its payout is refused on chain, or its
    /// deposit did not deliver what it claimed. It waits for an administrator
    /// and gates nothing.
    Stuck(String),
    /// Its deposit never delivered anything and never will: archive it.
    Abandon(String),
}

impl From<String> for TaskFault {
    fn from(err: String) -> Self {
        Self::Transient(err)
    }
}

/// What a finalization round decided to do with a pending task.
enum TaskOutcome {
    /// Keep working on it: write the updated task back into the queue.
    Retained(BridgeLog),
    /// Its incoming transfer provably delivered nothing, so the bridge received
    /// nothing and owes nothing. Archive it and drop it from the queue.
    Abandoned(BridgeLog),
}

impl TaskOutcome {
    fn into_log(self) -> BridgeLog {
        match self {
            Self::Retained(log) | Self::Abandoned(log) => log,
        }
    }
}

/// A payout as a round records it on its task before broadcasting it.
type Payout = (BridgeTx, TxMeta);

/// The payout a task carries: the metadata is missing on tasks recorded by a
/// version that did not keep it.
type PayoutRecord = (BridgeTx, Option<TxMeta>);

/// Result of atomically reserving the outgoing transaction slot of a pending
/// task before handing a signed transaction to an external provider.
enum PayoutClaim {
    /// This round filled the empty slot and is the only one allowed to broadcast
    /// the candidate transaction.
    Claimed,
    /// Another (possibly stale-overlapping) round filled the slot first. Reuse
    /// that transaction and never broadcast the candidate.
    Existing(PayoutRecord),
    /// This round lost the stale-lock race before it could reserve the slot.
    RunSuperseded,
    /// The task was finalized or removed while this round was building its
    /// candidate transaction.
    TaskGone,
}

fn claim_payout_in(
    pending: &mut VecDeque<BridgeLog>,
    from_tx: &BridgeTx,
    candidate: &Payout,
    now_ms: u64,
) -> PayoutClaim {
    let Some(task) = pending.iter_mut().find(|task| task.from_tx == *from_tx) else {
        return PayoutClaim::TaskGone;
    };

    match &task.to_tx {
        Some(existing) => PayoutClaim::Existing((existing.clone(), task.to_meta.clone())),
        None => {
            task.to_tx = Some(candidate.0.clone());
            task.to_meta = Some(candidate.1.clone());
            if task.payout_started_at == 0 {
                task.payout_started_at = now_ms;
            }
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
    candidate: &Payout,
    now_ms: u64,
) -> PayoutClaim {
    if !finalize_run_matches(run_generation, current, running) {
        return PayoutClaim::RunSuperseded;
    }
    claim_payout_in(pending, from_tx, candidate, now_ms)
}

fn claim_pending_payout(
    run_generation: u64,
    from_tx: &BridgeTx,
    candidate: &Payout,
    now_ms: u64,
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
            now_ms,
        )
    })
}

/// Records when a task's ICP payout was first attempted and returns it: the
/// ledger's dedup key is built from it, so a repeated attempt has to carry the
/// same one. `None` when the task is gone.
fn claim_icp_payout_in(
    pending: &mut VecDeque<BridgeLog>,
    from_tx: &BridgeTx,
    now_ms: u64,
) -> Option<u64> {
    let task = pending.iter_mut().find(|task| task.from_tx == *from_tx)?;
    if task.payout_started_at == 0 {
        task.payout_started_at = now_ms;
    }
    Some(task.payout_started_at)
}

fn claim_icp_payout(run_generation: u64, from_tx: &BridgeTx, now_ms: u64) -> Option<u64> {
    let current = FINALIZE_RUN_GENERATION.with(Cell::get);
    STATE.with_borrow_mut(|state| {
        if !finalize_run_matches(run_generation, current, state.finalize_bridging_round.1) {
            return None;
        }
        claim_icp_payout_in(&mut state.pending, from_tx, now_ms)
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum BlockTag {
    Latest,
    Finalized,
}

/// RPC values shared by every task in one finalization round.
///
/// Several receipts on the same EVM chain need the same block height.
/// Keeping one async slot per chain and tag prevents concurrent tasks from
/// paying for duplicate outcalls while still allowing different chains to
/// progress in parallel. Errors are cached for the round too: the slot's lock
/// is held across the fetch, so if the leader's full provider sweep just
/// failed, the tasks parked behind it inherit that failure instead of each
/// serially repeating the same sweep — which could otherwise stretch a round
/// toward the stale-lock takeover.
type BlockSlot = Rc<futures::lock::Mutex<Option<Result<u64, String>>>>;
type BlockCache = Rc<RefCell<HashMap<(String, BlockTag), BlockSlot>>>;

#[derive(Clone, Default)]
struct FinalizeContext {
    evm_blocks: BlockCache,
}

impl FinalizeContext {
    async fn evm_block_number<H: HttpOutcall>(
        &self,
        chain: &str,
        tag: BlockTag,
        client: &EvmClient<H>,
    ) -> Result<u64, String> {
        let slot = self
            .evm_blocks
            .borrow_mut()
            .entry((chain.to_string(), tag))
            .or_insert_with(|| Rc::new(futures::lock::Mutex::new(None)))
            .clone();
        let mut cached = slot.lock().await;
        if let Some(result) = cached.clone() {
            return result;
        }

        let result = match tag {
            BlockTag::Latest => client.block_number().await,
            BlockTag::Finalized => client.finalized_block_number().await,
        };
        *cached = Some(result.clone());
        result
    }
}

/// Picks the tasks a round works on.
///
/// Every EVM payout spends from the bridge's one address per chain, so a
/// round takes at most one task per destination chain to keep their nonces
/// apart, and a task whose payout is already in flight on a chain is the one
/// that gets that chain: it holds the nonce the next payout follows. Stuck
/// tasks wait for an administrator and are skipped. Beyond that the queue is
/// served in order, and `rotate_processed` moves what a round worked on to
/// the back, so a deposit that never confirms cannot starve the queue behind
/// it.
fn select_round_tasks(pending: &VecDeque<BridgeLog>, limit: usize) -> Vec<BridgeLog> {
    let mut tasks: Vec<BridgeLog> = Vec::with_capacity(limit);
    let mut evm_locked: HashSet<&str> = HashSet::new();

    for task in pending.iter().filter(|task| !task.stuck) {
        if let BridgeTarget::Evm(chain) = &task.to
            && task.payout_in_flight()
            && evm_locked.insert(chain.as_str())
        {
            tasks.push(task.clone());
            if tasks.len() == limit {
                return tasks;
            }
        }
    }

    for task in pending.iter().filter(|task| !task.stuck) {
        if tasks.iter().any(|picked| picked.from_tx == task.from_tx) {
            continue;
        }
        if let BridgeTarget::Evm(chain) = &task.to
            && !evm_locked.insert(chain.as_str())
        {
            continue;
        }
        tasks.push(task.clone());
        if tasks.len() == limit {
            break;
        }
    }
    tasks
}

/// Moves the tasks a round worked on behind the ones it did not, keeping the
/// order within each group.
fn rotate_processed(pending: &mut VecDeque<BridgeLog>, processed: &[BridgeTx]) {
    let (mut untouched, mut worked): (VecDeque<BridgeLog>, VecDeque<BridgeLog>) = pending
        .drain(..)
        .partition(|task| !processed.contains(&task.from_tx));
    untouched.append(&mut worked);
    *pending = untouched;
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

/// The smallest amount, in ledger units, a chain carrying `chain_decimals` can
/// represent exactly, or `None` when the chain is at least as precise as the
/// ledger and `convert_amount` never has anything to drop.
fn chain_precision_unit(token_decimals: u8, chain_decimals: u8) -> Result<Option<u128>, String> {
    if chain_decimals >= token_decimals {
        return Ok(None);
    }
    10u128
        .checked_pow((token_decimals - chain_decimals) as u32)
        .map(Some)
        .ok_or_else(|| "exponent too large".to_string())
}

/// A deposit is made in the source chain's decimals but credited in the
/// ledger's. When the chain carries fewer decimals, an amount with more
/// precision than that would be floored on chain and credited in full.
fn check_source_precision(
    icp_amount: u128,
    token_decimals: u8,
    chain_decimals: u8,
) -> Result<(), String> {
    match chain_precision_unit(token_decimals, chain_decimals)? {
        Some(unit) if !icp_amount.is_multiple_of(unit) => Err(format!(
            "amount {icp_amount} has more precision than the source chain carries; use a multiple of {unit}"
        )),
        _ => Ok(()),
    }
}

/// The payout is converted into the destination chain's decimals, and that
/// conversion floors. A remainder the destination cannot represent would be
/// kept by the bridge rather than reaching the user, so the amount after the
/// fee has to land on the chain's grid.
fn check_payout_precision(
    payout_amount: u128,
    token_decimals: u8,
    chain_decimals: u8,
) -> Result<(), String> {
    match chain_precision_unit(token_decimals, chain_decimals)? {
        Some(unit) if !payout_amount.is_multiple_of(unit) => Err(format!(
            "amount {payout_amount} after the fee has more precision than the destination chain carries; the amount left after the fee must be a multiple of {unit}"
        )),
        _ => Ok(()),
    }
}

/// Addresses a payout must never go to: the bridge's own addresses and the
/// token's contracts, where the funds would be locked or burned, and the
/// addresses no one holds.
#[derive(Default)]
struct ForbiddenDestinations {
    evm: Vec<Address>,
    sol: Vec<Pubkey>,
    icp: Vec<Principal>,
}

/// Parses a payout destination for `target` and returns it in its canonical
/// form, or the reason it cannot be paid.
fn check_destination(
    target: &BridgeTarget,
    to_addr: Option<&str>,
    forbidden: &ForbiddenDestinations,
) -> Result<Option<String>, String> {
    let Some(to_addr) = to_addr else {
        return Ok(None);
    };
    match target {
        BridgeTarget::Icp => {
            let principal = Principal::from_text(to_addr)
                .map_err(|_| format!("invalid ICP address {to_addr}"))?;
            if forbidden.icp.contains(&principal) {
                return Err(format!("{to_addr} cannot receive a payout"));
            }
            Ok(Some(principal.to_text()))
        }
        BridgeTarget::Evm(_) => {
            let address = parse_evm_address(to_addr)?;
            if forbidden.evm.contains(&address) {
                return Err(format!("{to_addr} cannot receive a payout"));
            }
            Ok(Some(address.to_checksum(None)))
        }
        BridgeTarget::Sol => {
            let pubkey =
                Pubkey::from_str(to_addr).map_err(|_| format!("invalid SOL address {to_addr}"))?;
            if forbidden.sol.contains(&pubkey) {
                return Err(format!("{to_addr} cannot receive a payout"));
            }
            Ok(Some(pubkey.to_string()))
        }
    }
}

/// The fees of the archived tasks whose deposit came in on ICP.
fn icp_fee_share(logs: impl Iterator<Item = BridgeLogLocal>) -> u128 {
    logs.filter(|log| {
        log.from == BridgeTarget::Icp
            && log.from_tx.is_finalized()
            && log.to_tx.as_ref().is_some_and(|tx| tx.is_finalized())
    })
    .fold(0u128, |sum, log| sum.saturating_add(log.fee))
}

pub mod state {
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
    /// Every user address is derived from these, so a failure is logged and
    /// retried on the next upgrade or `admin_init_public_keys` rather than
    /// trapping the install. Until a key is there, bridging that needs it is
    /// refused.
    pub async fn init_public_keys() {
        let key_name = STATE.with_borrow(|s| s.key_name.clone());
        init_ecdsa_public_key(key_name.clone()).await;
        init_ed25519_public_key(key_name).await;
    }

    /// Fetches whichever master key is still missing; a no-op once both are
    /// there.
    pub async fn try_init_public_keys() {
        let (key_name, ecdsa_missing, ed25519_missing) = STATE.with_borrow(|s| {
            (
                s.key_name.clone(),
                s.ecdsa_public_key.public_key.is_empty(),
                s.ed25519_public_key.public_key.is_empty(),
            )
        });

        if ecdsa_missing {
            init_ecdsa_public_key(key_name.clone()).await;
        }
        if ed25519_missing {
            init_ed25519_public_key(key_name).await;
        }
    }

    async fn init_ecdsa_public_key(key_name: String) {
        match ecdsa_public_key(key_name, vec![]).await {
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

    /// Fills in `icp_collected_fees` from the archive on the first upgrade
    /// that has the counter.
    ///
    /// The flag, not the counter, records that this ran: a share of zero is a
    /// legitimate answer — fees may have been introduced after the ICP-side
    /// tasks completed — and without the flag every later upgrade would read
    /// the whole archive again, which grows without bound.
    pub fn migrate_icp_collected_fees() -> u128 {
        let needed = STATE.with_borrow(|s| !s.icp_collected_fees_migrated);
        if !needed {
            return 0;
        }
        let fees = BRIDGE_LOGS.with_borrow(|logs| icp_fee_share(logs.iter()));
        STATE.with_borrow_mut(|s| {
            s.icp_collected_fees = fees;
            s.icp_collected_fees_migrated = true;
        });
        fees
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
                DefaultHttpOutcall,
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
        STATE.with_borrow(|s| SvmClient::new(s.svm_providers.clone(), DefaultHttpOutcall))
    }

    fn forbidden_destinations(s: &State, target: &BridgeTarget) -> ForbiddenDestinations {
        match target {
            BridgeTarget::Icp => ForbiddenDestinations {
                icp: vec![Principal::anonymous(), s.icp_address, s.token_ledger],
                ..Default::default()
            },
            BridgeTarget::Evm(chain) => {
                let mut evm = vec![Address::ZERO, s.evm_address];
                if let Some((contract, _, _)) = s.evm_token_contracts.get(chain) {
                    evm.push(*contract);
                }
                ForbiddenDestinations {
                    evm,
                    ..Default::default()
                }
            }
            BridgeTarget::Sol => ForbiddenDestinations {
                sol: vec![
                    Pubkey::default(),
                    s.svm_address,
                    s.svm_token_address.0,
                    s.svm_token_address.2,
                ],
                ..Default::default()
            },
        }
    }

    /// Parses a payout destination for `target` and returns it in canonical
    /// form, refusing the addresses no payout should ever go to.
    pub fn validate_destination(
        s: &State,
        target: &BridgeTarget,
        to_addr: Option<&str>,
    ) -> Result<Option<String>, String> {
        check_destination(target, to_addr, &forbidden_destinations(s, target))
    }

    /// The target a chain name denotes, if the bridge serves it.
    pub fn parse_target(s: &State, chain: &str) -> Result<BridgeTarget, String> {
        match chain {
            "ICP" => Ok(BridgeTarget::Icp),
            "SOL" => {
                if s.svm_token_address.0 == Pubkey::default() {
                    return Err("SOL token is not supported".to_string());
                }
                Ok(BridgeTarget::Sol)
            }
            _ => {
                if !s.evm_token_contracts.contains_key(chain) {
                    return Err(format!("chain {chain} not found or not supported"));
                }
                Ok(BridgeTarget::Evm(chain.to_string()))
            }
        }
    }

    /// Whether the master key `target` derives its addresses from is there.
    fn check_keys_for(s: &State, target: &BridgeTarget) -> Result<(), String> {
        let missing = match target {
            BridgeTarget::Icp => false,
            BridgeTarget::Evm(_) => s.ecdsa_public_key.public_key.is_empty(),
            BridgeTarget::Sol => s.ed25519_public_key.public_key.is_empty(),
        };
        if missing {
            Err(format!(
                "the bridge's {} key is not initialised yet, please retry later",
                target.name()
            ))
        } else {
            Ok(())
        }
    }

    fn chain_decimals(s: &State, target: &BridgeTarget) -> Option<u8> {
        match target {
            BridgeTarget::Icp => None,
            BridgeTarget::Evm(chain) => s.evm_token_contracts.get(chain).map(|c| c.1),
            BridgeTarget::Sol => Some(s.svm_token_address.1),
        }
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
        let now_ms = now_ms();
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

    /// A bridging request that passed every check and can be carried out.
    struct BridgePlan {
        from: BridgeTarget,
        to: BridgeTarget,
        to_addr: Option<String>,
        token_ledger: Principal,
        fee: u128,
    }

    fn plan_bridge(
        s: &State,
        from_chain: &str,
        to_chain: &str,
        icp_amount: u128,
        to_addr: Option<&str>,
        user: Principal,
    ) -> Result<BridgePlan, String> {
        if s.error_rounds >= MAX_ERROR_ROUNDS {
            return Err("the bridge is paused after repeated errors and retries by itself, please try again later".to_string());
        }

        if icp_amount < s.min_threshold_to_bridge {
            return Err(format!(
                "amount {} is below the minimum threshold to bridge {}",
                icp_amount, s.min_threshold_to_bridge
            ));
        }
        let payout_amount = bridge_amount_after_fee(icp_amount, s.token_bridge_fee)?;

        let from = parse_target(s, from_chain).map_err(|err| format!("from_chain: {err}"))?;
        let to = parse_target(s, to_chain).map_err(|err| format!("to_chain: {err}"))?;
        check_keys_for(s, &from)?;
        check_keys_for(s, &to)?;
        if let Some(decimals) = chain_decimals(s, &from) {
            check_source_precision(icp_amount, s.token_decimals, decimals)?;
        }
        if let Some(decimals) = chain_decimals(s, &to) {
            check_payout_precision(payout_amount, s.token_decimals, decimals)?;
        }
        let to_addr = validate_destination(s, &to, to_addr)?;

        for log in s.pending.iter() {
            // A chain whose providers are failing blocks new tasks for
            // everyone: the deposits already in are waiting on it, and letting
            // more in behind them only deepens the hole. A task that is stuck
            // on its own account does not.
            if let Some(err) = &log.error
                && log.has_transient_error()
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

        Ok(BridgePlan {
            from,
            to,
            to_addr,
            token_ledger: s.token_ledger,
            fee: s.token_bridge_fee,
        })
    }

    /// A signed deposit, recorded on its task before it is broadcast so that
    /// a broadcast whose outcome is unknown cannot strand the user's funds.
    enum Deposit {
        /// Transferred on the ICP ledger: nothing left to broadcast.
        Settled(BridgeTx),
        Evm {
            tx: BridgeTx,
            meta: TxMeta,
            client: EvmClient<DefaultHttpOutcall>,
        },
        Sol {
            tx: BridgeTx,
            meta: TxMeta,
            client: SvmClient<DefaultHttpOutcall>,
        },
    }

    impl Deposit {
        fn record(&self) -> (BridgeTx, Option<TxMeta>) {
            match self {
                Self::Settled(tx) => (tx.clone(), None),
                Self::Evm { tx, meta, .. } | Self::Sol { tx, meta, .. } => {
                    (tx.clone(), Some(meta.clone()))
                }
            }
        }

        async fn broadcast(self) -> Result<(), String> {
            match self {
                Self::Settled(_) => Ok(()),
                Self::Evm { meta, client, .. } => client
                    .send_raw_transaction(evm_raw_hex(&meta)?)
                    .await
                    .map(|_| ()),
                Self::Sol { meta, client, .. } => {
                    client.send_transaction(svm_raw(&meta)?).await.map(|_| ())
                }
            }
        }
    }

    fn evm_raw_hex(meta: &TxMeta) -> Result<String, String> {
        meta.raw
            .as_ref()
            .map(|raw| Bytes::copy_from_slice(raw).to_string())
            .ok_or_else(|| "no signed transaction to broadcast".to_string())
    }

    fn svm_raw(meta: &TxMeta) -> Result<ByteBufB64, String> {
        meta.raw
            .as_ref()
            .map(|raw| ByteBufB64::from(raw.to_vec()))
            .ok_or_else(|| "no signed transaction to broadcast".to_string())
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
        let _signing = acquire_active_bridge_user(user)?;

        let plan = STATE.with_borrow(|s| {
            plan_bridge(
                s,
                &from_chain,
                &to_chain,
                icp_amount,
                to_addr.as_deref(),
                user,
            )
        })?;

        let deposit = match &plan.from {
            BridgeTarget::Icp => {
                Deposit::Settled(from_icp(plan.token_ledger, user, icp_amount).await?)
            }
            BridgeTarget::Evm(chain) => sign_evm_deposit(chain, user, icp_amount, now_ms)
                .await
                .map_err(|err| format!("{chain}: {err}"))?,
            BridgeTarget::Sol => sign_svm_deposit(user, icp_amount)
                .await
                .map_err(|err| format!("SOL: {err}"))?,
        };

        let (from_tx, from_meta) = deposit.record();
        let from_name = plan.from.name().to_string();
        STATE.with_borrow_mut(|s| {
            s.idle_rounds = 0;
            s.pending.push_back(BridgeLog {
                id: None,
                user,
                from: plan.from,
                to: plan.to,
                icp_amount,
                fee: plan.fee,
                from_tx: from_tx.clone(),
                to_tx: None,
                to_addr: plan.to_addr,
                created_at: now_ms,
                finalized_at: 0,
                error: None,
                stuck: false,
                payout_started_at: 0,
                from_meta,
                to_meta: None,
            });
        });

        // A deposit on another chain takes a few seconds to be mined; an ICP
        // one is already final.
        let delay = if matches!(deposit, Deposit::Settled(_)) {
            0
        } else {
            5
        };
        schedule_finalize(Duration::from_secs(delay));

        if let Err(err) = deposit.broadcast().await {
            // The task is recorded already, so the funds cannot be stranded:
            // the rounds broadcast the signed transaction again until it lands
            // or provably never will.
            let note = format!("{from_name}: broadcast failed: {err}");
            ic_cdk::api::debug_print(&note);
            STATE.with_borrow_mut(|s| {
                if let Some(task) = s.pending.iter_mut().find(|t| t.from_tx == from_tx) {
                    task.error = Some(note);
                }
            });
        }

        Ok(from_tx)
    }

    async fn sign_evm_deposit(
        chain: &str,
        user: Principal,
        icp_amount: u128,
        now_ms: u64,
    ) -> Result<Deposit, String> {
        let bridge_addr = STATE.with_borrow(|s| s.evm_address);
        let (client, signed_tx) = build_erc20_transfer_tx(
            chain,
            &user,
            &bridge_addr,
            icp_amount,
            now_ms,
            Funding::Verify,
        )
        .await?;
        let tx_hash: [u8; 32] = (*signed_tx.hash()).into();
        Ok(Deposit::Evm {
            tx: BridgeTx::Evm(false, tx_hash.into()),
            meta: TxMeta {
                deadline: TxDeadline::Nonce(signed_tx.tx().nonce),
                raw: Some(ByteBuf::from(signed_tx.encoded_2718())),
            },
            client,
        })
    }

    async fn sign_svm_deposit(user: Principal, icp_amount: u128) -> Result<Deposit, String> {
        let bridge_addr = STATE.with_borrow(|s| s.svm_address);
        let (client, signed_tx, last_valid_block_height) =
            build_spl_transfer_tx(&user, &bridge_addr, icp_amount, Funding::Verify).await?;
        let signature: [u8; 64] = signed_tx.signatures[0].into();
        let raw = bincode::serialize(&signed_tx).map_err(|err| err.to_string())?;
        Ok(Deposit::Sol {
            tx: BridgeTx::Sol(false, signature.into()),
            meta: TxMeta {
                deadline: TxDeadline::BlockHeight(last_valid_block_height),
                raw: Some(ByteBuf::from(raw)),
            },
            client,
        })
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
        let started_at = now_ms();
        let tasks = STATE.with_borrow_mut(|s| {
            if !finalize_lock_available(
                s.finalize_bridging_round.1,
                s.finalize_bridging_started_at,
                started_at,
            ) {
                // already running
                return None;
            }

            let tasks = select_round_tasks(&s.pending, ROUND_TASK_LIMIT);
            if tasks.is_empty() {
                return None;
            }

            s.finalize_bridging_round.1 = true;
            s.finalize_bridging_started_at = started_at;
            Some(tasks)
        });
        let Some(tasks) = tasks else {
            return;
        };

        let run_generation = next_finalize_run_generation();
        let _guard = RoundGuard { run_generation };
        // Progress must be measured against how far each task had got when
        // this round cloned it: `claim_pending_payout` writes a broadcast
        // payout into the live task before the round merges, so comparing
        // against the live queue at merge time would no longer see that
        // transition and the round would be miscounted as idle.
        let baselines: Vec<_> = tasks.iter().map(task_progress).collect();
        let processed: Vec<BridgeTx> = tasks.iter().map(|task| task.from_tx.clone()).collect();
        let outcomes = try_finalize_tasks(tasks, run_generation).await;
        let now_ms = now_ms();
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
            for (outcome, baseline) in outcomes.into_iter().zip(baselines) {
                if let TaskOutcome::Abandoned(task) = outcome {
                    // The deposit delivered nothing: nothing is owed. Archiving
                    // instead of erroring keeps the queue — and therefore the
                    // whole chain — clear for everyone else. The amount and
                    // the fee stay out of the totals: nothing bridged.
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
                has_error = has_error || task.has_transient_error();
                has_progress = has_progress || baseline != task_progress(&task);
                if let Some(t) = s.pending.iter_mut().find(|t| t.same_with(&task)) {
                    *t = task;
                    if t.to_tx.as_ref().is_some_and(|tx| tx.is_finalized()) {
                        t.error = None;
                        t.stuck = false;
                        t.finalized_at = now_ms;
                        t.from_meta = None;
                        t.to_meta = None;
                        s.total_bridged_tokens =
                            s.total_bridged_tokens.saturating_add(t.icp_amount);
                        s.total_collected_fees = s.total_collected_fees.saturating_add(t.fee);
                        if t.from == BridgeTarget::Icp {
                            s.icp_collected_fees = s.icp_collected_fees.saturating_add(t.fee);
                        }

                        archive_bridge_log(t).expect("failed to append to BRIDGE_LOGS");
                    }
                }
            }

            s.pending.retain(|t| !abandoned.contains(&t.from_tx));
            s.pending.retain(|t| !t.is_finalized());
            rotate_processed(&mut s.pending, &processed);
            s.finalize_bridging_round = (s.finalize_bridging_round.0.saturating_add(1), false);
            s.finalize_bridging_started_at = 0;

            let next_delay = if s.pending.iter().all(|t| t.stuck) {
                // nothing a round can do: the queue is empty, or every task in
                // it waits for an administrator
                None
            } else if has_error {
                s.idle_rounds = 0;
                s.error_rounds = s.error_rounds.saturating_add(1);
                Some(error_backoff_secs(s.error_rounds))
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
    /// Once `error_rounds` reaches `MAX_ERROR_ROUNDS` new tasks are refused
    /// and the rounds slow to a cooldown; this lifts both at once instead of
    /// waiting for a clean cooldown round.
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
        let now_ms = now_ms();
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

    /// The target and canonical address a retry would redirect a task to, or
    /// `None` when it leaves the target alone.
    ///
    /// Vetted the way a `bridge()` call's destination is: the chain has to be
    /// one the bridge serves, its master key has to be there, and the address
    /// has to be one a payout may go to. `admin_retry_bridging_task` and its
    /// validate twin both go through here so a proposal cannot pass validation
    /// and then fail when it executes.
    pub fn plan_retry_redirect(
        s: &State,
        task: &BridgeLog,
        to: Option<&BridgeTarget>,
        to_addr: Option<&str>,
    ) -> Result<Option<(BridgeTarget, Option<String>)>, String> {
        if to.is_none() && to_addr.is_none() {
            return Ok(None);
        }
        let target = match to {
            Some(target) => parse_target(s, target.name())?,
            None => task.to.clone(),
        };
        check_keys_for(s, &target)?;
        let to_addr = validate_destination(s, &target, to_addr)?;
        Ok(Some((target, to_addr)))
    }

    /// Clears the outgoing transaction and the error of a task so that the
    /// next finalization round pays it out afresh, optionally to a different
    /// target: a corrected address, the user's own address on the same chain
    /// (no address), or the chain the deposit came from — a refund.
    ///
    /// The caller must have verified on chain that the recorded outgoing
    /// transaction moved no funds — a reverted EVM transaction, or a Solana
    /// transaction whose blockhash expired without landing. Retrying a payout
    /// that did go through pays the recipient twice.
    pub fn retry_pending_task(
        from_tx: &BridgeTx,
        to: Option<BridgeTarget>,
        to_addr: Option<String>,
    ) -> Result<BridgeLog, String> {
        let stale_running = ensure_finalize_bridging_idle()?;

        let redirect = STATE.with_borrow(|s| {
            let task = s
                .pending
                .iter()
                .find(|t| t.from_tx == *from_tx)
                .ok_or_else(|| "no pending bridging task matches the given from_tx".to_string())?;
            plan_retry_redirect(s, task, to.as_ref(), to_addr.as_deref())
        })?;

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

            if let Some((target, to_addr)) = redirect {
                task.to = target;
                task.to_addr = to_addr;
            }
            task.to_tx = None;
            task.to_meta = None;
            task.error = None;
            task.stuck = false;
            task.payout_started_at = 0;
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
    /// the totals. Settling with the user is left to the administrator — or
    /// better, retry the task with a refund target instead of closing it.
    ///
    /// A task whose payout has been broadcast but not confirmed is refused
    /// unless `force` is set: the payout may still land, and settling it by
    /// hand as well would pay the recipient twice. An ICP payout is only ever
    /// recorded once the ledger has answered, so before settling an ICP-bound
    /// task check the ledger for the transfer this task would have made (its
    /// dedup key: `payout_started_at` and the memo derived from `from_tx`).
    pub fn close_pending_task(
        from_tx: &BridgeTx,
        now_ms: u64,
        force: bool,
    ) -> Result<BridgeLog, String> {
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
            if s.pending[idx].payout_in_flight() && !force {
                return Err("the payout has been broadcast and is not confirmed yet; closing the task now could pay the recipient twice. Retry it instead, or close it with force once it is certain the payout can no longer land".to_string());
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
        let now_ms = now_ms();
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
        let rt = async {
            if !settle_deposit(&mut task, &context, now_ms).await? {
                return Ok(());
            }
            if !finalize_run_is_current(run_generation) {
                return Ok(());
            }
            settle_payout(&mut task, &context, now_ms, run_generation).await
        }
        .await;

        match rt {
            Ok(()) => {
                task.error = None;
                task.stuck = false;
                TaskOutcome::Retained(task)
            }
            Err(TaskFault::Transient(err)) => {
                ic_cdk::api::debug_print(format!("finalize_tasks failed: {err}"));
                task.error = Some(err);
                task.stuck = false;
                TaskOutcome::Retained(task)
            }
            Err(TaskFault::Stuck(err)) => {
                ic_cdk::api::debug_print(format!("bridging task is stuck: {err}"));
                task.error = Some(err);
                task.stuck = true;
                TaskOutcome::Retained(task)
            }
            Err(TaskFault::Abandon(err)) => {
                task.error = Some(err);
                task.stuck = false;
                task.finalized_at = now_ms;
                task.from_meta = None;
                TaskOutcome::Abandoned(task)
            }
        }
    }

    /// Advances the incoming side of a task and tells whether its deposit is
    /// in.
    async fn settle_deposit(
        task: &mut BridgeLog,
        context: &FinalizeContext,
        now_ms: u64,
    ) -> Result<bool, TaskFault> {
        match (task.from.clone(), task.from_tx.clone()) {
            (BridgeTarget::Evm(chain), BridgeTx::Evm(false, hash)) => {
                let tx_hash: TxHash = (*hash).into();
                let sender = evm_address(&task.user)
                    .map_err(|err| TaskFault::Transient(format!("{chain}: {err}")))?;
                let status = check_evm_tx(
                    context,
                    &chain,
                    &tx_hash,
                    &sender,
                    task.from_meta.as_ref(),
                    task.created_at,
                    now_ms,
                )
                .await
                .map_err(|err| TaskFault::Transient(format!("{chain}: {err}")))?;
                match status {
                    TxStatus::Confirmed(receipt) => {
                        verify_evm_deposit(&chain, task, &receipt)?;
                        task.from_tx = BridgeTx::Evm(true, hash);
                        task.from_meta = None;
                        Ok(true)
                    }
                    TxStatus::Pending { seen } => {
                        if !seen {
                            rebroadcast_evm(
                                &chain,
                                task.from_meta.as_ref(),
                                task.created_at,
                                now_ms,
                            )
                            .await;
                        }
                        Ok(false)
                    }
                    TxStatus::Failed(reason) | TxStatus::Dead(reason) => {
                        Err(TaskFault::Abandon(format!(
                            "{chain}: incoming transaction {tx_hash} {reason}, nothing was received"
                        )))
                    }
                }
            }
            (BridgeTarget::Sol, BridgeTx::Sol(false, signature)) => {
                let status =
                    check_sol_tx(&signature, task.from_meta.as_ref(), task.created_at, now_ms)
                        .await
                        .map_err(|err| TaskFault::Transient(format!("SOL: {err}")))?;
                match status {
                    TxStatus::Confirmed(()) => {
                        task.from_tx = BridgeTx::Sol(true, signature);
                        task.from_meta = None;
                        Ok(true)
                    }
                    TxStatus::Pending { seen } => {
                        if !seen {
                            rebroadcast_svm(task.from_meta.as_ref(), task.created_at, now_ms).await;
                        }
                        Ok(false)
                    }
                    TxStatus::Failed(reason) | TxStatus::Dead(reason) => Err(TaskFault::Abandon(
                        format!("SOL: incoming transaction {reason}, nothing was received"),
                    )),
                }
            }
            _ => Ok(true),
        }
    }

    /// Checks that a confirmed deposit transaction delivered the amount the
    /// task claims: a successful status only says the call did not revert,
    /// and a token that returns `false` instead, or takes a fee on transfer,
    /// would otherwise be credited in full.
    fn verify_evm_deposit(
        chain: &str,
        task: &BridgeLog,
        receipt: &EvmReceipt,
    ) -> Result<(), TaskFault> {
        let (token, expected, bridge_addr, user_addr) = STATE
            .with_borrow(|s| {
                let (contract, decimals, _) = s
                    .evm_token_contracts
                    .get(chain)
                    .ok_or_else(|| format!("chain {chain} not found"))?;
                let value = convert_amount(task.icp_amount, s.token_decimals, *decimals)?;
                let user_addr = derive_evm_address(&s.ecdsa_public_key, &task.user)?;
                Ok::<_, String>((*contract, U256::from(value), s.evm_address, user_addr))
            })
            .map_err(|err| TaskFault::Transient(format!("{chain}: {err}")))?;

        let delivered = receipt.transferred(&token, &user_addr, &bridge_addr);
        if delivered == expected {
            Ok(())
        } else {
            Err(TaskFault::Stuck(format!(
                "{chain}: incoming transaction {} delivered {delivered} token units to the bridge where {expected} were expected; an administrator must settle this task",
                receipt.transaction_hash
            )))
        }
    }

    fn record_payout(task: &mut BridgeLog, record: PayoutRecord, now_ms: u64) {
        task.to_tx = Some(record.0);
        task.to_meta = record.1;
        if task.payout_started_at == 0 {
            task.payout_started_at = now_ms;
        }
    }

    /// When the payout was first handed to a chain, for the grace period
    /// before a missing payout is chased.
    fn payout_since(task: &BridgeLog) -> u64 {
        if task.payout_started_at > 0 {
            task.payout_started_at
        } else {
            task.created_at
        }
    }

    /// Pays out a task whose deposit is in, or advances the payout it has
    /// already made.
    async fn settle_payout(
        task: &mut BridgeLog,
        context: &FinalizeContext,
        now_ms: u64,
        run_generation: u64,
    ) -> Result<(), TaskFault> {
        match (task.to.clone(), task.to_tx.clone()) {
            (BridgeTarget::Icp, None) => {
                let token_ledger = STATE.with_borrow(|s| s.token_ledger);
                let to_principal = match &task.to_addr {
                    Some(addr) => Principal::from_text(addr).map_err(|_| {
                        TaskFault::Stuck(format!("ICP: invalid to_addr principal: {addr}"))
                    })?,
                    None => task.user,
                };
                let to_amount =
                    bridge_amount_after_fee(task.icp_amount, task.fee).map_err(TaskFault::Stuck)?;
                let Some(started_at) = claim_icp_payout(run_generation, &task.from_tx, now_ms)
                else {
                    return Ok(());
                };
                task.payout_started_at = started_at;
                let dedup = (started_at, payout_memo(&task.from_tx));
                match to_icp(token_ledger, to_principal, to_amount, Some(dedup)).await {
                    Ok(to_tx) => {
                        task.to_tx = Some(to_tx);
                        Ok(())
                    }
                    Err(IcpPayoutError::TooOld) => Err(TaskFault::Stuck(
                        "ICP: the payout is older than the ledger's dedup window; an administrator must check the ledger and retry or close this task".to_string(),
                    )),
                    Err(IcpPayoutError::Other(err)) => Err(TaskFault::Transient(err)),
                }
            }
            (BridgeTarget::Evm(chain), None) => {
                let to_addr = match &task.to_addr {
                    Some(addr) => parse_evm_address(addr)
                        .map_err(|err| TaskFault::Stuck(format!("{chain}: {err}")))?,
                    None => evm_address(&task.user)
                        .map_err(|err| TaskFault::Transient(format!("{chain}: {err}")))?,
                };
                let to_amount =
                    bridge_amount_after_fee(task.icp_amount, task.fee).map_err(TaskFault::Stuck)?;
                match to_evm(
                    run_generation,
                    &task.from_tx,
                    &chain,
                    to_addr,
                    to_amount,
                    now_ms,
                )
                .await
                {
                    Ok(Some(record)) => {
                        record_payout(task, record, now_ms);
                        Ok(())
                    }
                    // This round was superseded, or a concurrent round
                    // removed the task while the transaction was built.
                    Ok(None) => Ok(()),
                    Err((claimed, err)) => {
                        if let Some(record) = claimed {
                            record_payout(task, record, now_ms);
                        }
                        Err(TaskFault::Transient(err))
                    }
                }
            }
            (BridgeTarget::Evm(chain), Some(BridgeTx::Evm(false, hash))) => {
                let tx_hash: TxHash = (*hash).into();
                let sender = STATE.with_borrow(|s| s.evm_address);
                let since = payout_since(task);
                let status = check_evm_tx(
                    context,
                    &chain,
                    &tx_hash,
                    &sender,
                    task.to_meta.as_ref(),
                    since,
                    now_ms,
                )
                .await
                .map_err(|err| TaskFault::Transient(format!("{chain}: {err}")))?;
                match status {
                    TxStatus::Confirmed(_) => {
                        task.to_tx = Some(BridgeTx::Evm(true, hash));
                        task.to_meta = None;
                        Ok(())
                    }
                    TxStatus::Pending { seen } => {
                        if !seen {
                            rebroadcast_evm(&chain, task.to_meta.as_ref(), since, now_ms).await;
                        }
                        Ok(())
                    }
                    // Unlike a failed deposit, a failed payout leaves the
                    // bridge owing the user, and rebuilding it automatically
                    // would burn gas on every attempt. Leave it for an
                    // administrator.
                    TxStatus::Failed(reason) => Err(TaskFault::Stuck(format!(
                        "{chain}: outgoing transaction {tx_hash} {reason}; an administrator must retry or close this task"
                    ))),
                    // It can never land, so a fresh one cannot pay twice.
                    TxStatus::Dead(reason) => {
                        ic_cdk::api::debug_print(format!(
                            "{chain}: outgoing transaction {tx_hash} {reason}; it is rebuilt next round"
                        ));
                        task.to_tx = None;
                        task.to_meta = None;
                        Ok(())
                    }
                }
            }
            (BridgeTarget::Sol, None) => {
                let to_addr = match &task.to_addr {
                    Some(addr) => Pubkey::from_str(addr).map_err(|_| {
                        TaskFault::Stuck(format!("SOL: invalid to_addr address: {addr}"))
                    })?,
                    None => svm_address(&task.user)
                        .map_err(|err| TaskFault::Transient(format!("SOL: {err}")))?,
                };
                let to_amount =
                    bridge_amount_after_fee(task.icp_amount, task.fee).map_err(TaskFault::Stuck)?;
                match to_svm(run_generation, &task.from_tx, to_addr, to_amount, now_ms).await {
                    Ok(Some(record)) => {
                        record_payout(task, record, now_ms);
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err((claimed, err)) => {
                        if let Some(record) = claimed {
                            record_payout(task, record, now_ms);
                        }
                        Err(TaskFault::Transient(err))
                    }
                }
            }
            (BridgeTarget::Sol, Some(BridgeTx::Sol(false, signature))) => {
                let since = payout_since(task);
                let status = check_sol_tx(&signature, task.to_meta.as_ref(), since, now_ms)
                    .await
                    .map_err(|err| TaskFault::Transient(format!("SOL: {err}")))?;
                match status {
                    TxStatus::Confirmed(()) => {
                        task.to_tx = Some(BridgeTx::Sol(true, signature));
                        task.to_meta = None;
                        Ok(())
                    }
                    TxStatus::Pending { seen } => {
                        if !seen {
                            rebroadcast_svm(task.to_meta.as_ref(), since, now_ms).await;
                        }
                        Ok(())
                    }
                    TxStatus::Failed(reason) => Err(TaskFault::Stuck(format!(
                        "SOL: outgoing transaction {reason}; an administrator must retry or close this task"
                    ))),
                    TxStatus::Dead(reason) => {
                        ic_cdk::api::debug_print(format!(
                            "SOL: outgoing transaction {reason}; it is rebuilt next round"
                        ));
                        task.to_tx = None;
                        task.to_meta = None;
                        Ok(())
                    }
                }
            }
            _ => Ok(()),
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

    /// Why an ICP payout did not happen.
    pub enum IcpPayoutError {
        /// Its dedup timestamp is outside the ledger's window, so the ledger
        /// can no longer tell a retry from a second payout.
        TooOld,
        Other(String),
    }

    impl fmt::Display for IcpPayoutError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::TooOld => write!(f, "ICP: the transfer is too old for the ledger"),
                Self::Other(err) => f.write_str(err),
            }
        }
    }

    /// Transfers tokens out of the bridge on the ICP side.
    ///
    /// `dedup` carries the time the payout was first attempted and the memo
    /// derived from the task's incoming transaction. Passing it makes the
    /// ledger itself reject a second transfer for the same task: a payout
    /// re-attempted by a stale-lock takeover round while the first call is
    /// still in flight — or retried after an error whose outcome was unknown
    /// — comes back as `Duplicate` instead of paying the recipient twice. A
    /// payout still failing past the ledger's ~24h dedup window fails with
    /// `TooOld` and is left for an administrator, which is the safe
    /// direction. `None` (admin fee collection) keeps the ledger's default
    /// behavior.
    pub async fn to_icp(
        token_ledger: Principal,
        to_addr: Principal,
        icp_amount: u128,
        dedup: Option<(u64, Memo)>,
    ) -> Result<BridgeTx, IcpPayoutError> {
        if icp_amount == 0 {
            return Err(IcpPayoutError::Other(
                "ICP: amount must be greater than 0".to_string(),
            ));
        }

        let (created_at_time, memo) = match dedup {
            Some((started_at_ms, memo)) => {
                (Some(started_at_ms.saturating_mul(1_000_000)), Some(memo))
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
        .map_err(|err| IcpPayoutError::Other(format!("ICP: {err}")))?;
        let res = match res {
            Ok(idx) => idx,
            // An identical transfer for this task already landed, so the payout
            // is done; adopt its block height.
            Err(TransferError::Duplicate { duplicate_of }) => duplicate_of,
            Err(TransferError::TooOld) => return Err(IcpPayoutError::TooOld),
            Err(err) => {
                return Err(IcpPayoutError::Other(format!(
                    "ICP: failed to transfer token to user, error: {:?}",
                    err
                )));
            }
        };
        let idx = u64::try_from(&res.0)
            .map_err(|_| IcpPayoutError::Other("ICP: block height too large".to_string()))?;
        Ok(BridgeTx::Icp(true, idx))
    }

    /// Broadcasts an outgoing payout transaction.
    ///
    /// A successful `Some` is the payout this task must poll. `None` means
    /// the round was superseded or the task disappeared while its candidate was
    /// being built. On failure the error's `Option` carries the payout that
    /// was atomically recorded before it was handed to the provider, if any:
    /// the provider may have accepted and propagated it even though the RPC
    /// call itself failed.
    type BroadcastResult = Result<Option<PayoutRecord>, (Option<PayoutRecord>, String)>;

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
            Funding::Trusted,
        )
        .await
        .map_err(|err| (None, format!("{chain}: {err}")))?;

        let tx_hash: [u8; 32] = (*signed_tx.hash()).into();
        let raw = signed_tx.encoded_2718();
        let payout: Payout = (
            BridgeTx::Evm(false, tx_hash.into()),
            TxMeta {
                deadline: TxDeadline::Nonce(signed_tx.tx().nonce),
                raw: Some(ByteBuf::from(raw.clone())),
            },
        );
        let data = Bytes::from(raw).to_string();
        broadcast_payout(run_generation, from_tx, payout, chain, now_ms, || {
            client.send_raw_transaction(data)
        })
        .await
    }

    async fn to_svm(
        run_generation: u64,
        from_tx: &BridgeTx,
        to_addr: Pubkey,
        icp_amount: u128,
        now_ms: u64,
    ) -> BroadcastResult {
        let (client, signed_tx, last_valid_block_height) = build_spl_transfer_tx(
            &ic_cdk::api::canister_self(),
            &to_addr,
            icp_amount,
            Funding::Trusted,
        )
        .await
        .map_err(|err| (None, format!("SOL: {err}")))?;

        let signature: [u8; 64] = signed_tx.signatures[0].into();
        let raw = bincode::serialize(&signed_tx).map_err(|err| (None, format!("SOL: {err}")))?;
        let payout: Payout = (
            BridgeTx::Sol(false, signature.into()),
            TxMeta {
                deadline: TxDeadline::BlockHeight(last_valid_block_height),
                raw: Some(ByteBuf::from(raw.clone())),
            },
        );
        broadcast_payout(run_generation, from_tx, payout, "SOL", now_ms, || {
            client.send_transaction(ByteBufB64::from(raw))
        })
        .await
    }

    /// Records the payout on its task, then hands it to the provider.
    ///
    /// The claim comes first so that a broadcast whose outcome is unknown is
    /// never rebuilt: the error carries the claimed payout for exactly that
    /// case. A claim that finds the slot taken, the round superseded or the
    /// task gone returns without broadcasting, see [`PayoutClaim`].
    async fn broadcast_payout<F, Fut, T>(
        run_generation: u64,
        from_tx: &BridgeTx,
        payout: Payout,
        chain: &str,
        now_ms: u64,
        send: F,
    ) -> BroadcastResult
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        match claim_pending_payout(run_generation, from_tx, &payout, now_ms) {
            PayoutClaim::Claimed => {}
            PayoutClaim::Existing(existing) => return Ok(Some(existing)),
            PayoutClaim::RunSuperseded | PayoutClaim::TaskGone => return Ok(None),
        }

        let (tx, meta) = payout;
        send().await.map_err(|err| {
            (
                Some((tx.clone(), Some(meta.clone()))),
                format!("{chain}: {err}"),
            )
        })?;
        Ok(Some((tx, Some(meta))))
    }

    /// Where an EVM transaction sent by `sender` has got to.
    ///
    /// A transaction no provider has is not necessarily still coming: once
    /// the sender's nonce has moved past the one it spends, it has been
    /// replaced and can never be mined. See [`EvmClient::replaced`] for how
    /// that is established without mistaking a provider's lag for it.
    async fn check_evm_tx(
        context: &FinalizeContext,
        chain: &str,
        tx_hash: &TxHash,
        sender: &Address,
        meta: Option<&TxMeta>,
        since_ms: u64,
        now_ms: u64,
    ) -> Result<TxStatus<EvmReceipt>, String> {
        let client = evm_client(chain)?;
        let receipt = client
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|err| format!("failed to get transaction receipt, error: {err}"))?;

        match receipt {
            Some(receipt) if receipt.transaction_hash == *tx_hash => {
                let Some(block_number) = receipt.block_number() else {
                    return Ok(TxStatus::Pending { seen: true });
                };
                let confirmed = if client.max_confirmations == 0 {
                    context
                        .evm_block_number(chain, BlockTag::Finalized, &client)
                        .await
                        .map_err(|err| format!("failed to get finalized block, error: {err}"))?
                        >= block_number
                } else {
                    context
                        .evm_block_number(chain, BlockTag::Latest, &client)
                        .await
                        .map_err(|err| format!("failed to get block number, error: {err}"))?
                        .saturating_sub(block_number)
                        >= client.max_confirmations
                };
                if !confirmed {
                    return Ok(TxStatus::Pending { seen: true });
                }
                // A mined-and-reverted transaction will never finalize, so it
                // must not be reported as merely unconfirmed — that polls
                // forever.
                Ok(if receipt.succeeded() {
                    TxStatus::Confirmed(receipt)
                } else {
                    TxStatus::Failed("reverted on chain".to_string())
                })
            }
            _ => {
                if now_ms.saturating_sub(since_ms) >= UNSEEN_TX_GRACE_MS
                    && let Some(TxMeta {
                        deadline: TxDeadline::Nonce(nonce),
                        ..
                    }) = meta
                    && client.replaced(sender, *nonce, tx_hash).await?
                {
                    return Ok(TxStatus::Dead(format!(
                        "was replaced: nonce {nonce} was spent by another transaction"
                    )));
                }
                Ok(TxStatus::Pending { seen: false })
            }
        }
    }

    /// Where a Solana transaction has got to.
    ///
    /// A transaction no provider has expires once the finalized block height
    /// is past the last height its blockhash is valid at. See
    /// [`SvmClient::expired`] for how that is established without mistaking a
    /// provider's lag for it.
    async fn check_sol_tx(
        signature: &[u8; 64],
        meta: Option<&TxMeta>,
        since_ms: u64,
        now_ms: u64,
    ) -> Result<TxStatus<()>, String> {
        let client = svm_client();
        let signature = SvmSignature::from(*signature).to_string();
        let status = client
            .get_signature_status(&signature)
            .await
            .map_err(|err| format!("failed to get signature status, error: {err}"))?;

        match status {
            SolTxStatus::Finalized => Ok(TxStatus::Confirmed(())),
            SolTxStatus::Failed(err) => Ok(TxStatus::Failed(format!("failed on chain: {err}"))),
            SolTxStatus::Landed => Ok(TxStatus::Pending { seen: true }),
            SolTxStatus::Unknown => {
                if now_ms.saturating_sub(since_ms) >= UNSEEN_TX_GRACE_MS
                    && let Some(TxMeta {
                        deadline: TxDeadline::BlockHeight(last_valid),
                        ..
                    }) = meta
                    && client
                        .expired(&signature, *last_valid, SOL_EXPIRY_MARGIN_BLOCKS)
                        .await?
                {
                    return Ok(TxStatus::Dead(format!(
                        "expired: its blockhash was valid until block {last_valid}"
                    )));
                }
                Ok(TxStatus::Pending { seen: false })
            }
        }
    }

    /// Hands a signed transaction no provider has seen to the providers again.
    /// Best effort: a provider that already has it answers with an error, and
    /// one that is down is tried again next round.
    async fn rebroadcast_evm(chain: &str, meta: Option<&TxMeta>, since_ms: u64, now_ms: u64) {
        if now_ms.saturating_sub(since_ms) < UNSEEN_TX_GRACE_MS {
            return;
        }
        let Some(raw) = meta.and_then(|meta| meta.raw.as_ref()) else {
            return;
        };
        let Ok(client) = evm_client(chain) else {
            return;
        };
        if let Err(err) = client
            .send_raw_transaction(Bytes::copy_from_slice(raw).to_string())
            .await
        {
            ic_cdk::api::debug_print(format!("{chain}: re-broadcast failed: {err}"));
        }
    }

    async fn rebroadcast_svm(meta: Option<&TxMeta>, since_ms: u64, now_ms: u64) {
        if now_ms.saturating_sub(since_ms) < UNSEEN_TX_GRACE_MS {
            return;
        }
        let Some(raw) = meta.and_then(|meta| meta.raw.as_ref()) else {
            return;
        };
        if let Err(err) = svm_client()
            .send_transaction(ByteBufB64::from(raw.to_vec()))
            .await
        {
            ic_cdk::api::debug_print(format!("SOL: re-broadcast failed: {err}"));
        }
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
        /// The token units an ERC-20 transfer moves, checked against the
        /// sender's balance when the sender is a user.
        token_transfer: Option<u128>,
    }

    /// Attaches the nonce, the gas price and the signature to a planned
    /// transaction, refreshing the cached gas price when it has gone stale.
    async fn sign_evm_tx(
        chain: &str,
        from: &Principal,
        plan: EvmTxPlan,
        now_ms: u64,
        funding: Funding,
    ) -> Result<(EvmClient<DefaultHttpOutcall>, Signed<TxEip1559>), String> {
        let EvmTxPlan {
            to,
            recipient,
            value,
            input,
            gas_limit,
            token_transfer,
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
            tx.nonce = client.get_transaction_count(&from_addr).await?;
        } else {
            let (nonce, gas_price, max_priority_fee_per_gas) = futures::future::try_join3(
                client.get_transaction_count(&from_addr),
                client.gas_price(),
                client.max_priority_fee_per_gas(),
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

        if funding == Funding::Verify {
            verify_evm_funds(
                &client,
                &from_addr,
                &tx,
                token_transfer.map(|amount| (to, amount)),
            )
            .await?;
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

    /// Refuses to sign for an address that cannot pay for the transaction:
    /// its native balance must cover the value and the gas, and for a token
    /// transfer its token balance must cover the amount.
    async fn verify_evm_funds(
        client: &EvmClient<DefaultHttpOutcall>,
        from: &Address,
        tx: &TxEip1559,
        token: Option<(Address, u128)>,
    ) -> Result<(), String> {
        let gas = U256::from(tx.gas_limit).saturating_mul(U256::from(tx.max_fee_per_gas));
        let needed = gas.saturating_add(tx.value);
        let balance = client.get_balance(from).await?;
        if balance < needed {
            return Err(format!(
                "address {from} holds {balance} wei, and the transaction needs {needed} for its value and gas"
            ));
        }
        if let Some((contract, amount)) = token {
            let held = client.erc20_balance_of(&contract, from).await?;
            if held < U256::from(amount) {
                return Err(format!(
                    "address {from} holds {held} token units, and the transfer needs {amount}"
                ));
            }
        }
        Ok(())
    }

    pub async fn build_erc20_transfer_tx(
        chain: &str,
        from: &Principal,
        to_addr: &Address,
        icp_amount: u128,
        now_ms: u64,
        funding: Funding,
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
                token_transfer: Some(value),
            })
        })?;

        sign_evm_tx(chain, from, plan, now_ms, funding).await
    }

    pub async fn build_evm_transfer_tx(
        chain: &str,
        from: &Principal,
        to_addr: &Address,
        amount: u128,
        now_ms: u64,
        funding: Funding,
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
            token_transfer: None,
        };

        sign_evm_tx(chain, from, plan, now_ms, funding).await
    }

    /// A signed Solana transaction, its client, and the last block height its
    /// blockhash is valid at.
    type SignedSvmTx = (SvmClient<DefaultHttpOutcall>, Transaction, u64);

    pub async fn build_spl_transfer_tx(
        from: &Principal,
        to_addr: &Pubkey,
        icp_amount: u128,
        funding: Funding,
    ) -> Result<SignedSvmTx, String> {
        let (from_addr, from_ata, to_ata, amount, ixs) = STATE.with_borrow(|s| {
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

            let from_ata =
                get_associated_token_address(&from_addr, &mint_pubkey, &token_program_id);
            let to_ata = get_associated_token_address(to_addr, &mint_pubkey, &token_program_id);
            let ix0 = create_associated_token_account_idempotent(
                &from_addr,
                to_addr,
                &mint_pubkey,
                &token_program_id,
            );
            let ix = transfer_checked_instruction(
                &token_program_id,
                &from_ata,
                &mint_pubkey,
                &to_ata,
                &from_addr,
                &[],
                amount,
                decimals,
            );

            Ok::<_, String>((from_addr, from_ata, to_ata, amount, vec![ix0, ix]))
        })?;

        let client = svm_client();
        if funding == Funding::Verify {
            let (lamports, tokens, to_ata_exists) = futures::future::try_join3(
                client.get_balance(&from_addr.to_string()),
                client.get_token_account_balance(&from_ata.to_string()),
                async {
                    client
                        .get_account_info(&to_ata.to_string())
                        .await
                        .map(|account| account.is_some())
                },
            )
            .await
            .map_err(|err| format!("failed to read the balances of {from_addr}: {err}"))?;
            if tokens < amount {
                return Err(format!(
                    "address {from_addr} holds {tokens} token units, and the transfer needs {amount}"
                ));
            }
            // the first instruction opens the recipient's token account when it
            // is missing, and the fee payer is the one who funds its rent
            let needed = if to_ata_exists {
                SOL_TX_FEE_LAMPORTS
            } else {
                SOL_TX_FEE_LAMPORTS.saturating_add(SPL_ACCOUNT_RENT_LAMPORTS)
            };
            if lamports < needed {
                return Err(format!(
                    "address {from_addr} holds {lamports} lamports, and the transaction needs {needed} for its fee{}",
                    if to_ata_exists {
                        ""
                    } else {
                        " and the recipient's new token account"
                    }
                ));
            }
        }

        sign_svm_tx(client, from, from_addr, &ixs).await
    }

    pub async fn build_sol_transfer_tx(
        from: &Principal,
        to_addr: &Pubkey,
        sol_amount: u64,
        funding: Funding,
    ) -> Result<SignedSvmTx, String> {
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

        let client = svm_client();
        if funding == Funding::Verify {
            let lamports = client.get_balance(&from_addr.to_string()).await?;
            let needed = sol_amount.saturating_add(SOL_TX_FEE_LAMPORTS);
            if lamports < needed {
                return Err(format!(
                    "address {from_addr} holds {lamports} lamports, and the transfer needs {needed} with its fee"
                ));
            }
        }

        sign_svm_tx(client, from, from_addr, &ixs).await
    }

    /// Attaches a recent blockhash and the signature of `from` to the planned
    /// instructions, with `from_addr` — the address `from` derives to — as
    /// the fee payer.
    async fn sign_svm_tx(
        client: SvmClient<DefaultHttpOutcall>,
        from: &Principal,
        from_addr: Pubkey,
        ixs: &[Instruction],
    ) -> Result<SignedSvmTx, String> {
        let key_name = STATE.with_borrow(|s| s.key_name.clone());
        let blockhash = client
            .get_latest_blockhash()
            .await
            .map_err(|err| format!("failed to get latest blockhash, error: {err}"))?;

        let message = Message::new_with_blockhash(ixs, Some(&from_addr), &blockhash.to_hash()?);
        let msg = bincode::serialize(&message).map_err(|err| err.to_string())?;
        let sig = sign_with_schnorr(key_name, vec![from.as_slice().to_vec()], msg).await?;
        let signature: [u8; 64] = sig
            .try_into()
            .map_err(|_| "invalid signature length".to_string())?;
        let transaction = Transaction {
            message,
            signatures: vec![signature.into()],
        };

        Ok((client, transaction, blockhash.last_valid_block_height))
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
    use crate::outcall::tests::{MockHttpOutcall, result};
    use ic_stable_structures::VectorMemory;

    fn principal(bytes: &[u8]) -> Principal {
        Principal::from_slice(bytes)
    }

    fn log(user: Principal, from: BridgeTarget, to: BridgeTarget, from_tx: BridgeTx) -> BridgeLog {
        BridgeLog {
            id: None,
            user,
            from,
            to,
            icp_amount: 100,
            fee: 1,
            from_tx,
            to_tx: None,
            to_addr: None,
            created_at: 1,
            finalized_at: 0,
            error: None,
            stuck: false,
            payout_started_at: 0,
            from_meta: None,
            to_meta: None,
        }
    }

    fn evm_tx(seed: u8) -> BridgeTx {
        BridgeTx::Evm(false, [seed; 32].into())
    }

    fn meta(nonce: u64) -> TxMeta {
        TxMeta {
            deadline: TxDeadline::Nonce(nonce),
            raw: Some(ByteBuf::from(vec![nonce as u8])),
        }
    }

    fn evm(chain: &str) -> BridgeTarget {
        BridgeTarget::Evm(chain.to_string())
    }

    #[test]
    fn finalization_shares_block_outcalls_per_chain_and_tag() {
        let mock = MockHttpOutcall::new(vec![result("0x2a".into()), result("0x2b".into())]);
        let client = EvmClient::new(
            vec!["https://rpc0".to_string(), "https://rpc1".to_string()],
            2,
            mock.clone(),
        );
        let context = FinalizeContext::default();

        let (first, second) = futures::executor::block_on(futures::future::join(
            context.evm_block_number("BNB", BlockTag::Latest, &client),
            context.evm_block_number("BNB", BlockTag::Latest, &client),
        ));

        assert_eq!(first, Ok(42));
        assert_eq!(second, Ok(42));
        assert_eq!(mock.urls().len(), 2);
        assert_eq!(mock.methods(), vec!["eth_blockNumber"; 2]);
    }

    #[test]
    fn finalization_shares_failed_block_outcalls_per_chain() {
        // A failed sweep is cached for the round too: tasks parked behind the
        // leader inherit its error instead of serially repeating the sweep.
        let mock = MockHttpOutcall::new(vec![
            Err("all providers down".to_string()),
            Err("all providers down".to_string()),
        ]);
        let client = EvmClient::new(
            vec!["https://rpc0".to_string(), "https://rpc1".to_string()],
            2,
            mock.clone(),
        );
        let context = FinalizeContext::default();

        let (first, second) = futures::executor::block_on(futures::future::join(
            context.evm_block_number("BNB", BlockTag::Finalized, &client),
            context.evm_block_number("BNB", BlockTag::Finalized, &client),
        ));

        assert!(first.is_err());
        assert_eq!(first, second);
        assert_eq!(mock.urls().len(), 2);
    }

    #[test]
    fn only_the_first_overlapping_round_can_claim_a_payout() {
        let from_tx = BridgeTx::Icp(true, 7);
        let first = (evm_tx(1), meta(1));
        let replacement = (evm_tx(2), meta(2));
        let mut pending = VecDeque::from([log(
            Principal::anonymous(),
            BridgeTarget::Icp,
            evm("ETH"),
            from_tx.clone(),
        )]);

        assert!(matches!(
            claim_payout_in(&mut pending, &from_tx, &first, 10),
            PayoutClaim::Claimed
        ));
        assert!(pending[0].to_tx.as_ref().is_some_and(|tx| tx == &first.0));
        assert_eq!(pending[0].to_meta, Some(first.1.clone()));
        assert_eq!(pending[0].payout_started_at, 10);

        let second = claim_payout_in(&mut pending, &from_tx, &replacement, 20);
        match second {
            PayoutClaim::Existing((existing, existing_meta)) => {
                assert!(existing == first.0);
                assert_eq!(existing_meta, Some(first.1.clone()));
            }
            _ => panic!("the stale round replaced the claimed payout"),
        }
        assert!(pending[0].to_tx.as_ref().is_some_and(|tx| tx == &first.0));
        assert_eq!(pending[0].payout_started_at, 10);

        assert!(matches!(
            claim_payout_in(&mut pending, &BridgeTx::Icp(true, 8), &replacement, 30),
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
        let candidate = (evm_tx(3), meta(3));
        let mut pending = VecDeque::from([log(
            Principal::anonymous(),
            BridgeTarget::Icp,
            evm("ETH"),
            from_tx.clone(),
        )]);

        // A run whose generation was bumped away must not touch the slot.
        assert!(matches!(
            claim_pending_payout_with(true, &mut pending, 2, 1, &from_tx, &candidate, 1),
            PayoutClaim::RunSuperseded
        ));
        assert!(pending[0].to_tx.is_none());

        // A released lock refuses the claim even for a matching generation.
        assert!(matches!(
            claim_pending_payout_with(false, &mut pending, 2, 2, &from_tx, &candidate, 1),
            PayoutClaim::RunSuperseded
        ));
        assert!(pending[0].to_tx.is_none());

        // The current generation with the lock held is the only one that can.
        assert!(matches!(
            claim_pending_payout_with(true, &mut pending, 2, 2, &from_tx, &candidate, 1),
            PayoutClaim::Claimed
        ));
        assert!(
            pending[0]
                .to_tx
                .as_ref()
                .is_some_and(|tx| tx == &candidate.0)
        );
    }

    #[test]
    fn an_icp_payout_keeps_its_first_attempt_time() {
        let from_tx = BridgeTx::Evm(true, [4; 32].into());
        let mut pending = VecDeque::from([log(
            Principal::anonymous(),
            evm("BNB"),
            BridgeTarget::Icp,
            from_tx.clone(),
        )]);

        assert_eq!(claim_icp_payout_in(&mut pending, &from_tx, 100), Some(100));
        assert_eq!(claim_icp_payout_in(&mut pending, &from_tx, 200), Some(100));
        assert_eq!(claim_icp_payout_in(&mut pending, &evm_tx(9), 300), None);
    }

    #[test]
    fn a_round_takes_in_flight_payouts_first_and_one_task_per_evm_chain() {
        let user = Principal::anonymous();
        let mut stuck = log(user, BridgeTarget::Icp, evm("BNB"), BridgeTx::Icp(true, 1));
        stuck.stuck = true;
        let waiting_deposit = log(user, evm("BNB"), BridgeTarget::Icp, evm_tx(2));
        let new_bnb_payout = log(user, BridgeTarget::Icp, evm("BNB"), BridgeTx::Icp(true, 3));
        let eth_payout = log(user, BridgeTarget::Icp, evm("ETH"), BridgeTx::Icp(true, 4));
        let mut in_flight = log(user, BridgeTarget::Icp, evm("BNB"), BridgeTx::Icp(true, 5));
        in_flight.to_tx = Some(evm_tx(50));
        let sol_payout = log(
            user,
            BridgeTarget::Icp,
            BridgeTarget::Sol,
            BridgeTx::Icp(true, 6),
        );
        let pending = VecDeque::from([
            stuck.clone(),
            waiting_deposit.clone(),
            new_bnb_payout.clone(),
            eth_payout.clone(),
            in_flight.clone(),
            sol_payout.clone(),
        ]);

        let picked: Vec<BridgeTx> = select_round_tasks(&pending, 3)
            .into_iter()
            .map(|task| task.from_tx)
            .collect();

        // the BNB payout already broadcast holds the BNB nonce, so it goes
        // before the new BNB payout; the stuck task is skipped; the limit holds
        assert_eq!(
            picked,
            vec![
                in_flight.from_tx.clone(),
                waiting_deposit.from_tx.clone(),
                eth_payout.from_tx.clone()
            ]
        );

        let picked: Vec<BridgeTx> = select_round_tasks(&pending, 10)
            .into_iter()
            .map(|task| task.from_tx)
            .collect();
        assert_eq!(
            picked,
            vec![
                in_flight.from_tx.clone(),
                waiting_deposit.from_tx.clone(),
                eth_payout.from_tx.clone(),
                sol_payout.from_tx.clone()
            ]
        );

        let mut only_stuck = pending.clone();
        only_stuck.retain(|task| task.stuck);
        assert!(select_round_tasks(&only_stuck, 3).is_empty());
    }

    #[test]
    fn processed_tasks_rotate_to_the_back_of_the_queue() {
        let user = Principal::anonymous();
        let a = log(user, evm("BNB"), BridgeTarget::Icp, evm_tx(1));
        let b = log(user, evm("BNB"), BridgeTarget::Icp, evm_tx(2));
        let c = log(user, evm("BNB"), BridgeTarget::Icp, evm_tx(3));
        let d = log(user, evm("BNB"), BridgeTarget::Icp, evm_tx(4));
        let mut pending = VecDeque::from([a.clone(), b.clone(), c.clone(), d.clone()]);

        rotate_processed(&mut pending, &[a.from_tx.clone(), c.from_tx.clone()]);

        let order: Vec<BridgeTx> = pending.into_iter().map(|task| task.from_tx).collect();
        assert_eq!(order, vec![b.from_tx, d.from_tx, a.from_tx, c.from_tx]);
    }

    #[test]
    fn payout_destinations_are_canonical_and_never_the_bridge_itself() {
        let bridge_evm = Address::from([0x11; 20]);
        let contract = Address::from([0x22; 20]);
        let bridge_sol = Pubkey::new_from_array([3; 32]);
        let mint = Pubkey::new_from_array([4; 32]);
        let bridge = principal(&[5; 10]);
        let forbidden = ForbiddenDestinations {
            evm: vec![Address::ZERO, bridge_evm, contract],
            sol: vec![Pubkey::default(), bridge_sol, mint],
            icp: vec![Principal::anonymous(), bridge],
        };

        assert_eq!(check_destination(&evm("BNB"), None, &forbidden), Ok(None));

        let ok = "0xe74583edAFF618D88463554b84Bc675196b36990";
        assert_eq!(
            check_destination(&evm("BNB"), Some(&ok.to_lowercase()), &forbidden),
            Ok(Some(ok.to_string()))
        );
        assert!(
            check_destination(&evm("BNB"), Some(&ok.replace("AFF", "Aff")), &forbidden).is_err()
        );
        assert!(
            check_destination(&evm("BNB"), Some(&Address::ZERO.to_string()), &forbidden).is_err()
        );
        assert!(check_destination(&evm("BNB"), Some(&bridge_evm.to_string()), &forbidden).is_err());
        assert!(check_destination(&evm("BNB"), Some(&contract.to_string()), &forbidden).is_err());

        let wallet = Pubkey::new_from_array([9; 32]);
        assert_eq!(
            check_destination(&BridgeTarget::Sol, Some(&wallet.to_string()), &forbidden),
            Ok(Some(wallet.to_string()))
        );
        assert!(
            check_destination(
                &BridgeTarget::Sol,
                Some(&bridge_sol.to_string()),
                &forbidden
            )
            .is_err()
        );
        assert!(
            check_destination(&BridgeTarget::Sol, Some(&mint.to_string()), &forbidden).is_err()
        );
        assert!(check_destination(&BridgeTarget::Sol, Some("not a pubkey"), &forbidden).is_err());

        let user = principal(&[6; 29]);
        assert_eq!(
            check_destination(&BridgeTarget::Icp, Some(&user.to_text()), &forbidden),
            Ok(Some(user.to_text()))
        );
        assert!(check_destination(&BridgeTarget::Icp, Some("2vxsx-fae"), &forbidden).is_err());
        assert!(
            check_destination(&BridgeTarget::Icp, Some(&bridge.to_text()), &forbidden).is_err()
        );
        assert!(check_destination(&BridgeTarget::Icp, Some("nope"), &forbidden).is_err());
    }

    #[test]
    fn a_deposit_must_be_exact_in_the_source_chains_decimals() {
        assert!(check_source_precision(123_456_789, 8, 18).is_ok());
        assert!(check_source_precision(123_456_789, 8, 8).is_ok());
        assert!(check_source_precision(123_456_700, 8, 6).is_ok());
        assert!(check_source_precision(123_456_789, 8, 6).is_err());
        assert!(check_source_precision(100_000_000, 8, 0).is_ok());
        assert!(check_source_precision(100_000_001, 8, 0).is_err());
    }

    #[test]
    fn a_payout_must_be_exact_in_the_destination_chains_decimals() {
        // a destination at least as precise as the ledger loses nothing
        assert!(check_payout_precision(123_456_789, 8, 18).is_ok());
        assert!(check_payout_precision(123_456_789, 8, 8).is_ok());

        // a coarser one floors, and the remainder would stay with the bridge
        assert!(check_payout_precision(123_456_700, 8, 6).is_ok());
        assert!(check_payout_precision(123_456_789, 8, 6).is_err());

        // the amount checked is the one after the fee, not the one deposited
        let fee = 1u128;
        assert!(
            check_payout_precision(bridge_amount_after_fee(123_456_700, fee).unwrap(), 8, 6)
                .is_err()
        );
        assert!(
            check_payout_precision(bridge_amount_after_fee(123_456_701, fee).unwrap(), 8, 6)
                .is_ok()
        );
    }

    #[test]
    fn the_circuit_breaker_backs_off_then_cools_down_instead_of_stopping() {
        assert_eq!(error_backoff_secs(1), 5);
        assert_eq!(
            error_backoff_secs(MAX_ERROR_ROUNDS - 1),
            5 * (MAX_ERROR_ROUNDS - 1)
        );
        assert_eq!(error_backoff_secs(MAX_ERROR_ROUNDS), ERROR_COOLDOWN_SECS);
        assert_eq!(error_backoff_secs(u64::MAX), ERROR_COOLDOWN_SECS);
    }

    #[test]
    fn only_fees_of_deposits_made_on_icp_sit_on_the_ledger() {
        let user = Principal::anonymous();
        let finalized = |mut log: BridgeLog, to_tx: BridgeTx| {
            log.to_tx = Some(to_tx);
            log
        };
        let logs = vec![
            // ICP → BNB, complete: its fee stays on the ledger
            finalized(
                log(user, BridgeTarget::Icp, evm("BNB"), BridgeTx::Icp(true, 1)),
                BridgeTx::Evm(true, [1; 32].into()),
            ),
            // BNB → ICP, complete: its fee stays on BNB Chain
            finalized(
                log(
                    user,
                    evm("BNB"),
                    BridgeTarget::Icp,
                    BridgeTx::Evm(true, [2; 32].into()),
                ),
                BridgeTx::Icp(true, 2),
            ),
            // ICP → BNB, closed by an administrator: nothing bridged
            log(user, BridgeTarget::Icp, evm("BNB"), BridgeTx::Icp(true, 3)),
            // ICP → SOL, complete
            finalized(
                log(
                    user,
                    BridgeTarget::Icp,
                    BridgeTarget::Sol,
                    BridgeTx::Icp(true, 4),
                ),
                BridgeTx::Sol(true, [4; 64].into()),
            ),
        ];

        assert_eq!(icp_fee_share(logs.into_iter().map(BridgeLogLocal::from)), 2);
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
    fn bridge_log_records_survive_a_round_trip_with_their_metadata() {
        let mut task = log(
            principal(&[7; 29]),
            evm("BNB"),
            BridgeTarget::Icp,
            evm_tx(1),
        );
        task.from_meta = Some(meta(9));
        task.stuck = true;
        task.payout_started_at = 42;

        let local: BridgeLogLocal = task.clone().into();
        let decoded: BridgeLog = BridgeLogLocal::from_bytes(local.to_bytes()).into();
        assert_eq!(decoded.from_meta, task.from_meta);
        assert!(decoded.stuck);
        assert_eq!(decoded.payout_started_at, 42);

        // a record written before the fields existed decodes with defaults
        #[derive(Serialize)]
        struct RecordBeforeMetadata {
            u: Principal,
            f: BridgeTarget,
            t: BridgeTarget,
            a: u128,
            ft: BridgeTx,
            tt: Option<BridgeTx>,
            ca: u64,
            fa: u64,
        }
        let before = RecordBeforeMetadata {
            u: task.user,
            f: task.from.clone(),
            t: task.to.clone(),
            a: task.icp_amount,
            ft: task.from_tx.clone(),
            tt: None,
            ca: 1,
            fa: 0,
        };
        let legacy = BridgeLogLocal::from_bytes(Cow::Owned(cbor_into_vec(&before).unwrap()));
        assert_eq!(legacy.user, task.user);
        assert!(legacy.from_tx == task.from_tx);
        assert!(!legacy.stuck);
        assert_eq!(legacy.payout_started_at, 0);
        assert!(legacy.from_meta.is_none());
        assert_eq!(legacy.fee, 0);
    }
}
