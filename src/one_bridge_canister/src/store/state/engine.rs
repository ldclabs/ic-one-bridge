use super::*;

pub(super) async fn finalize_bridging() {
    let started = now_ms();
    let available = STATE.with_borrow(|s| {
        finalize_lock_available(
            s.finalize_bridging_round.1,
            s.finalize_bridging_started_at,
            started,
        )
    });
    if !available {
        return;
    }
    if STATE.with_borrow(|s| !s.ledger_verified) {
        return;
    }
    if STATE.with_borrow(|s| !s.icp_collected_fees_migrated) {
        schedule_finalize(Duration::from_secs(3));
        return;
    }
    let operations = journal::open_ids(1);
    let tasks = pending::select_due(started, ROUND_TASK_LIMIT - operations.len());
    if tasks.is_empty() && operations.is_empty() {
        // A timer may have fired before a rescheduled task became due.
        STATE.with_borrow_mut(|s| {
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
        });
        next_finalize_run_generation();
        if let Some(due) = pending::next_due() {
            schedule_finalize(Duration::from_millis(due.saturating_sub(started).max(1)));
        }
        return;
    }
    STATE.with_borrow_mut(|s| {
        s.finalize_bridging_round.1 = true;
        s.finalize_bridging_started_at = started;
    });
    let run_generation = next_finalize_run_generation();
    let _guard = RoundGuard {
        run_generation,
        task_ids: tasks.iter().map(|t| t.task_id).collect(),
    };
    // An independent watchdog exists before any external call, even with no
    // future ingress. Leases keep stalled tasks behind other ready work.
    schedule_finalize(Duration::from_millis(FINALIZE_BRIDGING_LOCK_TIMEOUT_MS));
    for task in &tasks {
        pending::update(task.task_id, |t| {
            t.next_poll_at = started.saturating_add(FINALIZE_BRIDGING_LOCK_TIMEOUT_MS)
        });
    }
    let context = FinalizeContext::new(&tasks);
    enum Job {
        Task(Box<BridgeLog>),
        Operation(u64),
    }
    enum Done {
        Task(Box<TaskOutcome>, (bool, bool, bool)),
        Operation(Result<BridgeTx, String>),
    }
    use futures::StreamExt;
    let jobs = tasks
        .into_iter()
        .map(|task| Job::Task(Box::new(task)))
        .chain(operations.into_iter().map(Job::Operation));
    let mut work = futures::stream::iter(jobs.map(|job| {
        let context = context.clone();
        async move {
            match job {
                Job::Task(task) => {
                    let baseline = task_progress(&task);
                    Done::Task(
                        Box::new(process_task(*task, now_ms(), context, run_generation).await),
                        baseline,
                    )
                }
                Job::Operation(id) => Done::Operation(recover_operation(id).await),
            }
        }
    }))
    .buffer_unordered(ROUND_TASK_LIMIT);
    let mut has_error = false;
    let mut has_progress = false;
    while let Some(done) = work.next().await {
        if !finalize_run_is_current(run_generation) {
            return;
        }
        match done {
            Done::Operation(result) => {
                has_error |= result.is_err();
                has_progress |= result.is_ok();
            }
            Done::Task(outcome, baseline) => {
                let abandon = matches!(outcome.as_ref(), TaskOutcome::Abandoned(_));
                let mut task = (*outcome).into_log();
                let Some(live) = pending::get(task.task_id) else {
                    continue;
                };
                task.payout_attempt = task.payout_attempt.or(live.payout_attempt);
                task.ledger = task.ledger.or(live.ledger);
                if let Some(id) = task.payout_attempt {
                    if task.payout_resolution.is_none()
                        && journal::get(id)
                            .is_some_and(|e| matches!(e.phase, journal::Phase::Rejected(_)))
                    {
                        task.payout_resolution = Some(PayoutResolution::Failed);
                    }
                    match task.payout_resolution {
                        Some(PayoutResolution::Expired) => {
                            journal::failed(
                                id,
                                "transaction is provably expired or replaced".into(),
                                false,
                                false,
                            );
                            journal::handled(id);
                            task.payout_attempt = None;
                            task.payout_started_at = 0;
                            task.payout_resolution = None;
                        }
                        Some(PayoutResolution::Failed) => {
                            journal::failed(
                                id,
                                task.error.clone().unwrap_or_default(),
                                false,
                                false,
                            );
                            journal::handled(id);
                        }
                        Some(PayoutResolution::Incomplete) => {
                            journal::failed(
                                id,
                                task.error.clone().unwrap_or_default(),
                                true,
                                false,
                            );
                        }
                        _ => {}
                    }
                }

                has_error |= task.has_transient_error();
                let progress = baseline != task_progress(&task) || abandon || task.stuck;
                has_progress |= progress;
                if abandon || task.is_finalized() {
                    task.finalized_at = now_ms();
                    task.from_meta = None;
                    task.to_meta = None;
                    if !abandon {
                        task.error = None;
                        task.error_chain = None;
                        task.stuck = false;
                        task.payout_resolution = Some(PayoutResolution::Completed);
                        STATE.with_borrow_mut(|s| {
                            s.total_bridged_tokens =
                                s.total_bridged_tokens.saturating_add(task.icp_amount);
                            s.total_collected_fees =
                                s.total_collected_fees.saturating_add(task.fee);
                            if task.from == BridgeTarget::Icp {
                                s.icp_collected_fees =
                                    s.icp_collected_fees.saturating_add(task.fee);
                            }
                        });
                        if let Some(id) = task.payout_attempt {
                            let _ = journal::completed(
                                id,
                                task.to_tx.clone().expect("finalized payout"),
                            );
                            journal::handled(id);
                        }
                        let _ = journal::completed(task.task_id, task.from_tx.clone());
                    } else {
                        journal::failed(
                            task.task_id,
                            task.error.clone().unwrap_or_default(),
                            false,
                            false,
                        );
                    }
                    journal::handled(task.task_id);
                    archive_bridge_log(&task).expect("archive task");
                    pending::remove(task.task_id);
                } else {
                    task.poll_attempts = if progress {
                        0
                    } else {
                        task.poll_attempts.saturating_add(1)
                    };
                    let delay = if task.has_transient_error() {
                        error_backoff_secs(u64::from(task.poll_attempts).max(1))
                    } else {
                        finalize_poll_delay_secs(u64::from(task.poll_attempts))
                    };
                    task.next_poll_at = now_ms().saturating_add(delay * 1000);
                    pending::insert(&task);
                }
            }
        }
    }
    if !finalize_run_is_current(run_generation) {
        return;
    }
    let unresolved = !journal::open_ids(1).is_empty();
    let operation_delay = STATE.with_borrow_mut(|s| {
        s.finalize_bridging_round = (s.finalize_bridging_round.0.saturating_add(1), false);
        s.finalize_bridging_started_at = 0;
        s.error_rounds = if has_error && !has_progress {
            s.error_rounds.saturating_add(1)
        } else {
            0
        };
        error_backoff_secs(s.error_rounds).max(3)
    });
    let now = now_ms();
    let next = match (pending::next_due(), unresolved) {
        (Some(due), true) => Some(due.min(now.saturating_add(operation_delay * 1000))),
        (Some(due), false) => Some(due),
        (None, true) => Some(now.saturating_add(operation_delay * 1000)),
        (None, false) => None,
    };
    clear_finalize_timer();
    if let Some(due) = next {
        schedule_finalize(Duration::from_millis(due.saturating_sub(now).max(1)));
    }
}

