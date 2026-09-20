//! Pending tasks live in stable memory. A single secondary map namespaces the
//! user, incoming transaction, due-time, EVM reservation and chain-error indexes.
//! Namespaces 5 and 6 hold immutable archive identities and reconciliation
//! holds; rewriting a task's retry flags cannot erase either financial control.
use super::*;
use std::ops::Bound::{Excluded, Included, Unbounded};

thread_local! {
    static TASKS: RefCell<StableBTreeMap<u64, BridgeLogLocal, Memory>> = RefCell::new(StableBTreeMap::init(memory(5)));
    static INDEX: RefCell<StableBTreeMap<Vec<u8>, u64, Memory>> = RefCell::new(StableBTreeMap::init(memory(6)));
    static NEXT_ID: RefCell<StableCell<u64, Memory>> = RefCell::new(StableCell::init(memory(7), 1));
}

pub fn next_id() -> u64 {
    NEXT_ID.with_borrow_mut(|cell| {
        let id = *cell.get();
        cell.set(id.checked_add(1).expect("operation ID exhausted"));
        id
    })
}
#[cfg(test)]
pub fn id_high_water() -> u64 {
    NEXT_ID.with_borrow(|cell| *cell.get())
}

fn user_prefix(user: &Principal) -> Vec<u8> {
    let mut prefix = vec![0];
    prefix.extend_from_slice(&UserLogKey::new(user, 0).0[..UserLogKey::SIZE - 8]);
    prefix
}

fn transaction_key(tx: &BridgeTx) -> Vec<u8> {
    let mut key = vec![1];
    match tx {
        BridgeTx::Icp(_, id) => {
            key.push(0);
            key.extend_from_slice(&id.to_be_bytes());
        }
        BridgeTx::Evm(_, hash) => {
            key.push(1);
            key.extend_from_slice(hash.as_slice());
        }
        BridgeTx::Sol(_, hash) => {
            key.push(2);
            key.extend_from_slice(hash.as_slice());
        }
    }
    key
}

fn archive_key(tx: &BridgeTx) -> Vec<u8> {
    let mut key = transaction_key(tx);
    key[0] = 5;
    key
}

pub fn archived_source(tx: &BridgeTx) -> Option<u64> {
    INDEX.with_borrow(|index| index.get(&archive_key(tx)))
}

pub fn record_archived_source(tx: &BridgeTx, archive_id: u64) {
    INDEX.with_borrow_mut(|index| {
        let key = archive_key(tx);
        if !index.contains_key(&key) {
            index.insert(key, archive_id);
        }
    });
    if let Some(task) = by_tx(tx) {
        update(task.task_id, |_| ());
    }
}

pub fn set_conflict_hold(task_id: u64, operation_id: u64, unresolved: bool) {
    INDEX.with_borrow_mut(|index| {
        let key = suffix(suffix(vec![6], task_id), operation_id);
        if unresolved {
            index.insert(key, operation_id);
        } else {
            index.remove(&key);
        }
    });
    if unresolved {
        update(task_id, |_| ());
    }
}

pub fn reconciliation_error(task: &BridgeLog) -> Option<String> {
    if let Some(id) = archived_source(&task.from_tx) {
        return Some(format!(
            "incoming transaction already appears in archive {id}; reconcile and close the duplicate task without another payout"
        ));
    }
    prefix_ids(suffix(vec![6], task.task_id), 1, None)
        .first()
        .map(|id| format!("resolve legacy conflict operation {id} before resuming this task"))
}

fn chain_prefix(kind: u8, chain: &str) -> Vec<u8> {
    let mut key = vec![kind];
    key.extend_from_slice(chain.as_bytes());
    key.push(0);
    key
}

fn suffix(mut prefix: Vec<u8>, id: u64) -> Vec<u8> {
    prefix.extend_from_slice(&id.to_be_bytes());
    prefix
}

fn keys(task: &BridgeLog) -> Vec<Vec<u8>> {
    let mut keys = vec![
        suffix(user_prefix(&task.user), task.task_id),
        transaction_key(&task.from_tx),
    ];
    if !task.stuck {
        keys.push(suffix(suffix(vec![2], task.next_poll_at), task.task_id));
    }
    if task.payout_may_execute()
        && let BridgeTarget::Evm(chain) = &task.to
    {
        keys.push(suffix(chain_prefix(3, chain), task.task_id));
    }
    if task.has_transient_error()
        && let Some(chain) = &task.error_chain
    {
        keys.push(suffix(chain_prefix(4, chain.name()), task.task_id));
    }
    keys
}

