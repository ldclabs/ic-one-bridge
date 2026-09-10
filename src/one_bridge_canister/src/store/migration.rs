//! Incremental migration follows the authoritative archive, not a nonempty-index
//! heuristic. Its cursor survives a downgrade, so later legacy appends are found.
use super::*;

const BATCH_SIZE: usize = 100;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Progress {
    next_archive: u64,
    icp_fees: u128,
    #[serde(default)]
    archive_index_version: u8,
    #[serde(default)]
    next_journal: u64,
    #[serde(default)]
    journal_target: u64,
}
thread_local! {
    static PROGRESS: RefCell<StableCell<Cbor<Progress>, Memory>> = RefCell::new(StableCell::init(memory(9), Cbor(Progress::default())));
}

pub fn archive_appended(id: u64, log: &BridgeLog) {
    pending::record_archived_source(&log.from_tx, id);
    PROGRESS.with_borrow_mut(|cell| {
        let mut p = cell.get().0.clone();
        if p.next_archive == id {
            if log.from == BridgeTarget::Icp
                && log.is_finalized()
                && pending::archived_source(&log.from_tx) == Some(id)
            {
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
    initialize_indexes();
    let mut left = limit;
    while left > 0 {
        let old = STATE.with_borrow_mut(|s| s.legacy_pending.pop_front());
        let Some(mut task) = old else {
            break;
        };
        if let Some(existing_id) = pending::known_transaction(&task.from_tx) {
            // Also preserve conflicts with an already-archived tombstone. There
            // need not be a live pending task for the original source anymore.
            journal::record_legacy_conflict(existing_id, task);
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
    let (cursor, processed) =
        journal::migrate_conflict_holds(progress.next_journal, progress.journal_target, left);
    progress.next_journal = cursor;
    left -= processed;
    while left > 0 {
        let log = BRIDGE_LOGS.with_borrow(|logs| logs.get(progress.next_archive));
        let Some(log) = log else {
            break;
        };
        pending::record_archived_source(&log.from_tx, progress.next_archive);
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
            && pending::archived_source(&log.from_tx) == Some(progress.next_archive)
        {
            progress.icp_fees = progress.icp_fees.saturating_add(log.fee);
        }
        progress.next_archive += 1;
        left -= 1;
    }
    let done = STATE.with_borrow(|s| s.legacy_pending.is_empty())
        && progress.next_journal >= progress.journal_target
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

fn initialize_indexes() {
    PROGRESS.with_borrow_mut(|cell| {
        let mut progress = cell.get().0.clone();
        if progress.archive_index_version == 0 {
            // Existing 26dda2b/5d1fcd9 cursors predate the terminal source index.
            // Revisit the archive in bounded batches instead of trusting that
            // a populated transaction index represents a completed payment.
            progress.next_archive = 0;
            progress.icp_fees = 0;
            progress.archive_index_version = 1;
            progress.journal_target = journal::last_id();
            cell.set(Cbor(progress));
        }
    });
}

pub fn start() {
    STATE.with_borrow_mut(|s| s.icp_collected_fees_migrated = false);
    initialize_indexes();
    PROGRESS.with_borrow_mut(|cell| {
        let mut progress = cell.get().0.clone();
        progress.journal_target = journal::last_id();
        cell.set(Cbor(progress));
    });
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
    let progress = PROGRESS.with_borrow(|cell| cell.get().0.clone());
    BRIDGE_LOGS
        .with_borrow(|logs| logs.len().saturating_sub(progress.next_archive))
        .saturating_add(STATE.with_borrow(|s| s.legacy_pending.len() as u64))
        // Operation IDs are sparse, so this is a conservative work estimate.
        .saturating_add(
            progress
                .journal_target
                .saturating_sub(progress.next_journal),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(source: u64) -> BridgeLog {
        super::super::tests::log(
            Principal::from_slice(&[41, 42, 43]),
            BridgeTarget::Icp,
            BridgeTarget::Evm("BNB".into()),
            BridgeTx::Icp(true, source),
        )
    }

    #[test]
    fn completed_archive_holds_survive_flag_rewrites_and_rechecks() {
        let mut paid = task(70_001);
        paid.to_tx = Some(BridgeTx::Evm(true, [17; 32].into()));
        BRIDGE_LOGS.with_borrow(|logs| logs.append(&paid.clone().into()).unwrap());
        let pending = task(70_001);
        STATE.with_borrow_mut(|s| s.legacy_pending.push_back(pending));

        // The pending entry can be converted before the archive is scanned.
        assert!(!step(1));
        let live = pending::by_tx(&paid.from_tx).unwrap();
        assert!(
            state::can_retry_task(&live)
                .unwrap_err()
                .contains("migration")
        );
        assert!(step(1));
        assert_eq!(pending::archived_source(&paid.from_tx), Some(0));
        let mut live = pending::by_tx(&paid.from_tx).unwrap();
        assert!(live.stuck);
        assert!(state::recheck_task(&live.from_tx, live.user, false).is_err());
        assert!(state::recheck_task(&live.from_tx, live.user, true).is_err());
        assert!(state::can_retry_task(&live).is_err());

        // A stale task copy or another internal writer cannot erase the hold.
        live.stuck = false;
        live.error = None;
        pending::insert(&live);
        assert!(pending::get(live.task_id).unwrap().stuck);
        STATE.with_borrow_mut(|s| s.finalize_bridging_round.1 = true);
        let generation = next_finalize_run_generation();
        assert!(matches!(
            claim_pending_payout(
                generation,
                &live.from_tx,
                &(
                    BridgeTx::Evm(false, [18; 32].into()),
                    TxMeta {
                        deadline: TxDeadline::Nonce(0),
                        raw: None,
                        svm_validity: None
                    }
                ),
                now_ms()
            ),
            PayoutClaim::ReconciliationRequired(_)
        ));
        pending::remove(live.task_id);
        assert_eq!(pending::archived_source(&paid.from_tx), Some(0));
    }

    #[test]
    fn old_completed_cursor_is_revisited_and_duplicate_fees_are_not_counted_twice() {
        let mut paid = task(70_002);
        paid.to_tx = Some(BridgeTx::Evm(true, [19; 32].into()));
        for _ in 0..2 {
            BRIDGE_LOGS.with_borrow(|logs| logs.append(&paid.clone().into()).unwrap());
        }
        // This is the schema written before the terminal archive index existed.
        PROGRESS.with_borrow_mut(|p| {
            p.set(Cbor(Progress {
                next_archive: 2,
                icp_fees: 2,
                ..Default::default()
            }));
        });
        assert!(!step(1));
        assert!(step(1));
        assert_eq!(STATE.with_borrow(|s| s.icp_collected_fees), paid.fee);
        let id = BRIDGE_LOGS.with_borrow(|logs| logs.append(&paid.clone().into()).unwrap());
        archive_appended(id, &paid);
        assert!(step(1));
        assert_eq!(STATE.with_borrow(|s| s.icp_collected_fees), paid.fee);
    }

    #[test]
    fn an_existing_tombstone_preserves_a_late_legacy_duplicate_without_reinserting_it() {
        let mut paid = task(70_003);
        paid.to_tx = Some(BridgeTx::Evm(true, [20; 32].into()));
        BRIDGE_LOGS.with_borrow(|logs| logs.append(&paid.into()).unwrap());
        assert!(step(1));
        let duplicate = task(70_003);
        STATE.with_borrow_mut(|s| s.legacy_pending.push_back(duplicate.clone()));
        assert!(step(1));
        assert!(pending::by_tx(&duplicate.from_tx).is_none());
        let operations = journal::page(duplicate.user, 100, None);
        assert!(
            operations
                .iter()
                .any(|op| op.kind == "legacy pending conflict")
        );
    }

    #[test]
    fn old_conflict_holds_are_backfilled_and_each_requires_resolution() {
        let mut live = task(70_004);
        live.task_id = pending::next_id();
        pending::insert(&live);
        let a = journal::record_legacy_conflict(live.task_id, live.clone());
        let b = journal::record_legacy_conflict(live.task_id, live.clone());
        // Simulate rows from 5d1fcd9, which did not write the hold namespace.
        pending::set_conflict_hold(live.task_id, a.id, false);
        pending::set_conflict_hold(live.task_id, b.id, false);
        assert!(pending::reconciliation_error(&live).is_none());
        assert!(!step(1));
        assert!(pending::reconciliation_error(&live).is_some());
        assert!(step(1));
        for (position, entry) in [a, b].into_iter().enumerate() {
            journal::resolve(
                entry.id,
                journal::get(entry.id).unwrap().revision,
                Resolution::NotExecuted,
                "operator reconciled the duplicate legacy record".into(),
                live.user,
            )
            .unwrap();
            assert_eq!(
                pending::reconciliation_error(&live).is_some(),
                position == 0
            );
        }
        assert!(state::can_retry_task(&pending::get(live.task_id).unwrap()).is_ok());
    }

    #[test]
    fn externally_confirmed_quarantined_payout_can_be_closed_without_repaying() {
        let mut paid = task(70_005);
        paid.to_tx = Some(BridgeTx::Evm(true, [21; 32].into()));
        BRIDGE_LOGS.with_borrow(|logs| logs.append(&paid.clone().into()).unwrap());
        let mut live = task(70_005);
        live.task_id = pending::next_id();
        live.to_tx = Some(BridgeTx::Evm(false, [22; 32].into()));
        let entry = journal::create(live.user, journal::Purpose::Payout(live.task_id), now_ms());
        journal::prepare(
            entry.id,
            journal::Request::LegacyPayout(Box::new(live.clone())),
            0,
        )
        .unwrap();
        live.payout_attempt = Some(entry.id);
        pending::insert(&live);
        assert!(step(10));
        let resolution = Resolution::Completed(BridgeTx::Evm(true, [22; 32].into()));
        journal::resolve(
            entry.id,
            journal::get(entry.id).unwrap().revision,
            resolution.clone(),
            "verified the outgoing transaction reached finality".into(),
            live.user,
        )
        .unwrap();
        state::apply_payout_resolution(entry.id, live.task_id, &resolution);
        let held = pending::get(live.task_id).unwrap();
        assert!(held.stuck && held.is_finalized());
        assert!(!journal::get(entry.id).unwrap().handled);
        assert!(state::can_close_task(&held, true).is_ok());
        assert!(state::can_retry_task(&held).is_err());
        let mut mismatched = held;
        mismatched.to_tx = Some(BridgeTx::Evm(true, [23; 32].into()));
        assert!(state::can_close_task(&mismatched, true).is_err());
    }

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
