//! Incremental migration follows the authoritative archive, not a nonempty-index
//! heuristic. Its cursor survives a downgrade, so later legacy appends are found.
use super::*;

const BATCH_SIZE: usize = 100;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Progress {
    next_archive: u64,
    icp_fees: u128,
}
thread_local! {
    static PROGRESS: RefCell<StableCell<Cbor<Progress>, Memory>> = RefCell::new(StableCell::init(memory(9), Cbor(Progress::default())));
}

pub fn archive_appended(id: u64, log: &BridgeLog) {
    PROGRESS.with_borrow_mut(|cell| {
        let mut p = cell.get().0.clone();
        if p.next_archive == id {
            if log.from == BridgeTarget::Icp && log.is_finalized() {
                p.icp_fees = p.icp_fees.saturating_add(log.fee);
            }
            p.next_archive += 1;
            cell.set(Cbor(p));
        }
    });
}

/// At most `limit` records are decoded and indexed in one message. The legacy
/// heap queue is read only during its one-time conversion; new tasks bypass it.
pub fn step(limit: usize) -> bool {
    let mut left = limit;
    while left > 0 {
        let old = STATE.with_borrow_mut(|s| s.legacy_pending.pop_front());
        let Some(mut task) = old else {
            break;
        };
        if let Some(existing) = pending::by_tx(&task.from_tx) {
            let conflict = journal::record_legacy_conflict(existing.task_id, task);
            pending::update(existing.task_id, |t| {
                t.stuck = true;
                t.error = Some(format!(
                    "duplicate legacy deposit record is preserved as operation {}; reconcile it before continuing this task",
                    conflict.id
                ));
                t.error_chain = None;
            });
        } else {
            if task.task_id == 0 {
                task.task_id = pending::next_id();
            }
            task.ledger = task
                .ledger
                .or_else(|| STATE.with_borrow(|s| Some(s.token_ledger)));
            if task.error_chain.is_none()
                && let Some(error) = &task.error
            {
                let error_chain = [&task.from, &task.to]
                    .into_iter()
                    .find(|c| error.starts_with(&format!("{}:", c.name())))
                    .cloned();
                task.error_chain = error_chain;
            }
            pending::insert(&task);
        }
        left -= 1;
    }
    let mut progress = PROGRESS.with_borrow(|cell| cell.get().0.clone());
    while left > 0 {
        let log = BRIDGE_LOGS.with_borrow(|logs| logs.get(progress.next_archive));
        let Some(log) = log else {
            break;
        };
        if let Some(task) = pending::by_tx(&log.from_tx) {
            pending::update(task.task_id, |t| {
                t.stuck = true;
                t.error = Some("incoming transaction also appears in the archive; reconcile duplicate legacy records".into());
            });
        }
        if pending::known_transaction(&log.from_tx).is_none() {
            let id = if log.task_id == 0 {
                pending::next_id()
            } else {
                log.task_id
            };
            pending::remember_transaction(&log.from_tx, id).expect("archive source identity");
        }
        USER_LOG_INDEX.with_borrow_mut(|index| {
            index.insert(UserLogKey::new(&log.user, progress.next_archive), ());
        });
        if log.from == BridgeTarget::Icp
            && log.from_tx.is_finalized()
            && log.to_tx.as_ref().is_some_and(BridgeTx::is_finalized)
        {
            progress.icp_fees = progress.icp_fees.saturating_add(log.fee);
        }
        progress.next_archive += 1;
        left -= 1;
    }
    let done = STATE.with_borrow(|s| s.legacy_pending.is_empty())
        && BRIDGE_LOGS.with_borrow(|logs| progress.next_archive == logs.len());
    PROGRESS.with_borrow_mut(|cell| {
        cell.set(Cbor(progress.clone()));
    });
    STATE.with_borrow_mut(|s| {
        s.icp_collected_fees_migrated = done;
        if done {
            s.icp_collected_fees = progress.icp_fees;
        }
    });
    done
}

pub fn start() {
    STATE.with_borrow_mut(|s| s.icp_collected_fees_migrated = false);
    if step(BATCH_SIZE) {
        return;
    }
    ic_cdk_timers::set_timer(Duration::ZERO, async {
        continue_migration();
    });
}
fn continue_migration() {
    if !step(BATCH_SIZE) {
        ic_cdk_timers::set_timer(Duration::ZERO, async {
            continue_migration();
        });
    } else {
        state::schedule_finalize(Duration::ZERO);
    }
}

pub fn remaining() -> u64 {
    let cursor = PROGRESS.with_borrow(|cell| cell.get().0.next_archive);
    BRIDGE_LOGS
        .with_borrow(|logs| logs.len().saturating_sub(cursor))
        .saturating_add(STATE.with_borrow(|s| s.legacy_pending.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_migration_is_bounded_and_catches_appends_after_a_downgrade() {
        let user = Principal::from_slice(&[6]);
        for id in 0..5 {
            let mut task = super::super::tests::log(
                user,
                BridgeTarget::Icp,
                BridgeTarget::Evm("ETH".into()),
                BridgeTx::Icp(true, id),
            );
            task.to_tx = Some(BridgeTx::Evm(true, [id as u8; 32].into()));
            BRIDGE_LOGS.with_borrow(|logs| logs.append(&task.into()).unwrap());
        }
        assert!(!step(2));
        assert_eq!(remaining(), 3);
        assert!(!step(2));
        assert!(step(2));
        assert_eq!(STATE.with_borrow(|s| s.icp_collected_fees), 5);
        let mut task = super::super::tests::log(
            user,
            BridgeTarget::Icp,
            BridgeTarget::Evm("ETH".into()),
            BridgeTx::Icp(true, 99),
        );
        task.to_tx = Some(BridgeTx::Evm(true, [99; 32].into()));
        // The older version knows nothing of PROGRESS and appends directly.
        BRIDGE_LOGS.with_borrow(|logs| logs.append(&task.into()).unwrap());
        assert!(step(2));
        assert_eq!(STATE.with_borrow(|s| s.icp_collected_fees), 6);
        assert_eq!(state::user_logs(user, 100, None).len(), 6);
        assert!(step(2));
        assert_eq!(STATE.with_borrow(|s| s.icp_collected_fees), 6);
    }

    #[test]
    fn conflicting_legacy_pending_records_are_preserved_for_reconciliation() {
        pending::reset();
        let user = Principal::from_slice(&[81, 82, 83]);
        let mut first = super::super::tests::log(
            user,
            BridgeTarget::Sol,
            BridgeTarget::Evm("ETH".into()),
            BridgeTx::Sol(false, [44; 64].into()),
        );
        first.to_addr = Some("0x0000000000000000000000000000000000000001".into());
        let mut second = first.clone();
        second.to_addr = Some("0x0000000000000000000000000000000000000002".into());
        STATE.with_borrow_mut(|s| s.legacy_pending = VecDeque::from([first, second.clone()]));

        let _ = step(2);

        let live = pending::by_tx(&second.from_tx).expect("first legacy record");
        assert!(live.stuck);
        let conflict = journal::page(user, 100, None)
            .into_iter()
            .find(|entry| entry.kind == "legacy pending conflict")
            .expect("preserved conflicting record");
        assert_eq!(
            conflict.related_task.and_then(|task| task.to_addr),
            second.to_addr
        );
        assert!(journal::unresolved());
    }
}