fn prefix_ids(prefix: Vec<u8>, take: usize, after: Option<u64>) -> Vec<u64> {
    let mut upper = prefix.clone();
    let at = upper
        .iter()
        .rposition(|b| *b != 255)
        .expect("index namespace");
    upper[at] += 1;
    upper.truncate(at + 1);
    let lower = after
        .map(|id| Excluded(suffix(prefix.clone(), id)))
        .unwrap_or(Included(prefix));
    INDEX.with_borrow(|index| {
        index
            .range((lower, Excluded(upper)))
            .take(take)
            .map(|e| e.value())
            .collect()
    })
}

pub fn len() -> u64 {
    TASKS.with_borrow(|tasks| tasks.len())
}
pub fn is_empty() -> bool {
    len() == 0
}
pub fn get(id: u64) -> Option<BridgeLog> {
    TASKS.with_borrow(|tasks| tasks.get(&id).map(Into::into))
}
pub fn by_tx(tx: &BridgeTx) -> Option<BridgeLog> {
    INDEX
        .with_borrow(|index| index.get(&transaction_key(tx)))
        .and_then(get)
}
pub fn known_transaction(tx: &BridgeTx) -> Option<u64> {
    INDEX.with_borrow(|index| index.get(&transaction_key(tx)))
}
pub fn remember_transaction(tx: &BridgeTx, id: u64) -> Result<(), String> {
    INDEX.with_borrow_mut(|index| {
        let key = transaction_key(tx);
        if index.get(&key).is_some_and(|old| old != id) {
            return Err("incoming transaction already belongs to another operation".into());
        }
        index.insert(key, id);
        Ok(())
    })
}

pub fn insert(task: &BridgeLog) {
    let mut task = task.clone();
    if let Some(error) = reconciliation_error(&task) {
        task.stuck = true;
        task.error = Some(error);
        task.error_chain = None;
    }
    assert_ne!(task.task_id, 0, "pending tasks require a stable ID");
    remember_transaction(&task.from_tx, task.task_id).expect("duplicate incoming transaction");
    if let Some(old) = get(task.task_id) {
        INDEX.with_borrow_mut(|index| {
            for key in keys(&old) {
                index.remove(&key);
            }
        });
    }
    TASKS.with_borrow_mut(|tasks| {
        tasks.insert(task.task_id, task.clone().into());
    });
    INDEX.with_borrow_mut(|index| {
        for key in keys(&task) {
            index.insert(key, task.task_id);
        }
    });
}

pub fn update<R>(id: u64, f: impl FnOnce(&mut BridgeLog) -> R) -> Option<R> {
    let mut task = get(id)?;
    let result = f(&mut task);
    insert(&task);
    Some(result)
}

pub fn remove(id: u64) {
    if let Some(task) = get(id) {
        INDEX.with_borrow_mut(|index| {
            for key in keys(&task) {
                if key.first() != Some(&1) {
                    index.remove(&key);
                }
            }
        });
        TASKS.with_borrow_mut(|tasks| {
            tasks.remove(&id);
        });
    }
}

pub fn page(user: Option<Principal>, take: usize, after: Option<u64>) -> Vec<BridgeLog> {
    let take = take.min(PENDING_LOGS_LIMIT);
    let ids = match user {
        Some(user) => prefix_ids(user_prefix(&user), take, after),
        None => TASKS.with_borrow(|tasks| {
            tasks
                .keys_range((after.map(Excluded).unwrap_or(Unbounded), Unbounded))
                .take(take)
                .collect()
        }),
    };
    ids.into_iter()
        .filter_map(get)
        .map(BridgeLog::public_view)
        .collect()
}

pub fn user_count(user: Principal, cap: usize) -> usize {
    prefix_ids(user_prefix(&user), cap, None).len()
}
pub fn unconfirmed_deposit(user: Principal, chain: &BridgeTarget) -> bool {
    prefix_ids(user_prefix(&user), 1001, None)
        .into_iter()
        .filter_map(get)
        .any(|task| task.from == *chain && !task.from_tx.is_finalized())
}
pub fn chain_error(chain: &BridgeTarget) -> Option<String> {
    prefix_ids(chain_prefix(4, chain.name()), 1, None)
        .into_iter()
        .find_map(get)
        .and_then(|t| t.error)
}

pub fn chain_reserved_by_other(chain: &str, id: u64) -> bool {
    prefix_ids(chain_prefix(3, chain), 2, None)
        .into_iter()
        .any(|owner| owner != id)
}

pub fn next_due() -> Option<u64> {
    INDEX.with_borrow(|index| {
        index
            .range(vec![2]..vec![3])
            .next()
            .map(|e| u64::from_be_bytes(e.key()[1..9].try_into().expect("due key")))
    })
}

