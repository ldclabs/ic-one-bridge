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
    FeeRecognition {
        total: u128,
        evidence: String,
    },
    Deposit(DepositPlan),
    Payout(u64),
    Withdrawal {
        to: Principal,
        amount: u128,
        ledger: Principal,
    },
    FeeFunding {
        amount: u128,
        ledger: Principal,
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
    Recorded,
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
    pub reserved_fee: u128,
    pub accounting_settled: bool,
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
        Purpose::FeeFunding { .. } => ("fee funding", None),
        Purpose::FeeRecognition { .. } => ("fee reconciliation", None),
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
    static ENTRIES: RefCell<StableBTreeMap<u64, Cbor<Entry>, Memory>> = RefCell::new(StableBTreeMap::init(memory(8)));
    static USERS: RefCell<StableBTreeMap<UserLogKey, (), Memory>> = RefCell::new(StableBTreeMap::init(memory(11)));
    // The key is owner || caller request ID. It outlives completion and upgrades.
    static REQUEST_IDS: RefCell<StableBTreeMap<Vec<u8>, u64, Memory>> = RefCell::new(StableBTreeMap::init(memory(12)));
    static OPEN: RefCell<StableBTreeMap<(u8, u64), (), Memory>> = RefCell::new(StableBTreeMap::init(memory(13)));
}

