use alloy_consensus::{SignableTransaction, TxEip1559};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxHash, U256, hex};
use candid::{CandidType, Nat, Principal};
use ic_auth_types::{ByteBufB64, cbor_from_slice, cbor_into_vec};
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
    future::Future,
    rc::Rc,
    str::FromStr,
    sync::LazyLock,
    time::Duration,
};

use crate::{
    ecdsa::{cost_sign_with_ecdsa, derive_public_key, ecdsa_public_key, sign_with_ecdsa},
    evm::{EvmClient, EvmReceipt, encode_erc20_transfer},
    helper::{bridge_amount_after_fee, convert_amount, format_error, now_ms, parse_evm_address},
    outcall::{DefaultHttpOutcall, HttpOutcall},
    schnorr::{derive_schnorr_public_key, schnorr_public_key},
    svm::{
        Message, Pubkey, Signature as SvmSignature, SolTxStatus, SolValidity, SvmClient,
        Transaction, create_associated_token_account_idempotent, get_associated_token_address,
        system_transfer_instruction, transfer_checked_instruction,
    },
    types::PublicKeyOutput,
};

type Memory = VirtualMemory<DefaultMemoryImpl>;

pub mod budget;
mod journal;
mod migration;
pub mod pending;
pub use budget::{EvmFeeLimits, ResourceLimits};
pub use journal::{OperationInfo, Resolution};

fn memory(id: u8) -> Memory {
    MEMORY_MANAGER.with_borrow(|m| m.get(MemoryId::new(id)))
}

#[derive(Clone)]
struct Cbor<T>(T);
impl<T: Serialize + serde::de::DeserializeOwned> Storable for Cbor<T> {
    const BOUND: Bound = Bound::Unbounded;
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(cbor_into_vec(&self.0).expect("encode stable record"))
    }
    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        Self(cbor_from_slice(&bytes).expect("decode stable record"))
    }
    fn into_bytes(self) -> Vec<u8> {
        cbor_into_vec(&self.0).expect("encode stable record")
    }
}

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

/// The fee a Solana transaction with one signature pays.
const SOL_TX_FEE_LAMPORTS: u64 = 5_000;

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

mod model;
pub use model::{
    BridgeLog, BridgeLogLocal, BridgeTarget, BridgeTx, LogRuntime, PayoutResolution, State,
    StateInfo, TxDeadline, TxMeta,
};

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

const STATE_MEMORY_ID: MemoryId = MemoryId::new(0);
// MemoryId 1 is reserved for the legacy user index and must never be reused.
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
    task_ids: Vec<u64>,
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
            for id in &self.task_ids {
                pending::update(*id, |task| {
                    if !task.stuck {
                        task.next_poll_at = now_ms().saturating_add(delay * 1000);
                    }
                });
            }
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
    Deposit(u64),
    Payout {
        task_id: u64,
        run_generation: u64,
    },
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

/// Old absence responses no longer establish non-execution reliably. Positive
/// finalized receipts can still complete a task at any age.
const ABSENCE_PROOF_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
enum TxCheckError {
    Transient(String),
    Reconcile(String),
}
impl From<String> for TxCheckError {
    fn from(error: String) -> Self {
        Self::Transient(error)
    }
}
impl TxCheckError {
    fn into_fault(self, chain: &str) -> TaskFault {
        match self {
            Self::Transient(error) => TaskFault::Transient(format!("{chain}: {error}")),
            Self::Reconcile(error) => TaskFault::Stuck(format!("{chain}: {error}")),
        }
    }
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

/// One encoding shared by the durable intent, pending task and broadcaster.
pub struct SignedTransfer {
    pub tx: BridgeTx,
    pub meta: TxMeta,
}

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
    /// A financial hold cannot be lifted by retrying or replaying signed bytes.
    ReconciliationRequired(String),
}

fn claim_pending_payout(
    run_generation: u64,
    from_tx: &BridgeTx,
    candidate: &Payout,
    now: u64,
) -> PayoutClaim {
    if !finalize_run_is_current(run_generation) {
        return PayoutClaim::RunSuperseded;
    }
    let Some(mut task) = pending::by_tx(from_tx) else {
        return PayoutClaim::TaskGone;
    };
    if let Err(error) = state::ensure_task_reconciled(&task) {
        return PayoutClaim::ReconciliationRequired(error);
    }
    if let Some(tx) = task.to_tx {
        return PayoutClaim::Existing((tx, task.to_meta));
    }
    task.to_tx = Some(candidate.0.clone());
    task.to_meta = Some(candidate.1.clone());
    task.payout_resolution = None;
    if task.payout_started_at == 0 {
        task.payout_started_at = now;
    }
    pending::insert(&task);
    PayoutClaim::Claimed
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
type ReadSlot<T> = Rc<futures::lock::Mutex<Option<Result<T, String>>>>;
type BlockCache = Rc<RefCell<HashMap<(String, BlockTag), ReadSlot<u64>>>>;
type LedgerFeeCache = Rc<RefCell<HashMap<Principal, ReadSlot<u128>>>>;

async fn read_once<T: Clone>(
    slot: &ReadSlot<T>,
    read: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let mut cached = slot.lock().await;
    if let Some(result) = &*cached {
        return result.clone();
    }
    let result = read.await;
    *cached = Some(result.clone());
    result
}

#[derive(Clone, Default)]
struct FinalizeContext {
    evm_blocks: BlockCache,
    sol_signatures: Vec<String>,
    sol_statuses: ReadSlot<Vec<SolTxStatus>>,
    ledger_fees: LedgerFeeCache,
}

impl FinalizeContext {
    fn new(tasks: &[BridgeLog]) -> Self {
        let mut signatures = Vec::new();
        for task in tasks {
            for tx in [Some(&task.from_tx), task.to_tx.as_ref()]
                .into_iter()
                .flatten()
            {
                if let BridgeTx::Sol(false, signature) = tx {
                    signatures.push(SvmSignature::from(**signature).to_string());
                }
            }
        }
        signatures.sort();
        signatures.dedup();
        Self {
            sol_signatures: signatures,
            ..Default::default()
        }
    }

    async fn sol_status<H: HttpOutcall>(
        &self,
        signature: &str,
        client: &SvmClient<H>,
    ) -> Result<SolTxStatus, String> {
        let Ok(index) = self
            .sol_signatures
            .binary_search_by(|value| value.as_str().cmp(signature))
        else {
            return client.get_signature_status(signature).await;
        };
        // Only Solana tasks wait for this batch. Other chains can settle while
        // its providers are slow, and concurrent Solana tasks reuse the result.
        let statuses = read_once(
            &self.sol_statuses,
            client.get_signature_statuses(&self.sol_signatures),
        )
        .await?;
        statuses
            .get(index)
            .cloned()
            .ok_or_else(|| "missing signature status".into())
    }

    async fn ledger_fee(&self, ledger: Principal) -> Result<u128, String> {
        self.ledger_fee_with(ledger, state::ledger_fee(ledger))
            .await
    }

    async fn ledger_fee_with(
        &self,
        ledger: Principal,
        read: impl Future<Output = Result<u128, String>>,
    ) -> Result<u128, String> {
        let slot = self
            .ledger_fees
            .borrow_mut()
            .entry(ledger)
            .or_default()
            .clone();
        read_once(&slot, read).await
    }
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
        read_once(&slot, async {
            match tag {
                BlockTag::Latest => client.block_number().await,
                BlockTag::Finalized => client.finalized_block_number().await,
            }
        })
        .await
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

pub mod state;

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
mod tests;