async fn recover_operation(id: u64) -> Result<BridgeTx, String> {
    let entry = journal::get(id).ok_or_else(|| "operation disappeared".to_string())?;
    match entry.purpose {
        journal::Purpose::Deposit(_) => resume_deposit_entry(entry).await,
        journal::Purpose::Withdrawal { .. } => {
            let tx = journal::execute(id).await?;
            journal::handled(id);
            Ok(tx)
        }
        journal::Purpose::Payout(_) => Err("payout is recovered through its task".into()),
        journal::Purpose::LegacyConflict { .. } => {
            Err("legacy conflicts require controller reconciliation".into())
        }
    }
}

async fn process_task(
    mut task: BridgeLog,
    now_ms: u64,
    context: FinalizeContext,
    run_generation: u64,
) -> TaskOutcome {
    let rt = async {
        ensure_task_reconciled(&task)
            .map_err(|error| (task.from.clone(), TaskFault::Stuck(error)))?;
        let from = task.from.clone();
        if !settle_deposit(&mut task, &context, now_ms)
            .await
            .map_err(|e| (from, e))?
        {
            return Ok(());
        }
        if !finalize_run_is_current(run_generation) {
            return Ok(());
        }
        let to = task.to.clone();
        settle_payout(&mut task, &context, now_ms, run_generation)
            .await
            .map_err(|e| (to, e))
    }
    .await;

    match rt {
        Ok(()) => {
            task.error = None;
            task.error_chain = None;
            task.stuck = false;
            TaskOutcome::Retained(task)
        }
        Err((chain, TaskFault::Transient(err))) => {
            ic_cdk::api::debug_print(format!("finalize_tasks failed: {err}"));
            task.error = Some(crate::outcall::public_error(&err, &[]));
            task.error_chain = Some(chain);
            task.stuck = false;
            TaskOutcome::Retained(task)
        }
        Err((chain, TaskFault::Stuck(err))) => {
            ic_cdk::api::debug_print(format!("bridging task is stuck: {err}"));
            task.error = Some(crate::outcall::public_error(&err, &[]));
            task.error_chain = Some(chain);
            task.stuck = true;
            TaskOutcome::Retained(task)
        }
        Err((chain, TaskFault::Abandon(err))) => {
            if task.payout_may_execute() {
                task.error = Some(format!(
                    "{}: deposit failed but a payout may still execute; reconcile both chains",
                    chain.name()
                ));
                task.error_chain = Some(chain);
                task.stuck = true;
                return TaskOutcome::Retained(task);
            }

            task.error = Some(crate::outcall::public_error(&err, &[]));
            task.error_chain = Some(chain);
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
            .map_err(|err| err.into_fault(&chain))?;
            match status {
                TxStatus::Confirmed(receipt) => {
                    verify_evm_deposit(&chain, task, &receipt)?;
                    task.from_tx = BridgeTx::Evm(true, hash);
                    task.from_meta = None;
                    Ok(true)
                }
                TxStatus::Pending { seen } => {
                    if !seen {
                        rebroadcast_evm(&chain, task.from_meta.as_ref(), task.created_at, now_ms)
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
            let status = check_sol_tx(
                context,
                &signature,
                task.from_meta.as_ref(),
                task.created_at,
                now_ms,
            )
            .await
            .map_err(|err| err.into_fault("SOL"))?;
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

fn verify_evm_payout(chain: &str, task: &BridgeLog, receipt: &EvmReceipt) -> Result<(), TaskFault> {
    let (token, expected, bridge, recipient) = STATE
        .with_borrow(|s| {
            let (token, decimals, _) = s
                .evm_token_contracts
                .get(chain)
                .ok_or_else(|| "unknown chain".to_string())?;
            let recipient = match &task.to_addr {
                Some(addr) => parse_evm_address(addr)?,
                None => derive_evm_address(&s.ecdsa_public_key, &task.user)?,
            };
            let amount = bridge_amount_after_fee(task.icp_amount, task.fee)?;
            Ok::<_, String>((
                *token,
                U256::from(convert_amount(amount, s.token_decimals, *decimals)?),
                s.evm_address,
                recipient,
            ))
        })
        .map_err(|e| TaskFault::Stuck(format!("{chain}: {e}")))?;
    let delivered = receipt.transferred(&token, &bridge, &recipient);
    if delivered != expected {
        return Err(TaskFault::Stuck(format!(
            "{chain}: payout delivered {delivered} units, expected {expected}; reconcile the remaining debt before retrying"
        )));
    }
    Ok(())
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
            let to = match &task.to_addr {
                Some(addr) => {
                    Principal::from_text(addr).map_err(|e| TaskFault::Stuck(e.to_string()))?
                }
                None => task.user,
            };
            let amount =
                bridge_amount_after_fee(task.icp_amount, task.fee).map_err(TaskFault::Stuck)?;
            if task.payout_started_at > 0 && task.payout_attempt.is_none() {
                return Err(TaskFault::Stuck("ICP: legacy payout outcome needs ledger reconciliation before a fresh dedup key can be created".into()));
            }
            let Some(id) = payout_operation(task, run_generation)? else {
                return Ok(());
            };
            let entry = journal::get(id)
                .ok_or_else(|| TaskFault::Stuck("missing payout operation".into()))?;
            if entry.request.is_none() {
                let ledger = task
                    .ledger
                    .unwrap_or_else(|| STATE.with_borrow(|s| s.token_ledger));
                let fee = context.ledger_fee(ledger).await?;
                if !finalize_run_is_current(run_generation) {
                    return Ok(());
                }
                journal::prepare(
                    id,
                    journal::Request::Transfer {
                        ledger,
                        args: TransferArg {
                            from_subaccount: None,
                            to: Account {
                                owner: to,
                                subaccount: None,
                            },
                            fee: Some(fee.into()),
                            created_at_time: Some(entry.created_at.saturating_mul(1_000_000)),
                            memo: Some(journal::memo(id)),
                            amount: amount.into(),
                        },
                    },
                )?;
            }
            match journal::execute(id).await {
                Ok(tx) => {
                    task.to_tx = Some(tx);
                    task.payout_resolution = Some(PayoutResolution::Completed);
                    Ok(())
                }
                Err(error) => Err(payout_fault(task, error)),
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
                    Err(payout_fault(task, err))
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
            .map_err(|err| err.into_fault(&chain))?;
            match status {
                TxStatus::Confirmed(receipt) => {
                    task.payout_resolution = Some(PayoutResolution::Incomplete);
                    verify_evm_payout(&chain, task, &receipt)?;
                    task.payout_resolution = Some(PayoutResolution::Completed);
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
                TxStatus::Failed(reason) => {
                    task.payout_resolution = Some(PayoutResolution::Failed);
                    Err(TaskFault::Stuck(format!(
                        "{chain}: outgoing transaction {tx_hash} {reason}; retry or close this task"
                    )))
                }
                // It can never land, so a fresh one cannot pay twice.
                TxStatus::Dead(reason) => {
                    ic_cdk::api::debug_print(format!(
                        "{chain}: outgoing transaction {tx_hash} {reason}; it is rebuilt next round"
                    ));
                    task.payout_resolution = Some(PayoutResolution::Expired);
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
                    Err(payout_fault(task, err))
                }
            }
        }
        (BridgeTarget::Sol, Some(BridgeTx::Sol(false, signature))) => {
            let since = payout_since(task);
            let status = check_sol_tx(context, &signature, task.to_meta.as_ref(), since, now_ms)
                .await
                .map_err(|err| err.into_fault("SOL"))?;
            match status {
                TxStatus::Confirmed(()) => {
                    task.payout_resolution = Some(PayoutResolution::Completed);
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
                TxStatus::Failed(reason) => {
                    task.payout_resolution = Some(PayoutResolution::Failed);
                    Err(TaskFault::Stuck(format!(
                        "SOL: outgoing transaction {reason}; retry or close this task"
                    )))
                }
                TxStatus::Dead(reason) => {
                    ic_cdk::api::debug_print(format!(
                        "SOL: outgoing transaction {reason}; it is rebuilt next round"
                    ));
                    task.payout_resolution = Some(PayoutResolution::Expired);
                    task.to_tx = None;
                    task.to_meta = None;
                    Ok(())
                }
            }
        }
        _ => Ok(()),
    }
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
    if let Some((tx, meta)) = pending::by_tx(from_tx)
        .and_then(|t| t.payout_attempt)
        .and_then(journal::get)
        .and_then(|e| e.signed)
    {
        let client = evm_client(chain).map_err(|e| (None, e))?;
        let raw = evm_raw_hex(&meta).map_err(|e| (None, e))?;
        return broadcast_payout(run_generation, from_tx, (tx, meta), chain, now_ms, || {
            client.send_raw_transaction(raw)
        })
        .await;
    }

    let (client, signed_tx) = build_erc20_transfer_tx(
        chain,
        &crate::helper::canister_id(),
        &to_addr,
        icp_amount,
        now_ms,
        Funding::Payout {
            task_id: pending::by_tx(from_tx)
                .ok_or_else(|| (None, "task disappeared".to_string()))?
                .task_id,
            run_generation,
        },
    )
    .await
    .map_err(|err| (None, format!("{chain}: {err}")))?;

    let data = evm_raw_hex(&signed_tx.meta).map_err(|e| (None, e))?;
    let payout = (signed_tx.tx, signed_tx.meta);
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
    if let Some((tx, meta)) = pending::by_tx(from_tx)
        .and_then(|t| t.payout_attempt)
        .and_then(journal::get)
        .and_then(|e| e.signed)
    {
        let client = svm_client();
        let raw = svm_raw(&meta).map_err(|e| (None, e))?;
        return broadcast_payout(run_generation, from_tx, (tx, meta), "SOL", now_ms, || {
            client.send_transaction(raw)
        })
        .await;
    }

    let (client, signed_tx) = build_spl_transfer_tx(
        &crate::helper::canister_id(),
        &to_addr,
        icp_amount,
        Funding::Payout {
            task_id: pending::by_tx(from_tx)
                .ok_or_else(|| (None, "task disappeared".to_string()))?
                .task_id,
            run_generation,
        },
    )
    .await
    .map_err(|err| (None, format!("SOL: {err}")))?;

    let raw = svm_raw(&signed_tx.meta).map_err(|e| (None, e))?;
    let payout = (signed_tx.tx, signed_tx.meta);
    broadcast_payout(run_generation, from_tx, payout, "SOL", now_ms, || {
        client.send_transaction(raw)
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
        PayoutClaim::ReconciliationRequired(error) => return Err((None, error)),
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
) -> Result<TxStatus<EvmReceipt>, TxCheckError> {
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
            if !client.receipt_is_canonical(&receipt).await? {
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
            if now_ms.saturating_sub(since_ms) >= ABSENCE_PROOF_WINDOW_MS {
                return Err(TxCheckError::Reconcile("transaction history is too old for automatic absence proofs; reconcile or recheck it".into()));
            }
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
    context: &FinalizeContext,
    signature: &[u8; 64],
    meta: Option<&TxMeta>,
    since_ms: u64,
    now_ms: u64,
) -> Result<TxStatus<()>, TxCheckError> {
    let client = svm_client();
    let signature = SvmSignature::from(*signature).to_string();
    let status = context.sol_status(&signature, &client).await?;

    match status {
        SolTxStatus::Finalized => Ok(TxStatus::Confirmed(())),
        SolTxStatus::Failed(err) => Ok(TxStatus::Failed(format!("failed on chain: {err}"))),
        SolTxStatus::Landed => Ok(TxStatus::Pending { seen: true }),
        SolTxStatus::Unknown => {
            if now_ms.saturating_sub(since_ms) >= ABSENCE_PROOF_WINDOW_MS {
                return Err(TxCheckError::Reconcile("transaction history is too old for automatic absence proofs; reconcile or recheck it".into()));
            }
            if now_ms.saturating_sub(since_ms) >= UNSEEN_TX_GRACE_MS
                && let Some(validity) = meta.and_then(|m| m.svm_validity.as_ref())
                && client.expired(&signature, validity).await?
            {
                return Ok(TxStatus::Dead(
                    "expired: two finalized views reject its recorded blockhash".to_string(),
                ));
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_successful_receipt_without_the_promised_transfer_does_not_settle_a_payout() {
        let token = Address::from([1; 20]);
        let bridge = Address::from([2; 20]);
        let to = Address::from([3; 20]);
        STATE.with_borrow_mut(|s| {
            s.evm_address = bridge;
            s.evm_token_contracts.insert("ETH".into(), (token, 8, 1));
        });
        let mut task = crate::store::tests::log(
            Principal::from_slice(&[1]),
            BridgeTarget::Icp,
            BridgeTarget::Evm("ETH".into()),
            BridgeTx::Icp(true, 1000),
        );
        task.to_addr = Some(to.to_string());
        let mut value = serde_json::json!({"transactionHash":alloy_primitives::B256::ZERO,"status":"0x1","logs":[]});
        let receipt: EvmReceipt = serde_json::from_value(value.clone()).unwrap();
        assert!(verify_evm_payout("ETH", &task, &receipt).is_err());
        value["logs"] = serde_json::json!([{"address":token,"topics":[crate::evm::TRANSFER_EVENT,
            format!("0x{:0>64}",hex::encode(bridge)),format!("0x{:0>64}",hex::encode(to))],"data":format!("0x{:064x}",99)}]);
        let receipt: EvmReceipt = serde_json::from_value(value.clone()).unwrap();
        assert!(verify_evm_payout("ETH", &task, &receipt).is_ok());
        value["logs"][0]["data"] = serde_json::json!(format!("0x{:064x}", 98));
        let receipt: EvmReceipt = serde_json::from_value(value).unwrap();
        assert!(verify_evm_payout("ETH", &task, &receipt).is_err());
    }
}