/// Administrative restarts and upgrades wake existing backoffs in bounded
/// batches. No growing queue is serialized or rewritten in a single message.
pub fn wake_all() {
    wake_after(None);
}
fn wake_after(after: Option<u64>) {
    let ids = TASKS.with_borrow(|tasks| {
        tasks
            .keys_range((after.map(Excluded).unwrap_or(Unbounded), Unbounded))
            .take(128)
            .collect::<Vec<_>>()
    });
    for id in &ids {
        update(*id, |task| {
            if !task.stuck {
                task.next_poll_at = now_ms();
                task.poll_attempts = 0;
            }
        });
    }
    #[cfg(not(test))]
    if ids.len() == 128 {
        let last = *ids.last().expect("batch");
        ic_cdk_timers::set_timer(Duration::ZERO, async move {
            wake_after(Some(last));
        });
    }
}

/// Bounded work and fair due-time ordering; reservations do not confer global
/// priority. Blocked tasks move behind other ready tasks while their chain waits.
pub fn select_due(now: u64, limit: usize) -> Vec<BridgeLog> {
    let due: Vec<u64> = INDEX.with_borrow(|index| {
        index
            .range(vec![2]..vec![3])
            .take(128)
            .take_while(|e| u64::from_be_bytes(e.key()[1..9].try_into().expect("due key")) <= now)
            .map(|e| e.value())
            .collect()
    });
    let mut chains = HashSet::new();
    let mut picked = Vec::new();
    for id in due {
        let Some(task) = get(id) else { continue };
        if let BridgeTarget::Evm(chain) = &task.to
            && ((!task.payout_may_execute() && chain_reserved_by_other(chain, id))
                || !chains.insert(chain.clone()))
        {
            update(id, |t| t.next_poll_at = now.saturating_add(3_000));
            continue;
        }
        picked.push(task);
        if picked.len() == limit {
            break;
        }
    }
    picked
}

#[cfg(test)]
pub fn reset() {
    let ids = TASKS.with_borrow(|tasks| tasks.keys().collect::<Vec<_>>());
    for id in ids {
        remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(chain: &str) -> BridgeLog {
        let id = next_id();
        let mut task = super::super::tests::log(
            Principal::from_slice(&[1]),
            BridgeTarget::Icp,
            if chain == "SOL" {
                BridgeTarget::Sol
            } else {
                BridgeTarget::Evm(chain.into())
            },
            BridgeTx::Icp(true, id),
        );
        task.task_id = id;
        task
    }
    #[test]
    fn three_in_flight_evm_tasks_do_not_starve_solana() {
        reset();
        for chain in ["ETH", "BNB", "BASE"] {
            let mut t = task(chain);
            t.to_tx = Some(BridgeTx::Evm(false, [t.task_id as u8; 32].into()));
            insert(&t);
        }
        let healthy = task("SOL");
        insert(&healthy);
        let first = select_due(0, 3);
        assert_eq!(first.len(), 3);
        for t in first {
            update(t.task_id, |t| t.next_poll_at = 3_000);
        }
        let next = select_due(1_000, 3);
        assert!(next.iter().any(|t| t.task_id == healthy.task_id));
    }
    #[test]
    fn reservations_block_only_the_same_chain_and_errors_use_identity() {
        reset();
        let mut old = task("ETH");
        old.to_tx = Some(BridgeTx::Evm(false, [1; 32].into()));
        old.next_poll_at = 30_000;
        insert(&old);
        let blocked = task("ETH");
        insert(&blocked);
        let mut other = task("SOL");
        other.error = Some("provider unavailable".into());
        other.error_chain = Some(BridgeTarget::Evm("ETHW".into()));
        insert(&other);
        assert!(chain_error(&BridgeTarget::Evm("ETH".into())).is_none());
        assert!(chain_error(&BridgeTarget::Evm("ETHW".into())).is_some());
        let selected = select_due(0, 3);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].task_id, other.task_id);
    }
    #[test]
    fn pages_are_bounded_and_do_not_expose_signed_bytes() {
        reset();
        let mut t = task("SOL");
        t.from_meta = Some(TxMeta {
            deadline: TxDeadline::Nonce(1),
            raw: Some(vec![7; 10].into()),
            svm_validity: None,
        });
        insert(&t);
        let page = page(Some(t.user), 1, None);
        assert_eq!(page.len(), 1);
        assert!(page[0].from_meta.as_ref().unwrap().raw.is_none());
        assert!(get(t.task_id).unwrap().from_meta.unwrap().raw.is_some());
        assert!(self::super::page(Some(t.user), 1, Some(t.task_id)).is_empty());
    }
}
