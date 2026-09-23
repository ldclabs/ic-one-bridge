use super::*;
use crate::outcall::tests::{MockHttpOutcall, result};
use ic_stable_structures::VectorMemory;

#[test]
fn production_payout_claim_reuses_the_first_transaction_and_fences_old_runs() {
    pending::reset();
    let mut task = log(
        principal(&[1]),
        BridgeTarget::Icp,
        evm("ETH"),
        BridgeTx::Icp(true, 900),
    );
    task.task_id = pending::next_id();
    pending::insert(&task);
    STATE.with_borrow_mut(|s| {
        s.finalize_bridging_round.1 = true;
        s.icp_collected_fees_migrated = true;
    });
    let generation = next_finalize_run_generation();
    let first = (evm_tx(1), meta(1));
    let second = (evm_tx(2), meta(2));
    assert!(matches!(
        claim_pending_payout(generation, &task.from_tx, &first, 10),
        PayoutClaim::Claimed
    ));
    assert!(
        matches!(claim_pending_payout(generation,&task.from_tx,&second,20),PayoutClaim::Existing((tx,_)) if tx==first.0)
    );
    next_finalize_run_generation();
    assert!(matches!(
        claim_pending_payout(generation, &task.from_tx, &second, 30),
        PayoutClaim::RunSuperseded
    ));
    assert!(pending::get(task.task_id).unwrap().to_tx == Some(first.0));
}

#[test]
fn a_bad_legacy_resolution_does_not_allocate_or_modify_an_operation() {
    pending::reset();
    let user = principal(&[1]);
    let mut task = log(
        user,
        BridgeTarget::Icp,
        evm("ETH"),
        BridgeTx::Icp(true, 901),
    );
    task.task_id = pending::next_id();
    task.to_tx = Some(evm_tx(9));
    task.stuck = true;
    pending::insert(&task);
    let watermark = pending::id_high_water();
    assert!(
        state::resolve_legacy_payout(
            task.from_tx.clone(),
            task.task_id,
            Resolution::Completed(BridgeTx::Sol(true, [1; 64].into())),
            "operator checked the wrong chain".into(),
            user
        )
        .is_err()
    );
    assert_eq!(pending::id_high_water(), watermark);
    assert!(pending::get(task.task_id).unwrap().payout_attempt.is_none());
}

#[test]
fn a_task_cannot_be_closed_while_its_payout_operation_is_unresolved() {
    pending::reset();
    STATE.with_borrow_mut(|s| {
        s.finalize_bridging_round.1 = false;
        s.finalize_bridging_started_at = 0;
    });
    let user = principal(&[31, 32, 33]);
    let mut task = log(
        user,
        BridgeTarget::Icp,
        evm("ETH"),
        BridgeTx::Icp(true, 90_001),
    );
    task.task_id = pending::next_id();
    let payout = journal::create(user, journal::Purpose::Payout(task.task_id), now_ms());
    task.payout_attempt = Some(payout.id);
    task.stuck = true;
    pending::insert(&task);

    assert!(state::can_close_task(&task, true).is_err());
    journal::failed(payout.id, "known not to have executed".into(), false, false);
    journal::handled(payout.id);
    assert!(state::can_close_task(&task, true).is_ok());
}

#[test]
fn completing_an_orphaned_payout_operation_closes_its_journal_entry() {
    STATE.with_borrow_mut(|s| {
        s.finalize_bridging_round.1 = false;
        s.finalize_bridging_started_at = 0;
    });
    let user = principal(&[34, 35, 36]);
    let mut task = log(
        user,
        BridgeTarget::Icp,
        evm("ETH"),
        BridgeTx::Icp(true, 90_002),
    );
    task.task_id = pending::next_id();
    task.to_tx = Some(BridgeTx::Evm(false, [77; 32].into()));
    let entry = journal::create(user, journal::Purpose::Payout(task.task_id), now_ms());
    journal::prepare(entry.id, journal::Request::LegacyPayout(Box::new(task))).unwrap();
    let revision = journal::get(entry.id).unwrap().revision;
    let resolution = Resolution::Completed(BridgeTx::Evm(true, [77; 32].into()));
    let resolved = journal::resolve(
        entry.id,
        revision,
        resolution.clone(),
        "verified finalized payout after the task was externally closed".into(),
        user,
    )
    .unwrap();
    let journal::Purpose::Payout(task_id) = resolved.purpose else {
        unreachable!()
    };
    state::apply_payout_resolution(entry.id, task_id, &resolution);
    assert!(journal::get(entry.id).unwrap().handled);
}

