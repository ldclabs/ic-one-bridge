//! Durable payment intents. An ambiguous result never releases a reservation or
//! changes the request. Completed records dominate late callbacks.
use super::*;
use crate::helper::{CallFailure, call_result};

#[derive(Clone, CandidType, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct DepositPlan {
    pub user: Principal,
    pub from: BridgeTarget,
    pub to: BridgeTarget,
    pub to_addr: Option<String>,
    pub ledger: Principal,
    pub amount: u128,
    pub fee: u128,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Purpose {
    Deposit(DepositPlan),
    Payout(u64),
    Withdrawal {
        to: Principal,
        amount: u128,
        ledger: Principal,
    },
    /// A second legacy task claimed an incoming transaction already assigned
    /// to another task. The complete record is retained for controller review
    /// instead of being discarded during migration.
    LegacyConflict {
        existing_task: u64,
        record: Box<BridgeLog>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Request {
    TransferFrom {
        ledger: Principal,
        args: TransferFromArgs,
    },
    Transfer {
        ledger: Principal,
        args: TransferArg,
    },
    Signature {
        scheme: String,
        key_name: String,
        sender: Principal,
        message: ByteBuf,
        deadline: TxDeadline,
        validity: Option<SolValidity>,
    },
    LegacyPayout(Box<BridgeLog>),
}

#[derive(Clone, Debug, CandidType, Serialize, Deserialize)]
pub enum Resolution {
    Completed(BridgeTx),
    NotExecuted,
}

#[derive(Clone, CandidType, Serialize, Deserialize)]
pub struct Reconciliation {
    pub controller: Principal,
    pub evidence: String,
    pub resolution: Resolution,
}

#[derive(Clone, CandidType, Serialize, Deserialize, Debug)]
pub enum Phase {
    Planning,
    Prepared,
    Submitted,
    Signed,
    Completed(BridgeTx),
    Rejected(String),
    NeedsReview(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub owner: Principal,
    pub purpose: Purpose,
    pub created_at: u64,
    pub phase: Phase,
    pub request: Option<Request>,
    pub signed: Option<(BridgeTx, TxMeta)>,
    pub signature: Option<ByteBuf>,
    pub handled: bool,
    /// A withdrawal counts against total_withdrawn_fees before submission and
    /// stays counted while its outcome is unknown or successful.
    pub withdrawal_counted: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub call_generation: u64,
    #[serde(default)]
    pub reconciliation: Option<Reconciliation>,
}

#[derive(Clone, CandidType, Deserialize)]
pub struct OperationInfo {
    pub id: u64,
    pub owner: Principal,
    pub created_at: u64,
    pub kind: String,
    pub phase: Phase,
    pub deposit: Option<DepositPlan>,
    pub error: Option<String>,
    pub revision: u64,
    pub request: Option<RequestInfo>,
    pub signed_tx: Option<BridgeTx>,
    pub related_task: Option<BridgeLog>,
    pub reconciliation: Option<Reconciliation>,
}

#[derive(Clone, CandidType, Deserialize)]
pub enum RequestInfo {
    TransferFrom(Principal, TransferFromArgs),
    Transfer(Principal, TransferArg),
    Signature {
        scheme: String,
        sender: Principal,
        message_hash: ByteBuf,
        deadline: TxDeadline,
        validity: Option<SolValidity>,
    },
    LegacyPayout(Box<BridgeLog>),
}

pub fn info(entry: Entry) -> OperationInfo {
    let related_task = match &entry.purpose {
        Purpose::Payout(id) => pending::get(*id).map(BridgeLog::public_view),
        Purpose::LegacyConflict { record, .. } => Some(record.clone().public_view()),
        _ => None,
    };
    let request = entry.request.map(|request| match request {
        Request::TransferFrom { ledger, args } => RequestInfo::TransferFrom(ledger, args),
        Request::Transfer { ledger, args } => RequestInfo::Transfer(ledger, args),
        Request::Signature {
            scheme,
            sender,
            message,
            deadline,
            validity,
            ..
        } => RequestInfo::Signature {
            scheme: scheme.clone(),
            sender,
            message_hash: if scheme == "ecdsa" {
                message
            } else {
                alloy_primitives::keccak256(message).to_vec().into()
            },
            deadline,
            validity,
        },
        Request::LegacyPayout(task) => RequestInfo::LegacyPayout(Box::new(task.public_view())),
    });
    let (kind, deposit) = match entry.purpose {
        Purpose::Deposit(plan) => ("deposit", Some(plan)),
        Purpose::Payout(_) => ("payout", None),
        Purpose::Withdrawal { .. } => ("fee withdrawal", None),
        Purpose::LegacyConflict { .. } => ("legacy pending conflict", None),
    };
    OperationInfo {
        id: entry.id,
        owner: entry.owner,
        created_at: entry.created_at,
        kind: kind.into(),
        phase: entry.phase,
        deposit,
        error: entry.error,
        revision: entry.revision,
        request,
        signed_tx: entry.signed.map(|(tx, _)| tx),
        related_task,
        reconciliation: entry.reconciliation,
    }
}

thread_local! {
    static ENTRIES: RefCell<StableBTreeMap<u64, Cbor<Entry>, Memory>> = RefCell::new(StableBTreeMap::init(memory(mem::JOURNAL_ENTRIES)));
    static USERS: RefCell<StableBTreeMap<UserLogKey, (), Memory>> = RefCell::new(StableBTreeMap::init(memory(mem::JOURNAL_USERS)));
    // The key is owner || caller request ID. It outlives completion and upgrades.
    static REQUEST_IDS: RefCell<StableBTreeMap<Vec<u8>, u64, Memory>> = RefCell::new(StableBTreeMap::init(memory(mem::REQUEST_IDS)));
    static OPEN: RefCell<StableBTreeMap<(u8, u64), (), Memory>> = RefCell::new(StableBTreeMap::init(memory(mem::OPEN_OPERATIONS)));
}

/// Makes `key` map to `value`, or be absent, writing only when that changes.
fn set_key<K, V>(map: &mut StableBTreeMap<K, V, Memory>, key: K, value: Option<V>)
where
    K: Storable + Ord + Clone,
    V: Storable + PartialEq,
{
    match value {
        Some(value) => {
            if map.get(&key).as_ref() != Some(&value) {
                map.insert(key, value);
            }
        }
        None => {
            map.remove(&key);
        }
    }
}

/// The key under which an unresolved deposit or withdrawal of `owner` is
/// found without a caller request ID.
fn open_request_key(owner: &Principal, id: u64) -> Vec<u8> {
    let mut key = vec![255];
    key.extend_from_slice(&UserLogKey::new(owner, id).0);
    key
}

pub fn get(id: u64) -> Option<Entry> {
    ENTRIES.with_borrow(|entries| entries.get(&id).map(|e| e.0))
}

pub fn last_id() -> u64 {
    ENTRIES.with_borrow(|entries| entries.keys().next_back().unwrap_or(0))
}

fn sync_conflict_hold(entry: &Entry) {
    if let Purpose::LegacyConflict { existing_task, .. } = &entry.purpose {
        pending::set_conflict_hold(*existing_task, entry.id, is_open(entry));
    }
}

/// Backfills reconciliation holds and automatic recovery indexes. The upper
/// bound is captured at upgrade; new writes update both indexes in put.
pub fn migrate_indexes(after: u64, through: u64, limit: usize) -> (u64, usize) {
    if after >= through || limit == 0 {
        return (after, 0);
    }
    let entries = ENTRIES.with_borrow(|entries| {
        entries
            .range((
                std::ops::Bound::Excluded(after),
                std::ops::Bound::Included(through),
            ))
            .take(limit)
            .map(|entry| entry.value().0)
            .collect::<Vec<_>>()
    });
    for entry in &entries {
        sync_conflict_hold(entry);
        sync_open_index(entry);
    }
    let cursor = if entries.len() < limit {
        through
    } else {
        entries.last().expect("nonempty migration batch").id
    };
    (cursor, entries.len())
}

/// Stores `entry` as its next revision. Callers write back the entry they
/// read in the same message, so its revision is the stored one.
pub fn put(entry: &mut Entry) {
    entry.revision = entry.revision.saturating_add(1);
    ENTRIES.with_borrow_mut(|entries| {
        entries.insert(entry.id, Cbor(entry.clone()));
    });
    sync_open_index(entry);
    let open_request = is_open(entry)
        && matches!(
            entry.purpose,
            Purpose::Deposit(_) | Purpose::Withdrawal { .. }
        );
    REQUEST_IDS.with_borrow_mut(|ids| {
        set_key(
            ids,
            open_request_key(&entry.owner, entry.id),
            open_request.then_some(entry.id),
        );
    });
    sync_conflict_hold(entry);
}

fn is_open(entry: &Entry) -> bool {
    !entry.handled && !matches!(entry.phase, Phase::Rejected(_))
}

fn sync_open_index(entry: &Entry) {
    let open = is_open(entry);
    let payout = matches!(entry.purpose, Purpose::Payout(_));
    let ledger_request = matches!(
        entry.request,
        Some(Request::TransferFrom { .. } | Request::Transfer { .. })
    ) && matches!(
        entry.phase,
        Phase::Prepared | Phase::Submitted | Phase::Completed(_)
    );
    let completed_deposit =
        matches!(entry.purpose, Purpose::Deposit(_)) && matches!(entry.phase, Phase::Completed(_));
    let recoverable = open && !payout && (ledger_request || completed_deposit);
    OPEN.with_borrow_mut(|index| {
        set_key(index, (0, entry.id), open.then_some(()));
        set_key(index, (1, entry.id), recoverable.then_some(()));
        set_key(index, (2, entry.id), (open && !payout).then_some(()));
    });
}
pub fn unresolved() -> bool {
    OPEN.with_borrow(|open| !open.is_empty())
}
pub fn open_ids(take: usize) -> Vec<u64> {
    OPEN.with_borrow(|open| {
        open.keys_range((1, 0)..(2, 0))
            .take(take)
            .map(|(_, id)| id)
            .collect()
    })
}
pub fn open_count() -> u64 {
    OPEN.with_borrow(|open| open.range((2, 0)..(3, 0)).count() as u64)
}

/// The owner's unresolved deposit or withdrawal that `wanted` picks.
fn open_entry(owner: Principal, wanted: impl Fn(&Purpose) -> bool) -> Option<Entry> {
    let range = open_request_key(&owner, 0)..open_request_key(&owner, u64::MAX);
    REQUEST_IDS
        .with_borrow(|ids| ids.range(range).map(|e| e.value()).collect::<Vec<_>>())
        .into_iter()
        .filter_map(get)
        .find(|e| wanted(&e.purpose))
}

pub fn open_deposit(owner: Principal) -> Option<Entry> {
    open_entry(owner, |purpose| matches!(purpose, Purpose::Deposit(_)))
}

pub fn open_withdrawal(owner: Principal) -> Option<Entry> {
    open_entry(owner, |purpose| {
        matches!(purpose, Purpose::Withdrawal { .. })
    })
}

fn explicit_request_key(owner: Principal, id: &[u8]) -> Result<Vec<u8>, String> {
    if id.is_empty() || id.len() > 64 {
        return Err("request_id must be 1 to 64 bytes".into());
    }
    let mut key = vec![owner.as_slice().len() as u8];
    key.extend_from_slice(owner.as_slice());
    key.extend_from_slice(id);
    Ok(key)
}

fn bind_request_key(key: &[u8], operation_id: u64) -> Result<(), String> {
    REQUEST_IDS.with_borrow_mut(|ids| {
        if let Some(existing) = ids.get(&key.to_vec()) {
            if existing != operation_id {
                return Err("request_id already belongs to another operation".into());
            }
            return Ok(());
        }
        ids.insert(key.to_vec(), operation_id);
        Ok(())
    })
}

pub fn find_request(owner: Principal, id: &[u8]) -> Result<Option<Entry>, String> {
    let key = explicit_request_key(owner, id)?;
    Ok(REQUEST_IDS.with_borrow(|ids| ids.get(&key)).and_then(get))
}

pub fn bind_request(owner: Principal, id: &[u8], operation_id: u64) -> Result<(), String> {
    let entry = get(operation_id).ok_or_else(|| "operation not found".to_string())?;
    if entry.owner != owner || !matches!(entry.purpose, Purpose::Deposit(_)) {
        return Err("request_id can only be bound to the owner's deposit operation".into());
    }
    bind_request_key(&explicit_request_key(owner, id)?, operation_id)
}

pub fn draft(owner: Principal, purpose: Purpose, created_at: u64) -> Entry {
    Entry {
        id: 0,
        owner,
        purpose,
        created_at,
        phase: Phase::Planning,
        request: None,
        signed: None,
        signature: None,
        handled: false,
        withdrawal_counted: false,
        error: None,
        revision: 0,
        call_generation: 0,
        reconciliation: None,
    }
}

pub fn create(owner: Principal, purpose: Purpose, created_at: u64) -> Entry {
    let mut entry = draft(owner, purpose, created_at);
    entry.id = pending::next_id();
    put(&mut entry);
    USERS.with_borrow_mut(|users| {
        users.insert(UserLogKey::new(&owner, entry.id), ());
    });
    entry
}

/// Records a new deposit intent, bound to the caller's request ID if it
/// brought one. The caller has already resumed any intent that ID or the
/// user's unresolved deposit points at.
pub fn for_deposit(
    plan: DepositPlan,
    request_id: Option<&[u8]>,
    now: u64,
) -> Result<Entry, String> {
    let key = request_id
        .map(|id| explicit_request_key(plan.user, id))
        .transpose()?;
    let entry = create(plan.user, Purpose::Deposit(plan), now);
    if let Some(key) = key {
        bind_request_key(&key, entry.id)?;
    }
    Ok(entry)
}

fn user_log_ids_raw(owner: Principal, take: usize, before: Option<u64>) -> Vec<u64> {
    USERS.with_borrow(|users| user_log_ids(users, &owner, before.unwrap_or(u64::MAX), take))
}

pub fn page(owner: Principal, take: usize, before: Option<u64>) -> Vec<OperationInfo> {
    user_log_ids_raw(owner, take.clamp(1, 100), before)
        .into_iter()
        .filter_map(get)
        .map(info)
        .collect()
}

/// Persist the exact request before calling the ledger. Transfer fees are paid
/// directly from the bridge account; only governance withdrawals reserve their
/// principal against the existing earned-fee ceiling.
pub fn prepare(id: u64, request: Request) -> Result<(), String> {
    let mut entry = get(id).ok_or_else(|| "operation not found".to_string())?;
    if entry.request.is_some() {
        return Ok(());
    }
    if !matches!(entry.phase, Phase::Planning) {
        return Err("operation cannot be prepared again".into());
    }
    if matches!(request, Request::Transfer { .. })
        && let Purpose::Withdrawal { amount, .. } = &entry.purpose
    {
        STATE.with_borrow_mut(|s| {
            if *amount > available_withdrawal(s) {
                return Err("withdrawal exceeds earned ICP fees".to_string());
            }
            s.total_withdrawn_fees = s
                .total_withdrawn_fees
                .checked_add(*amount)
                .ok_or_else(|| "withdrawal overflow".to_string())?;
            Ok::<_, String>(())
        })?;
        entry.withdrawal_counted = true;
    }
    entry.request = Some(request);
    entry.phase = Phase::Prepared;
    put(&mut entry);
    Ok(())
}

/// Preserve the pre-upgrade governance withdrawal cap. Outstanding withdrawals
/// remain included in total_withdrawn_fees until proven not to have executed.
/// Neither this cap nor historical fee migration is a ledger-fee budget.
pub fn available_withdrawal(s: &State) -> u128 {
    if !s.icp_collected_fees_migrated {
        return 0;
    }
    s.icp_collected_fees.saturating_sub(s.total_withdrawn_fees)
}

fn settle_withdrawal(entry: &mut Entry, counted: bool) {
    if entry.withdrawal_counted == counted
        || !matches!(entry.request, Some(Request::Transfer { .. }))
    {
        return;
    }
    if let Purpose::Withdrawal { amount, .. } = &entry.purpose {
        STATE.with_borrow_mut(|s| {
            s.total_withdrawn_fees = if counted {
                s.total_withdrawn_fees.saturating_add(*amount)
            } else {
                s.total_withdrawn_fees.saturating_sub(*amount)
            };
        });
        entry.withdrawal_counted = counted;
    }
}

pub fn completed(id: u64, tx: BridgeTx) -> Result<BridgeTx, String> {
    let mut entry = get(id).ok_or_else(|| "operation not found".to_string())?;
    if let Phase::Completed(existing) = entry.phase {
        return Ok(existing);
    }
    if matches!(entry.purpose, Purpose::Deposit(_)) {
        pending::remember_transaction(&tx, id)?;
    }
    settle_withdrawal(&mut entry, true);
    entry.phase = Phase::Completed(tx.clone());
    entry.error = None;
    put(&mut entry);
    Ok(tx)
}

pub fn handled(id: u64) {
    if let Some(mut entry) = get(id) {
        entry.handled = true;
        if matches!(entry.phase, Phase::Completed(_)) {
            entry.request = None;
            entry.signed = None;
            entry.signature = None;
        }
        put(&mut entry);
    }
}

pub fn failed(id: u64, error: String, uncertain: bool, retryable: bool) -> String {
    let error = crate::outcall::public_error(&error, &[]);
    if let Some(mut entry) = get(id) {
        if matches!(entry.phase, Phase::Completed(_)) {
            return error;
        }
        entry.error = Some(error.clone());
        entry.phase = if uncertain {
            if retryable {
                Phase::Submitted
            } else {
                Phase::NeedsReview(error.clone())
            }
        } else if retryable {
            Phase::Prepared
        } else {
            settle_withdrawal(&mut entry, false);
            Phase::Rejected(error.clone())
        };
        put(&mut entry);
    }
    error
}

pub trait LedgerTransport {
    async fn transfer(
        &self,
        ledger: Principal,
        args: TransferArg,
    ) -> Result<Result<Nat, TransferError>, CallFailure>;
    async fn transfer_from(
        &self,
        ledger: Principal,
        args: TransferFromArgs,
    ) -> Result<Result<Nat, TransferFromError>, CallFailure>;
}
pub struct Ledger;
impl LedgerTransport for Ledger {
    async fn transfer(
        &self,
        ledger: Principal,
        args: TransferArg,
    ) -> Result<Result<Nat, TransferError>, CallFailure> {
        call_result(ledger, "icrc1_transfer", (args,)).await
    }
    async fn transfer_from(
        &self,
        ledger: Principal,
        args: TransferFromArgs,
    ) -> Result<Result<Nat, TransferFromError>, CallFailure> {
        call_result(ledger, "icrc2_transfer_from", (args,)).await
    }
}

pub async fn execute(id: u64) -> Result<BridgeTx, String> {
    execute_with(id, &Ledger).await
}

pub async fn execute_with(id: u64, ledger: &impl LedgerTransport) -> Result<BridgeTx, String> {
    let mut entry = get(id).ok_or_else(|| "operation not found".to_string())?;
    let uncertain = match &entry.phase {
        Phase::Completed(tx) => return Ok(tx.clone()),
        Phase::Rejected(e) | Phase::NeedsReview(e) => return Err(e.clone()),
        Phase::Prepared => false,
        Phase::Submitted => true,
        _ => return Err("operation is not a prepared ledger request".into()),
    };
    let request = entry
        .request
        .clone()
        .ok_or_else(|| "missing ledger request".to_string())?;
    entry.call_generation = entry.call_generation.saturating_add(1);
    let call_generation = entry.call_generation;
    entry.phase = Phase::Submitted;
    put(&mut entry); // commits before the call
    let result: Result<Nat, (String, bool, bool)> = match request {
        Request::Transfer {
            ledger: target,
            args,
        } => match ledger.transfer(target, args).await {
            Ok(Ok(index))
            | Ok(Err(TransferError::Duplicate {
                duplicate_of: index,
            })) => Ok(index),
            Ok(Err(TransferError::TooOld)) => Err((
                "ICP: dedup window elapsed; reconcile this operation".into(),
                true,
                false,
            )),
            Ok(Err(error)) => {
                let retry = matches!(
                    error,
                    TransferError::TemporarilyUnavailable | TransferError::CreatedInFuture { .. }
                );
                Err((format!("ICP: {error}"), uncertain, retry))
            }
            Err(error) => Err((error.message, uncertain || error.ambiguous, true)),
        },
        Request::TransferFrom {
            ledger: target,
            args,
        } => match ledger.transfer_from(target, args).await {
            Ok(Ok(index))
            | Ok(Err(TransferFromError::Duplicate {
                duplicate_of: index,
            })) => Ok(index),
            Ok(Err(TransferFromError::TooOld)) => Err((
                "ICP: dedup window elapsed; reconcile this operation".into(),
                true,
                false,
            )),
            Ok(Err(error)) => {
                let retry = matches!(
                    error,
                    TransferFromError::TemporarilyUnavailable
                        | TransferFromError::CreatedInFuture { .. }
                );
                Err((format!("ICP: {error}"), uncertain, retry))
            }
            Err(error) => Err((error.message, uncertain || error.ambiguous, true)),
        },
        _ => return Err("operation is not a ledger transfer".into()),
    };
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::after_ledger_reply();
    // An older failing callback must not downgrade a later successful retry.
    if let Some(Entry {
        phase: Phase::Completed(tx),
        ..
    }) = get(id)
    {
        return Ok(tx);
    }
    match result {
        Ok(index) => match u64::try_from(&index.0) {
            Ok(index) => completed(id, BridgeTx::Icp(true, index)),
            Err(_) => Err(failed(
                id,
                "ICP: block index exceeds API range; reconcile operation".into(),
                true,
                false,
            )),
        },
        Err((message, ambiguous, retry)) => {
            if get(id).is_some_and(|entry| entry.call_generation != call_generation) {
                return Err("a newer attempt is still resolving this operation".into());
            }
            Err(failed(id, message, ambiguous, retry))
        }
    }
}

/// Only a fresh signature intent can cross the signing boundary. A trap or an
/// ambiguous signature response leaves Submitted, which requires reconciliation.
pub fn start_signature(id: u64) -> Result<Request, String> {
    let mut entry = get(id).ok_or_else(|| "signature intent not found".to_string())?;
    if !matches!(entry.phase, Phase::Prepared) {
        return Err(
            "signature outcome is unknown; reconcile the existing operation before signing again"
                .into(),
        );
    }
    let request = entry
        .request
        .clone()
        .ok_or_else(|| "signature request missing".to_string())?;
    if !matches!(request, Request::Signature { .. }) {
        return Err("not a signature request".into());
    }
    entry.phase = Phase::Submitted;
    put(&mut entry);
    Ok(request)
}

/// A signature intent the signer refused outright: nothing was signed, so a
/// fresh intent cannot pay twice.
pub fn signature_refused(entry: &Entry) -> bool {
    matches!(entry.phase, Phase::Rejected(_))
        && entry.signed.is_none()
        && matches!(entry.request, Some(Request::Signature { .. }))
}

/// Whether the operation waits for a controller: its outcome is unknown, or
/// it was refused in a way a retry cannot fix.
pub fn needs_review(entry: &Entry) -> bool {
    match entry.phase {
        Phase::NeedsReview(_) => true,
        Phase::Rejected(_) => !signature_refused(entry),
        Phase::Submitted => matches!(entry.request, Some(Request::Signature { .. })),
        _ => false,
    }
}

pub fn record_signed(id: u64, transfer: &SignedTransfer, signature: Vec<u8>) -> Result<(), String> {
    let mut entry = get(id).ok_or_else(|| "signature intent not found".to_string())?;
    if !matches!(entry.phase, Phase::Submitted | Phase::NeedsReview(_)) {
        return Err("signature operation has already been resolved".into());
    }
    entry.phase = Phase::Signed;
    entry.signed = Some((transfer.tx.clone(), transfer.meta.clone()));
    entry.signature = Some(signature.into());
    put(&mut entry);
    if let Purpose::Payout(task_id) = entry.purpose {
        let wake = pending::update(task_id, |task| {
            if !task.stuck {
                return false;
            }
            task.stuck = false;
            task.error = None;
            task.error_chain = None;
            task.next_poll_at = now_ms();
            true
        })
        .unwrap_or(false);
        if wake {
            state::schedule_finalize(Duration::ZERO);
        }
    }
    Ok(())
}

pub fn memo(id: u64) -> Memo {
    let mut bytes = b"1bridge/payment/v1".to_vec();
    bytes.extend_from_slice(&id.to_be_bytes());
    Memo::from(bytes)
}

pub fn safe_to_reset(id: u64) -> bool {
    get(id).is_some_and(|entry| {
        matches!(
            entry.phase,
            Phase::Rejected(_) | Phase::Planning | Phase::Prepared
        )
    })
}

pub fn record_legacy_conflict(existing_task: u64, mut record: BridgeLog) -> Entry {
    let error = format!(
        "legacy incoming transaction conflicts with task {existing_task}; review both records"
    );
    record.stuck = true;
    record.error = Some(error.clone());
    let mut entry = create(
        record.user,
        Purpose::LegacyConflict {
            existing_task,
            record: Box::new(record),
        },
        now_ms(),
    );
    entry.phase = Phase::NeedsReview(error.clone());
    entry.error = Some(error);
    put(&mut entry);
    entry
}

pub fn check_resolution(
    entry: &Entry,
    revision: u64,
    resolution: &Resolution,
    evidence: &str,
) -> Result<(), String> {
    if entry.revision != revision {
        return Err("operation changed; review its current revision".into());
    }
    if !(20..=2000).contains(&evidence.trim().len()) {
        return Err("include 20 to 2000 bytes identifying the external ledger/chain evidence and reconciliation".into());
    }
    if matches!(entry.phase, Phase::Completed(_)) {
        return Err("completed operations cannot be reset".into());
    }
    if let Resolution::Completed(tx) = resolution {
        if matches!(entry.purpose, Purpose::Deposit(_))
            && pending::known_transaction(tx).is_some_and(|id| id != entry.id)
        {
            return Err("transaction already belongs to another operation".into());
        }
        if let Some(Request::LegacyPayout(task)) = &entry.request
            && task.to_tx.as_ref().is_some_and(|recorded| recorded != tx)
        {
            return Err("legacy completion must match its recorded payout".into());
        }

        if !tx.is_finalized() || entry.request.is_none() {
            return Err(
                "completion requires a finalized transaction for a submitted request".into(),
            );
        }
        if let Some((signed, _)) = &entry.signed
            && signed != tx
        {
            return Err("completion must identify the recorded signed transaction".into());
        }
        let correct_kind = match &entry.request {
            Some(Request::Transfer { .. } | Request::TransferFrom { .. }) => {
                matches!(tx, BridgeTx::Icp(..))
            }
            Some(Request::Signature { scheme, .. }) if scheme == "ecdsa" => {
                matches!(tx, BridgeTx::Evm(..))
            }
            Some(Request::Signature { scheme, .. }) if scheme == "ed25519" => {
                matches!(tx, BridgeTx::Sol(..))
            }
            Some(Request::LegacyPayout(task)) => matches!(
                (&task.to, tx),
                (BridgeTarget::Icp, BridgeTx::Icp(..))
                    | (BridgeTarget::Evm(_), BridgeTx::Evm(..))
                    | (BridgeTarget::Sol, BridgeTx::Sol(..))
            ),
            _ => false,
        };
        if !correct_kind {
            return Err("transaction belongs to the wrong chain kind".into());
        }
    }
    Ok(())
}

/// This is an explicit controller attestation after external reconciliation,
/// not an automatic absence proof. The evidence and exact revision are retained.
pub fn resolve(
    id: u64,
    revision: u64,
    resolution: Resolution,
    evidence: String,
    controller: Principal,
) -> Result<Entry, String> {
    let mut entry = get(id).ok_or_else(|| "operation not found".to_string())?;
    check_resolution(&entry, revision, &resolution, &evidence)?;
    entry.reconciliation = Some(Reconciliation {
        controller,
        evidence: evidence.clone(),
        resolution: resolution.clone(),
    });
    put(&mut entry);
    match resolution {
        Resolution::Completed(tx) => {
            completed(id, tx)?;
        }
        Resolution::NotExecuted => {
            failed(
                id,
                format!("manually reconciled as not executed: {evidence}"),
                false,
                false,
            );
        }
    }
    get(id).ok_or_else(|| "operation disappeared".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn deposit() -> Entry {
        let owner = Principal::from_slice(&[1]);
        let ledger = Principal::from_slice(&[2]);
        let entry = create(
            owner,
            Purpose::Deposit(DepositPlan {
                user: owner,
                from: BridgeTarget::Icp,
                to: BridgeTarget::Evm("ETH".into()),
                to_addr: None,
                ledger,
                amount: 100,
                fee: 1,
            }),
            now_ms(),
        );
        prepare(
            entry.id,
            Request::TransferFrom {
                ledger,
                args: TransferFromArgs {
                    spender_subaccount: None,
                    from: Account {
                        owner,
                        subaccount: None,
                    },
                    to: Account {
                        owner: crate::helper::canister_id(),
                        subaccount: None,
                    },
                    amount: 100u64.into(),
                    fee: None,
                    memo: Some(memo(entry.id)),
                    created_at_time: Some(entry.created_at * 1_000_000),
                },
            },
        )
        .unwrap();
        get(entry.id).unwrap()
    }
    struct LostReply {
        calls: Cell<u32>,
        debits: Cell<u32>,
        first_request: RefCell<Option<Vec<u8>>>,
    }
    impl LostReply {
        fn new() -> Self {
            Self {
                calls: Cell::new(0),
                debits: Cell::new(0),
                first_request: RefCell::new(None),
            }
        }
        fn apply(&self, args: Vec<u8>) -> Result<Result<Nat, TransferError>, CallFailure> {
            self.calls.set(self.calls.get() + 1);
            if let Some(first) = &*self.first_request.borrow() {
                assert_eq!(first, &args);
            }
            if self.debits.get() == 0 {
                self.debits.set(1);
                *self.first_request.borrow_mut() = Some(args);
                Err(CallFailure {
                    ambiguous: true,
                    message: "ledger applied transfer but reply cannot be decoded".into(),
                })
            } else {
                Ok(Err(TransferError::Duplicate {
                    duplicate_of: 7u64.into(),
                }))
            }
        }
    }
    impl LedgerTransport for LostReply {
        async fn transfer(
            &self,
            _: Principal,
            args: TransferArg,
        ) -> Result<Result<Nat, TransferError>, CallFailure> {
            self.apply(cbor_into_vec(&args).unwrap())
        }
        async fn transfer_from(
            &self,
            _: Principal,
            args: TransferFromArgs,
        ) -> Result<Result<Nat, TransferFromError>, CallFailure> {
            self.apply(cbor_into_vec(&args).unwrap()).map(|value| {
                value.map_err(|_| TransferFromError::Duplicate {
                    duplicate_of: 7u64.into(),
                })
            })
        }
    }
    #[test]
    fn unknown_deposit_reply_reuses_the_persisted_request_and_debits_once() {
        let entry = deposit();
        let ledger = LostReply::new();
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_err());
        assert!(matches!(get(entry.id).unwrap().phase, Phase::Submitted));
        assert!(!safe_to_reset(entry.id));
        let reopened: StableBTreeMap<u64, Cbor<Entry>, Memory> =
            StableBTreeMap::init(memory(mem::JOURNAL_ENTRIES));
        assert!(matches!(
            reopened.get(&entry.id).unwrap().0.phase,
            Phase::Submitted
        ));
        assert!(matches!(
            futures::executor::block_on(execute_with(entry.id, &ledger)),
            Ok(BridgeTx::Icp(true, 7))
        ));
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_ok());
        assert_eq!((ledger.calls.get(), ledger.debits.get()), (2, 1));
    }
    fn withdrawal(owner: Principal, amount: u128) -> Entry {
        create(
            owner,
            Purpose::Withdrawal {
                to: owner,
                amount,
                ledger: Principal::from_slice(&[2]),
            },
            now_ms(),
        )
    }

    fn transfer_request(entry: &Entry, amount: u128) -> Request {
        Request::Transfer {
            ledger: Principal::from_slice(&[2]),
            args: TransferArg {
                from_subaccount: None,
                to: Account {
                    owner: entry.owner,
                    subaccount: None,
                },
                amount: amount.into(),
                fee: Some(10u64.into()),
                memo: Some(memo(entry.id)),
                created_at_time: Some(entry.created_at * 1_000_000),
            },
        }
    }

    #[test]
    fn unknown_withdrawal_keeps_its_cap_reservation_and_debits_once() {
        STATE.with_borrow_mut(|s| {
            s.icp_collected_fees_migrated = true;
            s.icp_collected_fees = 200;
        });
        let entry = withdrawal(Principal::from_slice(&[1]), 50);
        prepare(entry.id, transfer_request(&entry, 50)).unwrap();
        let ledger = LostReply::new();
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_err());
        assert_eq!(
            STATE.with_borrow(|s| (s.total_withdrawn_fees, available_withdrawal(s))),
            (50, 150)
        );

        // A second controller cannot spend the unresolved withdrawal's share.
        let other = withdrawal(Principal::from_slice(&[3]), 151);
        assert!(prepare(other.id, transfer_request(&other, 151)).is_err());
        assert!(!get(other.id).unwrap().withdrawal_counted);
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_ok());
        failed(entry.id, "late error".into(), false, false);
        assert_eq!(
            STATE.with_borrow(|s| (s.total_withdrawn_fees, available_withdrawal(s))),
            (50, 150)
        );
        assert_eq!(ledger.debits.get(), 1);
    }

    #[test]
    fn a_corrected_success_reinstates_a_released_withdrawal_exactly_once() {
        STATE.with_borrow_mut(|s| {
            s.icp_collected_fees_migrated = true;
            s.icp_collected_fees = 200;
        });
        let owner = Principal::from_slice(&[71]);
        let entry = withdrawal(owner, 50);
        prepare(entry.id, transfer_request(&entry, 50)).unwrap();
        resolve(
            entry.id,
            get(entry.id).unwrap().revision,
            Resolution::NotExecuted,
            "ledger evidence initially showed no execution".into(),
            owner,
        )
        .unwrap();
        failed(entry.id, "same rejection".into(), false, false);
        assert_eq!(
            STATE.with_borrow(|s| (s.total_withdrawn_fees, available_withdrawal(s))),
            (0, 200)
        );
        completed(entry.id, BridgeTx::Icp(true, 9_999)).unwrap();
        completed(entry.id, BridgeTx::Icp(true, 9_999)).unwrap();
        assert_eq!(
            STATE.with_borrow(|s| (s.total_withdrawn_fees, available_withdrawal(s))),
            (50, 150)
        );
        assert!(get(entry.id).unwrap().withdrawal_counted);
    }

    #[test]
    fn ledger_payout_needs_no_earned_fee_balance_and_still_deduplicates() {
        assert_eq!(STATE.with_borrow(|s| s.icp_collected_fees), 0);
        let entry = create(Principal::from_slice(&[8]), Purpose::Payout(99), now_ms());
        prepare(entry.id, transfer_request(&entry, 100)).unwrap();
        let ledger = LostReply::new();
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_err());
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_ok());
        assert_eq!(ledger.debits.get(), 1);
        assert_eq!(STATE.with_borrow(|s| s.total_withdrawn_fees), 0);
    }

    #[test]
    fn signature_intent_cannot_be_replanned_after_submission() {
        let entry = create(Principal::from_slice(&[1]), Purpose::Payout(99), now_ms());
        prepare(
            entry.id,
            Request::Signature {
                scheme: "ecdsa".into(),
                key_name: "test".into(),
                sender: entry.owner,
                message: vec![1; 32].into(),
                deadline: TxDeadline::Nonce(7),
                validity: None,
            },
        )
        .unwrap();
        assert!(start_signature(entry.id).is_ok());
        assert!(start_signature(entry.id).is_err());
        assert!(!safe_to_reset(entry.id));
    }
    #[test]
    fn each_write_is_one_revision_and_the_open_indexes_follow_it() {
        let entry = deposit();
        // Created, then prepared.
        assert_eq!(entry.revision, 2);
        assert!(open_ids(10).contains(&entry.id));
        assert_eq!(open_count(), 1);
        assert_eq!(open_deposit(entry.owner).map(|e| e.id), Some(entry.id));
        handled(entry.id);
        assert_eq!(get(entry.id).unwrap().revision, 3);
        assert!(!unresolved());
        assert!(open_deposit(entry.owner).is_none());
    }

    #[test]
    fn idempotency_key_cannot_be_reused_for_different_arguments() {
        let entry = deposit();
        let Purpose::Deposit(plan) = entry.purpose else {
            unreachable!()
        };
        handled(entry.id);
        let first = for_deposit(plan.clone(), Some(b"request"), now_ms()).unwrap();
        handled(first.id);
        let again = state::deposit_entry(
            plan.user,
            "ICP",
            "ETH",
            100,
            None,
            Some(b"request"),
            now_ms() + 10,
        )
        .unwrap();
        assert_eq!(first.id, again.id);
        assert!(
            state::deposit_entry(
                plan.user,
                "ICP",
                "ETH",
                101,
                None,
                Some(b"request"),
                now_ms()
            )
            .is_err()
        );
    }

    #[test]
    fn an_id_can_adopt_and_recover_a_matching_unkeyed_deposit() {
        let owner = Principal::from_slice(&[91, 92, 93]);
        let to = "0x0000000000000000000000000000000000000001";
        let plan = DepositPlan {
            user: owner,
            from: BridgeTarget::Icp,
            to: BridgeTarget::Evm("ETH".into()),
            to_addr: Some(to.into()),
            ledger: Principal::from_slice(&[94]),
            amount: 100,
            fee: 1,
        };
        let original = for_deposit(plan, None, now_ms()).unwrap();
        let other = state::deposit_entry(owner, "ICP", "ETH", 99, Some(to), None, now_ms());
        assert!(other.err().unwrap().contains("resume deposit operation"));
        let adopted = state::deposit_entry(
            owner,
            "ICP",
            "ETH",
            100,
            Some(to),
            Some(b"adopted-request"),
            now_ms(),
        )
        .unwrap();
        assert_eq!(adopted.id, original.id);
        assert_eq!(
            find_request(owner, b"adopted-request").unwrap().unwrap().id,
            original.id
        );
    }

    #[test]
    fn reconciled_external_deposits_are_recoverable_and_old_indexes_are_backfilled() {
        for (from, scheme, tx) in [
            (
                BridgeTarget::Evm("ETH".into()),
                "ecdsa",
                BridgeTx::Evm(true, [73; 32].into()),
            ),
            (
                BridgeTarget::Sol,
                "ed25519",
                BridgeTx::Sol(true, [74; 64].into()),
            ),
        ] {
            let owner = Principal::from_slice(&[75]);
            let entry = create(
                owner,
                Purpose::Deposit(DepositPlan {
                    user: owner,
                    from,
                    to: BridgeTarget::Icp,
                    to_addr: None,
                    ledger: Principal::from_slice(&[76]),
                    amount: 100,
                    fee: 1,
                }),
                now_ms(),
            );
            prepare(
                entry.id,
                Request::Signature {
                    scheme: scheme.into(),
                    key_name: "test".into(),
                    sender: owner,
                    message: vec![0; 32].into(),
                    deadline: TxDeadline::Nonce(0),
                    validity: None,
                },
            )
            .unwrap();
            start_signature(entry.id).unwrap();
            resolve(
                entry.id,
                get(entry.id).unwrap().revision,
                Resolution::Completed(tx),
                "operator verified the finalized external deposit".into(),
                owner,
            )
            .unwrap();
            assert!(open_ids(100).contains(&entry.id));
            // Simulate the index written before Completed Signature deposits
            // were included, then rebuild without changing the operation.
            OPEN.with_borrow_mut(|open| open.remove(&(1, entry.id)));
            let revision = get(entry.id).unwrap().revision;
            migrate_indexes(entry.id - 1, entry.id, 1);
            assert!(open_ids(100).contains(&entry.id));
            assert_eq!(get(entry.id).unwrap().revision, revision);
            handled(entry.id);
            assert!(!open_ids(100).contains(&entry.id));
        }
    }

    struct RejectingLedger(u8);
    impl LedgerTransport for RejectingLedger {
        async fn transfer(
            &self,
            _: Principal,
            _: TransferArg,
        ) -> Result<Result<Nat, TransferError>, CallFailure> {
            Ok(Err(match self.0 {
                0 => TransferError::BadFee {
                    expected_fee: 20u64.into(),
                },
                1 => TransferError::InsufficientFunds {
                    balance: 0u64.into(),
                },
                2 => TransferError::TemporarilyUnavailable,
                _ => TransferError::TooOld,
            }))
        }
        async fn transfer_from(
            &self,
            _: Principal,
            _: TransferFromArgs,
        ) -> Result<Result<Nat, TransferFromError>, CallFailure> {
            Ok(Err(match self.0 {
                0 => TransferFromError::BadFee {
                    expected_fee: 20u64.into(),
                },
                1 => TransferFromError::InsufficientFunds {
                    balance: 0u64.into(),
                },
                2 => TransferFromError::TemporarilyUnavailable,
                _ => TransferFromError::TooOld,
            }))
        }
    }

    #[test]
    fn ledger_errors_distinguish_rejection_retry_and_unknown_execution() {
        STATE.with_borrow_mut(|s| {
            s.icp_collected_fees_migrated = true;
            s.icp_collected_fees = 100_000;
        });
        for mode in 0..4 {
            for uncertain in [false, true] {
                let entry = withdrawal(Principal::from_slice(&[mode, u8::from(uncertain)]), 50);
                prepare(entry.id, transfer_request(&entry, 50)).unwrap();
                if uncertain {
                    assert!(
                        futures::executor::block_on(execute_with(entry.id, &LostReply::new()))
                            .is_err()
                    );
                }
                let before = cbor_into_vec(&get(entry.id).unwrap().request).unwrap();
                assert!(
                    futures::executor::block_on(execute_with(entry.id, &RejectingLedger(mode)))
                        .is_err()
                );
                let after = get(entry.id).unwrap();
                assert_eq!(cbor_into_vec(&after.request).unwrap(), before);
                match (mode, uncertain) {
                    (0 | 1, false) => {
                        assert!(matches!(after.phase, Phase::Rejected(_)));
                        assert!(!after.withdrawal_counted);
                        assert!(safe_to_reset(entry.id));
                    }
                    (2, false) => assert!(matches!(after.phase, Phase::Prepared)),
                    (2, true) => assert!(matches!(after.phase, Phase::Submitted)),
                    _ => assert!(matches!(after.phase, Phase::NeedsReview(_))),
                }
                if uncertain || mode >= 2 {
                    assert!(after.withdrawal_counted);
                }

                let deposit = deposit();
                if uncertain {
                    assert!(
                        futures::executor::block_on(execute_with(deposit.id, &LostReply::new()))
                            .is_err()
                    );
                }
                assert!(
                    futures::executor::block_on(execute_with(deposit.id, &RejectingLedger(mode)))
                        .is_err()
                );
                assert_eq!(
                    std::mem::discriminant(&get(deposit.id).unwrap().phase),
                    std::mem::discriminant(&after.phase)
                );
            }
        }
    }
}