pub fn get(id: u64) -> Option<Entry> {
    ENTRIES.with_borrow(|entries| entries.get(&id).map(|e| e.0))
}
pub fn put(entry: &Entry) {
    let mut entry = entry.clone();
    entry.revision = get(entry.id).map_or(1, |old| old.revision.saturating_add(1));
    ENTRIES.with_borrow_mut(|entries| {
        entries.insert(entry.id, Cbor(entry.clone()));
    });
    OPEN.with_borrow_mut(|open| {
        open.remove(&(0, entry.id));
        open.remove(&(1, entry.id));
        open.remove(&(2, entry.id));
        if !entry.handled && !matches!(entry.phase, Phase::Rejected(_)) {
            open.insert((0, entry.id), ());
            if !matches!(entry.purpose, Purpose::Payout(_)) {
                open.insert((2, entry.id), ());
            }
            if matches!(
                entry.request,
                Some(Request::TransferFrom { .. } | Request::Transfer { .. })
            ) && matches!(
                entry.phase,
                Phase::Prepared | Phase::Submitted | Phase::Completed(_)
            ) && !matches!(entry.purpose, Purpose::Payout(_))
            {
                open.insert((1, entry.id), ());
            }
        }
    });
    let mut key = vec![255];
    key.extend_from_slice(&UserLogKey::new(&entry.owner, entry.id).0);
    REQUEST_IDS.with_borrow_mut(|ids| {
        if !entry.handled
            && !matches!(entry.phase, Phase::Rejected(_))
            && matches!(
                entry.purpose,
                Purpose::Deposit(_) | Purpose::Withdrawal { .. } | Purpose::FeeFunding { .. }
            )
        {
            ids.insert(key, entry.id);
        } else {
            ids.remove(&key);
        }
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

pub fn open_deposit(owner: Principal) -> Option<Entry> {
    let mut start = vec![255];
    start.extend_from_slice(&UserLogKey::new(&owner, 0).0);
    let mut end = vec![255];
    end.extend_from_slice(&UserLogKey::new(&owner, u64::MAX).0);
    REQUEST_IDS
        .with_borrow(|ids| ids.range(start..end).map(|e| e.value()).collect::<Vec<_>>())
        .into_iter()
        .filter_map(get)
        .find(|e| matches!(e.purpose, Purpose::Deposit(_)))
}

pub fn open_withdrawal(owner: Principal) -> Option<Entry> {
    let mut start = vec![255];
    start.extend_from_slice(&UserLogKey::new(&owner, 0).0);
    let mut end = vec![255];
    end.extend_from_slice(&UserLogKey::new(&owner, u64::MAX).0);
    let ids =
        REQUEST_IDS.with_borrow(|ids| ids.range(start..end).map(|e| e.value()).collect::<Vec<_>>());
    ids.into_iter()
        .filter_map(get)
        .find(|e| matches!(e.purpose, Purpose::Withdrawal { .. }))
}

pub fn open_funding(owner: Principal) -> Option<Entry> {
    let mut start = vec![255];
    start.extend_from_slice(&UserLogKey::new(&owner, 0).0);
    let mut end = vec![255];
    end.extend_from_slice(&UserLogKey::new(&owner, u64::MAX).0);
    REQUEST_IDS
        .with_borrow(|ids| ids.range(start..end).map(|e| e.value()).collect::<Vec<_>>())
        .into_iter()
        .filter_map(get)
        .find(|entry| matches!(entry.purpose, Purpose::FeeFunding { .. }))
}

pub fn find_request(owner: Principal, id: &[u8]) -> Result<Option<Entry>, String> {
    if id.is_empty() || id.len() > 64 {
        return Err("request_id must be 1 to 64 bytes".into());
    }
    let mut key = vec![owner.as_slice().len() as u8];
    key.extend_from_slice(owner.as_slice());
    key.extend_from_slice(id);
    Ok(REQUEST_IDS.with_borrow(|ids| ids.get(&key)).and_then(get))
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
        reserved_fee: 0,
        accounting_settled: false,
        error: None,
        revision: 0,
        call_generation: 0,
        reconciliation: None,
    }
}

pub fn create(owner: Principal, purpose: Purpose, created_at: u64) -> Entry {
    let mut entry = draft(owner, purpose, created_at);
    entry.id = pending::next_id();
    put(&entry);
    USERS.with_borrow_mut(|users| {
        users.insert(UserLogKey::new(&owner, entry.id), ());
    });
    entry
}

pub fn for_deposit(
    plan: DepositPlan,
    request_id: Option<&[u8]>,
    now: u64,
) -> Result<Entry, String> {
    let key = request_id
        .map(|id| {
            if id.is_empty() || id.len() > 64 {
                return Err("request_id must be 1 to 64 bytes".to_string());
            }
            let mut key = vec![plan.user.as_slice().len() as u8];
            key.extend_from_slice(plan.user.as_slice());
            key.extend_from_slice(id);
            Ok(key)
        })
        .transpose()?;
    if let Some(key) = &key
        && let Some(id) = REQUEST_IDS.with_borrow(|ids| ids.get(key))
    {
        let existing = get(id).ok_or_else(|| "operation index is inconsistent".to_string())?;
        if matches!(&existing.purpose, Purpose::Deposit(old) if old == &plan) {
            return Ok(existing);
        }
        return Err("request_id already belongs to different bridge arguments".into());
    }
    // The legacy bridge API can recover its outstanding deposit without a new
    // ingress field. Explicit IDs are recommended for retries after completion.
    if let Some(existing) = open_deposit(plan.user)
        && let Purpose::Deposit(old) = &existing.purpose
    {
        if old == &plan {
            return Ok(existing);
        }
        return Err(format!(
            "resume unresolved deposit operation {} first",
            existing.id
        ));
    }
    let entry = create(plan.user, Purpose::Deposit(plan), now);
    if let Some(key) = key {
        REQUEST_IDS.with_borrow_mut(|ids| {
            ids.insert(key, entry.id);
        });
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

pub fn prepare(id: u64, request: Request, fee: u128) -> Result<(), String> {
    let mut entry = get(id).ok_or_else(|| "operation not found".to_string())?;
    if entry.request.is_some() {
        return Ok(());
    }
    if !matches!(entry.phase, Phase::Planning) {
        return Err("operation cannot be prepared again".into());
    }
    if matches!(request, Request::Transfer { .. }) {
        STATE.with_borrow_mut(|s| {
            let amount = match &entry.purpose {
                Purpose::Withdrawal { amount, .. } => *amount,
                _ => 0,
            };
            let needed = fee
                .checked_add(amount)
                .ok_or_else(|| "fee reservation overflow".to_string())?;
            if available_operating_funds(s) < needed {
                return Err(
                    "ICP: fund the ledger fee budget before transferring or withdrawing"
                        .to_string(),
                );
            }
            if amount > available_withdrawal(s) {
                return Err("withdrawal exceeds earned ICP fees".to_string());
            }
            s.reserved_icp_fees = s
                .reserved_icp_fees
                .checked_add(fee)
                .ok_or_else(|| "fee reservation overflow".to_string())?;
            s.total_withdrawn_fees = s
                .total_withdrawn_fees
                .checked_add(amount)
                .ok_or_else(|| "withdrawal overflow".to_string())?;
            Ok::<_, String>(())
        })?;
        entry.reserved_fee = fee;
    }
    entry.request = Some(request);
    entry.phase = Phase::Prepared;
    put(&entry);
    Ok(())
}

pub fn available_operating_funds(s: &State) -> u128 {
    if !s.icp_collected_fees_migrated {
        return 0;
    }
    s.spendable_icp_fees
        .saturating_add(s.ledger_fee_credit)
        .saturating_sub(
            s.total_withdrawn_fees
                .saturating_sub(s.withdrawals_baseline),
        )
        .saturating_sub(s.icp_transfer_fees)
        .saturating_sub(s.reserved_icp_fees)
}
pub fn available_withdrawal(s: &State) -> u128 {
    s.spendable_icp_fees
        .saturating_sub(
            s.total_withdrawn_fees
                .saturating_sub(s.withdrawals_baseline),
        )
        .min(available_operating_funds(s))
}

fn settle_accounting(entry: &mut Entry, success: bool) {
    if entry.accounting_settled {
        return;
    }
    STATE.with_borrow_mut(|s| {
        if entry.reserved_fee > 0 {
            s.reserved_icp_fees = s.reserved_icp_fees.saturating_sub(entry.reserved_fee);
            if success {
                s.icp_transfer_fees = s.icp_transfer_fees.saturating_add(entry.reserved_fee);
            }
        }
        if !success
            && matches!(entry.request, Some(Request::Transfer { .. }))
            && let Purpose::Withdrawal { amount, .. } = &entry.purpose
        {
            s.total_withdrawn_fees = s.total_withdrawn_fees.saturating_sub(*amount);
        }
        if success && let Purpose::FeeFunding { amount, .. } = &entry.purpose {
            s.ledger_fee_credit = s.ledger_fee_credit.saturating_add(*amount);
        }
    });
    entry.accounting_settled = true;
}

pub fn completed(id: u64, tx: BridgeTx) -> Result<BridgeTx, String> {
    let mut entry = get(id).ok_or_else(|| "operation not found".to_string())?;
    if let Phase::Completed(existing) = entry.phase {
        return Ok(existing);
    }
    if matches!(
        entry.purpose,
        Purpose::Deposit(_) | Purpose::FeeFunding { .. }
    ) {
        pending::remember_transaction(&tx, id)?;
    }
    settle_accounting(&mut entry, true);
    entry.phase = Phase::Completed(tx.clone());
    entry.error = None;
    put(&entry);
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
        put(&entry);
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
            settle_accounting(&mut entry, false);
            Phase::Rejected(error.clone())
        };
        put(&entry);
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
    put(&entry); // commits before the call
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
    put(&entry);
    Ok(request)
}

pub fn record_signed(
    id: u64,
    tx: BridgeTx,
    meta: TxMeta,
    signature: Vec<u8>,
) -> Result<(), String> {
    let mut entry = get(id).ok_or_else(|| "signature intent not found".to_string())?;
    if !matches!(entry.phase, Phase::Submitted | Phase::NeedsReview(_)) {
        return Err("signature operation has already been resolved".into());
    }
    entry.phase = Phase::Signed;
    entry.signed = Some((tx, meta));
    entry.signature = Some(signature.into());
    put(&entry);
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
        #[cfg(not(test))]
        if wake {
            state::schedule_finalize(Duration::ZERO);
        }
        #[cfg(test)]
        let _ = wake;
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

pub fn record_fee_recognition(controller: Principal, total: u128, evidence: String) {
    let mut entry = create(
        controller,
        Purpose::FeeRecognition { total, evidence },
        now_ms(),
    );
    entry.phase = Phase::Recorded;
    entry.handled = true;
    entry.accounting_settled = true;
    put(&entry);
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
        if matches!(
            entry.purpose,
            Purpose::Deposit(_) | Purpose::FeeFunding { .. }
        ) && pending::known_transaction(tx).is_some_and(|id| id != entry.id)
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
    put(&entry);
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
            0,
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
        let reopened: StableBTreeMap<u64, Cbor<Entry>, Memory> = StableBTreeMap::init(memory(8));
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
    #[test]
    fn unknown_withdrawal_keeps_reservation_and_accounts_fee_exactly_once() {
        STATE.with_borrow_mut(|s| {
            s.icp_collected_fees_migrated = true;
            s.icp_collected_fees = 200;
            s.spendable_icp_fees = 200;
        });
        let owner = Principal::from_slice(&[1]);
        let target = Principal::from_slice(&[2]);
        let entry = create(
            owner,
            Purpose::Withdrawal {
                to: owner,
                amount: 50,
                ledger: target,
            },
            now_ms(),
        );
        prepare(
            entry.id,
            Request::Transfer {
                ledger: target,
                args: TransferArg {
                    from_subaccount: None,
                    to: Account {
                        owner,
                        subaccount: None,
                    },
                    amount: 50u64.into(),
                    fee: Some(10u64.into()),
                    memo: Some(memo(entry.id)),
                    created_at_time: Some(entry.created_at * 1_000_000),
                },
            },
            10,
        )
        .unwrap();
        let ledger = LostReply::new();
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_err());
        assert_eq!(
            STATE.with_borrow(|s| (
                s.total_withdrawn_fees,
                s.reserved_icp_fees,
                available_withdrawal(s)
            )),
            (50, 10, 140)
        );
        assert!(futures::executor::block_on(execute_with(entry.id, &ledger)).is_ok());
        failed(entry.id, "late error".into(), false, false);
        assert_eq!(
            STATE.with_borrow(|s| (
                s.total_withdrawn_fees,
                s.reserved_icp_fees,
                s.icp_transfer_fees,
                available_withdrawal(s)
            )),
            (50, 0, 10, 140)
        );
        assert_eq!(ledger.debits.get(), 1);
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
            0,
        )
        .unwrap();
        assert!(start_signature(entry.id).is_ok());
        assert!(start_signature(entry.id).is_err());
        assert!(!safe_to_reset(entry.id));
    }
    #[test]
    fn idempotency_key_cannot_be_reused_for_different_arguments() {
        let entry = deposit();
        let Purpose::Deposit(plan) = entry.purpose else {
            unreachable!()
        };
        handled(entry.id);
        let first = for_deposit(plan.clone(), Some(b"request"), now_ms()).unwrap();
        let again = for_deposit(plan.clone(), Some(b"request"), now_ms() + 10).unwrap();
        assert_eq!(first.id, again.id);
        let mut different = plan;
        different.amount += 1;
        assert!(for_deposit(different, Some(b"request"), now_ms()).is_err());
    }
}