#[test]
fn legacy_pending_fee_defaults_to_zero() {
    #[derive(Serialize)]
    struct BeforeFee {
        id: Option<u64>,
        user: Principal,
        from: BridgeTarget,
        to: BridgeTarget,
        icp_amount: u128,
        from_tx: BridgeTx,
        to_tx: Option<BridgeTx>,
        to_addr: Option<String>,
        created_at: u64,
        finalized_at: u64,
        error: Option<String>,
    }
    let old = BeforeFee {
        id: None,
        user: principal(&[1]),
        from: BridgeTarget::Icp,
        to: evm("ETH"),
        icp_amount: 100,
        from_tx: BridgeTx::Icp(true, 902),
        to_tx: None,
        to_addr: None,
        created_at: 1,
        finalized_at: 0,
        error: None,
    };
    let task: BridgeLog = cbor_from_slice(&cbor_into_vec(&old).unwrap()).unwrap();
    assert_eq!(task.fee, 0);
}

fn principal(bytes: &[u8]) -> Principal {
    Principal::from_slice(bytes)
}

pub(super) fn log(
    user: Principal,
    from: BridgeTarget,
    to: BridgeTarget,
    from_tx: BridgeTx,
) -> BridgeLog {
    BridgeLog {
        runtime: Some(LogRuntime {
            task_id: 0,
            ledger: None,
            payout_attempt: None,
            payout_resolution: None,
            next_poll_at: 0,
            poll_attempts: 0,
            error_chain: None,
        }),
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
        svm_validity: None,
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
fn ledger_fee_reads_are_shared_only_within_one_round_and_ledger() {
    let ledger = principal(&[81]);
    for answer in [Ok(10), Err("ledger temporarily unavailable".to_string())] {
        let context = FinalizeContext::default();
        let calls = Cell::new(0);
        let read = || async {
            calls.set(calls.get() + 1);
            answer.clone()
        };
        let results = futures::executor::block_on(async {
            futures::join!(
                context.ledger_fee_with(ledger, read()),
                context.ledger_fee_with(ledger, read()),
                context.ledger_fee_with(ledger, read())
            )
        });
        assert_eq!(results, (answer.clone(), answer.clone(), answer.clone()));
        assert_eq!(calls.get(), 1);
        let other = principal(&[82]);
        assert_eq!(
            futures::executor::block_on(context.ledger_fee_with(other, read())),
            answer
        );
        assert_eq!(calls.get(), 2);
        assert_eq!(
            futures::executor::block_on(FinalizeContext::default().ledger_fee_with(ledger, read())),
            answer
        );
        assert_eq!(calls.get(), 3);
    }
}

#[test]
fn solana_tasks_share_the_lazy_batch() {
    let tasks: Vec<_> = (1..=3)
        .map(|seed| {
            log(
                principal(&[seed]),
                BridgeTarget::Sol,
                BridgeTarget::Icp,
                BridgeTx::Sol(false, [seed; 64].into()),
            )
        })
        .collect();
    let context = FinalizeContext::new(&tasks);
    let reply = serde_json::json!({"context":{"slot":1},"value":[null,null,null]});
    let mock = MockHttpOutcall::new(vec![result(reply.clone()), result(reply)]);
    let client = SvmClient::new(vec!["https://a".into(), "https://b".into()], mock.clone());
    assert!(mock.urls().is_empty());
    let statuses = futures::executor::block_on(futures::future::join_all(
        context
            .sol_signatures
            .iter()
            .map(|signature| context.sol_status(signature, &client)),
    ));
    assert_eq!(statuses, vec![Ok(SolTxStatus::Unknown); 3]);
    assert_eq!(mock.urls().len(), 2);
}

#[test]
fn superseded_finalize_run_cannot_merge() {
    assert!(finalize_run_matches(2, 2, true));
    assert!(!finalize_run_matches(1, 2, true));
    assert!(!finalize_run_matches(2, 2, false));
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
    assert!(check_destination(&evm("BNB"), Some(&ok.replace("AFF", "Aff")), &forbidden).is_err());
    assert!(check_destination(&evm("BNB"), Some(&Address::ZERO.to_string()), &forbidden).is_err());
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
    assert!(check_destination(&BridgeTarget::Sol, Some(&mint.to_string()), &forbidden).is_err());
    assert!(check_destination(&BridgeTarget::Sol, Some("not a pubkey"), &forbidden).is_err());

    let user = principal(&[6; 29]);
    assert_eq!(
        check_destination(&BridgeTarget::Icp, Some(&user.to_text()), &forbidden),
        Ok(Some(user.to_text()))
    );
    assert!(check_destination(&BridgeTarget::Icp, Some("2vxsx-fae"), &forbidden).is_err());
    assert!(check_destination(&BridgeTarget::Icp, Some(&bridge.to_text()), &forbidden).is_err());
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
        check_payout_precision(bridge_amount_after_fee(123_456_700, fee).unwrap(), 8, 6).is_err()
    );
    assert!(
        check_payout_precision(bridge_amount_after_fee(123_456_701, fee).unwrap(), 8, 6).is_ok()
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
