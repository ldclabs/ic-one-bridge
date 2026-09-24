use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_plan_rejects_unencodable_destination_amount_before_any_payment() {
        let mut state = State::new();
        state.ledger_verified = true;
        state.icp_collected_fees_migrated = true;
        state.token_bridge_fee = 0;
        state.min_threshold_to_bridge = 1;
        state.ecdsa_public_key.public_key =
            ic_secp256k1::PublicKey::mainnet_key(ic_secp256k1::MasterPublicKeyId::EcdsaKey1)
                .serialize_sec1(true)
                .into();
        state.ed25519_public_key.public_key =
            ic_ed25519::PublicKey::mainnet_key(ic_ed25519::MasterPublicKeyId::Key1)
                .serialize_raw()
                .to_vec()
                .into();
        state
            .evm_token_contracts
            .insert("ETH".into(), (Address::from([2; 20]), 46, 1));
        let user = Principal::from_slice(&[1]);
        assert!(plan_bridge(&state, "ICP", "ETH", 2_000_000_000, None, user).is_err());
        state.svm_token_address = (
            Pubkey::new_from_array([3; 32]),
            18,
            Pubkey::from_str_const(crate::svm::TOKEN_PROGRAM),
        );
        state.svm_mint_verified = true;
        assert!(plan_bridge(&state, "ICP", "SOL", 2_000_000_000, None, user).is_err());
        state.svm_token_address.1 = 8;
        assert!(plan_bridge(&state, "ICP", "SOL", 2_000_000_000, None, user).is_ok());
    }

    fn task(seed: u64) -> BridgeLog {
        let mut task = crate::store::tests::log(
            Principal::from_slice(&[71]),
            BridgeTarget::Icp,
            BridgeTarget::Evm("ETH".into()),
            BridgeTx::Icp(true, seed),
        );
        task.task_id = pending::next_id();
        task
    }

    /// A task whose payout reverted on chain, waiting for an administrator.
    fn failed_payout(seed: u64) -> BridgeLog {
        let mut task = task(seed);
        task.to_tx = Some(BridgeTx::Evm(false, [9; 32].into()));
        task.payout_resolution = Some(PayoutResolution::Failed);
        task.stuck = true;
        task.error = Some("ETH: outgoing transaction reverted on chain".into());
        pending::insert(&task);
        task
    }

    #[test]
    fn a_recheck_does_not_wait_for_a_running_round() {
        pending::reset();
        STATE.with_borrow_mut(|s| {
            s.icp_collected_fees_migrated = true;
            s.finalize_bridging_round.1 = true;
            s.finalize_bridging_started_at = now_ms();
        });
        let task = failed_payout(8_001);
        recheck_task(&task.from_tx, task.user, false).unwrap();
        let live = pending::get(task.task_id).unwrap();
        assert!(!live.stuck && live.error.is_none());
        assert_eq!(live.next_poll_at, now_ms());
        // The round that is running keeps its lock.
        assert!(STATE.with_borrow(|s| s.finalize_bridging_round.1));
    }

    #[test]
    fn an_admin_retry_wakes_only_its_own_task() {
        pending::reset();
        STATE.with_borrow_mut(|s| s.icp_collected_fees_migrated = true);
        let failed = failed_payout(8_002);
        let mut waiting = task(8_003);
        waiting.next_poll_at = now_ms() + 600_000;
        waiting.poll_attempts = 50;
        pending::insert(&waiting);

        retry_pending_task(&failed.from_tx, None, None).unwrap();
        let retried = pending::get(failed.task_id).unwrap();
        assert!(retried.to_tx.is_none() && !retried.stuck);
        assert_eq!(retried.next_poll_at, now_ms());
        let untouched = pending::get(waiting.task_id).unwrap();
        assert_eq!(
            (untouched.next_poll_at, untouched.poll_attempts),
            (waiting.next_poll_at, 50)
        );
    }

    #[test]
    fn archived_tasks_leave_the_queue_without_their_scheduling_state() {
        pending::reset();
        let mut task = task(8_004);
        task.next_poll_at = 99;
        task.poll_attempts = 3;
        pending::insert(&task);
        let id = archive_task(&task).unwrap();
        assert!(pending::get(task.task_id).is_none());
        assert_eq!(pending::archived_source(&task.from_tx), Some(id));
        let stored = BRIDGE_LOGS.with_borrow(|logs| logs.get(id)).unwrap();
        assert_eq!(
            (stored.task_id, stored.next_poll_at, stored.poll_attempts),
            (task.task_id, 0, 0)
        );
    }
}
mod engine;
mod transactions;
use engine::finalize_bridging;
pub use transactions::{
    build_erc20_transfer_tx, build_evm_transfer_tx, build_sol_transfer_tx, build_spl_transfer_tx,
};

pub static DEFAULT_EXPR_PATH: LazyLock<HttpCertificationPath<'static>> =
    LazyLock::new(|| HttpCertificationPath::wildcard(""));
pub static DEFAULT_CERTIFICATION: LazyLock<HttpCertification> =
    LazyLock::new(HttpCertification::skip);
pub static DEFAULT_CEL_EXPR: LazyLock<String> =
    LazyLock::new(|| create_cel_expr(&DefaultCelBuilder::skip_certification()));
pub static DEFAULT_CERT_ENTRY: LazyLock<HttpCertificationTreeEntry> =
    LazyLock::new(|| HttpCertificationTreeEntry::new(&*DEFAULT_EXPR_PATH, *DEFAULT_CERTIFICATION));

mod initialization;
pub use initialization::try_init_public_keys;

pub fn with<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with_borrow(f)
}

pub fn with_mut<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let result = STATE.with_borrow_mut(f);
    crate::http_config::refresh();
    result
}

pub fn http_tree_with<R>(f: impl FnOnce(&HttpCertificationTree) -> R) -> R {
    HTTP_TREE.with(|r| f(&r.borrow()))
}

pub fn replace_http_tree(tree: HttpCertificationTree) {
    #[cfg(not(test))]
    ic_cdk::api::certified_data_set(tree.root_hash());
    HTTP_TREE.with_borrow_mut(|target| *target = tree);
}

pub fn init_http_certified_data() {
    crate::http_config::refresh();
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
    budget::save();
}

pub fn info() -> StateInfo {
    let total_bridge_count = BRIDGE_LOGS.with_borrow(|r| r.len());
    STATE.with_borrow(|s| StateInfo::new(s, total_bridge_count))
}

pub fn start_migrations() {
    migration::start();
}

fn derive_evm_address(public_key: &PublicKeyOutput, user: &Principal) -> Result<Address, String> {
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

fn derive_svm_address(public_key: &PublicKeyOutput, user: &Principal) -> Result<Pubkey, String> {
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
            icp: {
                let mut blocked = vec![
                    Principal::anonymous(),
                    Principal::management_canister(),
                    s.icp_address,
                    s.token_ledger,
                ];
                if let Some(minting) = &s.ledger_minting_account
                    && minting.subaccount.is_none_or(|s| s == [0; 32])
                {
                    blocked.push(minting.owner);
                }
                blocked
            },
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
            if s.svm_token_address.0 == Pubkey::default() || !s.svm_mint_verified {
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

fn checked_chain_amount(s: &State, target: &BridgeTarget, amount: u128) -> Result<u128, String> {
    let amount = convert_amount(
        amount,
        s.token_decimals,
        chain_decimals(s, target).unwrap_or(s.token_decimals),
    )?;
    if amount == 0 {
        return Err(format!("{}: amount rounds to zero", target.name()));
    }
    if *target == BridgeTarget::Sol && u64::try_from(amount).is_err() {
        return Err("SOL: amount exceeds the transaction's u64 limit".into());
    }
    Ok(amount)
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
    // Unit tests run natively, without the canister's timer API.
    if cfg!(test) {
        return;
    }
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
    if !s.ledger_verified {
        return Err("ICP: ledger metadata has not been verified; retry initialization".into());
    }
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
    checked_chain_amount(s, &from, icp_amount)?;
    checked_chain_amount(s, &to, payout_amount)?;
    let to_addr = validate_destination(s, &to, to_addr)?;

    for chain in [&from, &to] {
        if let Some(error) = pending::chain_error(chain) {
            return Err(format!("{}: waiting for recovery: {error}", chain.name()));
        }
    }
    if matches!(from, BridgeTarget::Evm(_)) && pending::unconfirmed_deposit(user, &from) {
        return Err("this user already has an unconfirmed deposit on the source chain".into());
    }
    if !s.icp_collected_fees_migrated {
        return Err("financial history migration is in progress".into());
    }
    if pending::len().saturating_add(journal::open_count())
        >= u64::from(s.resource_limits.max_pending)
        || pending::user_count(user, 1001) >= s.resource_limits.max_pending_per_user as usize
    {
        return Err("pending task capacity reached".into());
    }

    Ok(BridgePlan {
        from,
        to,
        to_addr,
        token_ledger: s.token_ledger,
        fee: s.token_bridge_fee,
    })
}

pub(crate) fn evm_raw_hex(meta: &TxMeta) -> Result<String, String> {
    meta.raw
        .as_ref()
        .map(|raw| Bytes::copy_from_slice(raw).to_string())
        .ok_or_else(|| "no signed transaction to broadcast".to_string())
}

pub(crate) fn svm_raw(meta: &TxMeta) -> Result<ByteBufB64, String> {
    meta.raw
        .as_ref()
        .map(|raw| ByteBufB64::from(raw.to_vec()))
        .ok_or_else(|| "no signed transaction to broadcast".to_string())
}

/// Hands a signed transaction to `target`'s providers. Broadcasting the same
/// bytes again is harmless, so every first send and resend goes through here.
pub(crate) async fn broadcast_raw(target: &BridgeTarget, meta: &TxMeta) -> Result<(), String> {
    match target {
        BridgeTarget::Evm(chain) => {
            let raw = evm_raw_hex(meta)?;
            evm_client(chain)?
                .send_raw_transaction(raw)
                .await
                .map(|_| ())
        }
        BridgeTarget::Sol => svm_client()
            .send_transaction(svm_raw(meta)?)
            .await
            .map(|_| ()),
        BridgeTarget::Icp => Err("ICP transfers are not broadcast".into()),
    }
}

pub async fn bridge(
    from_chain: String,
    to_chain: String,
    amount: u128,
    to_addr: Option<String>,
    user: Principal,
    now: u64,
) -> Result<BridgeTx, String> {
    bridge_with_id(from_chain, to_chain, amount, to_addr, user, now, None).await
}

pub async fn bridge_with_id(
    from_chain: String,
    to_chain: String,
    amount: u128,
    to_addr: Option<String>,
    user: Principal,
    now: u64,
    request_id: Option<ByteBuf>,
) -> Result<BridgeTx, String> {
    let _active = acquire_active_bridge_user(user)?;
    let entry = deposit_entry(
        user,
        &from_chain,
        &to_chain,
        amount,
        to_addr.as_deref(),
        request_id.as_deref().map(Vec::as_slice),
        now,
    )?;
    resume_deposit_entry(entry).await
}

/// Whether a recorded deposit plan is the one these bridge arguments ask
/// for. Only the caller's own arguments count, so a retry never depends on
/// today's fee or admission state.
fn deposit_matches(
    plan: &journal::DepositPlan,
    from_chain: &str,
    to_chain: &str,
    amount: u128,
    to_addr: Option<&str>,
) -> Result<bool, String> {
    Ok(plan.from.name() == from_chain
        && plan.to.name() == to_chain
        && plan.amount == amount
        && check_destination(&plan.to, to_addr, &ForbiddenDestinations::default())? == plan.to_addr)
}

/// The deposit intent a bridge call carries on: the one its request ID
/// names, the user's unresolved one when the arguments match it, or a new
/// one that passed every check.
pub(super) fn deposit_entry(
    user: Principal,
    from_chain: &str,
    to_chain: &str,
    amount: u128,
    to_addr: Option<&str>,
    request_id: Option<&[u8]>,
    now: u64,
) -> Result<journal::Entry, String> {
    if from_chain == to_chain {
        return Err("from_chain and to_chain cannot be the same".into());
    }
    if let Some(id) = request_id
        && let Some(entry) = journal::find_request(user, id)?
    {
        let journal::Purpose::Deposit(plan) = &entry.purpose else {
            return Err("request ID is not a deposit".into());
        };
        if !deposit_matches(plan, from_chain, to_chain, amount, to_addr)? {
            return Err("request ID belongs to different bridge arguments".into());
        }
        return Ok(entry);
    }
    // The legacy bridge API recovers its outstanding deposit without a
    // request ID; one given now is bound to it for later retries.
    if let Some(entry) = journal::open_deposit(user) {
        let journal::Purpose::Deposit(plan) = &entry.purpose else {
            unreachable!("open_deposit returns deposits")
        };
        if !deposit_matches(plan, from_chain, to_chain, amount, to_addr)? {
            return Err(format!("resume deposit operation {} first", entry.id));
        }
        if let Some(request_id) = request_id {
            journal::bind_request(user, request_id, entry.id)?;
        }
        return Ok(entry);
    }
    let plan =
        STATE.with_borrow(|s| plan_bridge(s, from_chain, to_chain, amount, to_addr, user))?;
    journal::for_deposit(
        journal::DepositPlan {
            user,
            from: plan.from,
            to: plan.to,
            to_addr: plan.to_addr,
            ledger: plan.token_ledger,
            amount,
            fee: plan.fee,
        },
        request_id,
        now,
    )
}

async fn resume_deposit_entry(entry: journal::Entry) -> Result<BridgeTx, String> {
    let id = entry.id;
    let result = resume_deposit_entry_inner(entry).await;
    if let Err(error) = &result
        && let Some(entry) = journal::get(id)
    {
        if matches!(entry.phase, journal::Phase::Planning) {
            journal::failed(id, error.clone(), false, false);
        } else if matches!(
            (entry.phase, entry.request),
            (
                journal::Phase::Submitted,
                Some(journal::Request::Signature { .. })
            )
        ) {
            journal::failed(id, error.clone(), true, false);
        }
    }
    result.map_err(|error| format!("operation {id}: {error}"))
}

async fn resume_deposit_entry_inner(entry: journal::Entry) -> Result<BridgeTx, String> {
    let journal::Purpose::Deposit(plan) = entry.purpose.clone() else {
        return Err("not a deposit operation".into());
    };
    if entry.handled {
        return match entry.phase {
            journal::Phase::Completed(tx) => Ok(tx),
            journal::Phase::Signed => entry
                .signed
                .map(|(tx, _)| tx)
                .ok_or_else(|| "missing signed deposit".into()),
            _ => Err("deposit has already been handled".into()),
        };
    }
    // The signed transfer is recorded on its task before it is broadcast, so
    // a broadcast whose outcome is unknown cannot strand the user's funds. A
    // ledger transfer has nothing left to broadcast.
    let (from_tx, from_meta) = match (entry.phase.clone(), &plan.from, entry.signed) {
        (journal::Phase::Completed(tx), _, _) => (tx, None),
        (_, BridgeTarget::Icp, _) => {
            if entry.request.is_none() {
                journal::prepare(
                    entry.id,
                    journal::Request::TransferFrom {
                        ledger: plan.ledger,
                        args: TransferFromArgs {
                            spender_subaccount: None,
                            from: Account {
                                owner: plan.user,
                                subaccount: None,
                            },
                            to: Account {
                                owner: crate::helper::canister_id(),
                                subaccount: None,
                            },
                            fee: None,
                            created_at_time: Some(entry.created_at.saturating_mul(1_000_000)),
                            memo: Some(journal::memo(entry.id)),
                            amount: plan.amount.into(),
                        },
                    },
                )?;
            }
            schedule_finalize(Duration::from_secs(5));
            (journal::execute(entry.id).await?, None)
        }
        (_, _, Some((tx, meta))) => (tx, Some(meta)),
        (_, BridgeTarget::Evm(chain), None) => {
            let bridge = STATE.with_borrow(|s| s.evm_address);
            let signed = build_erc20_transfer_tx(
                chain,
                &plan.user,
                &bridge,
                plan.amount,
                now_ms(),
                Funding::Deposit(entry.id),
            )
            .await?;
            (signed.tx, Some(signed.meta))
        }
        (_, BridgeTarget::Sol, None) => {
            let bridge = STATE.with_borrow(|s| s.svm_address);
            let signed =
                build_spl_transfer_tx(&plan.user, &bridge, plan.amount, Funding::Deposit(entry.id))
                    .await?;
            (signed.tx, Some(signed.meta))
        }
    };
    if pending::by_tx(&from_tx).is_none() {
        pending::insert(&BridgeLog {
            runtime: Some(LogRuntime {
                task_id: entry.id,
                ledger: Some(plan.ledger),
                next_poll_at: now_ms(),
                ..Default::default()
            }),
            id: None,
            user: plan.user,
            from: plan.from.clone(),
            to: plan.to,
            icp_amount: plan.amount,
            fee: plan.fee,
            from_tx: from_tx.clone(),
            to_tx: None,
            to_addr: plan.to_addr,
            created_at: entry.created_at,
            finalized_at: 0,
            error: None,
            stuck: false,
            payout_started_at: 0,
            from_meta: from_meta.clone(),
            to_meta: None,
        });
    }
    journal::handled(entry.id);
    schedule_finalize(Duration::from_secs(if from_meta.is_none() { 0 } else { 5 }));
    if let Some(meta) = &from_meta
        && let Err(error) = broadcast_raw(&plan.from, meta).await
        && let Some(task) = pending::by_tx(&from_tx)
    {
        pending::update(task.task_id, |t| {
            t.error = Some(crate::outcall::public_error(
                &format!("{}: broadcast failed: {error}", plan.from.name()),
                &[],
            ));
            t.error_chain = Some(plan.from);
        });
    }
    Ok(from_tx)
}

/// Finds the log recording `from_tx`, whether it is still being worked on or
/// already archived.
pub fn my_bridge_log(user: Principal, from_tx: BridgeTx) -> Option<BridgeLog> {
    if let Some(log) = pending::by_tx(&from_tx).filter(|t| t.user == user) {
        return Some(log);
    }

    let recent =
        USER_LOG_INDEX.with_borrow(|index| user_log_ids(index, &user, u64::MAX, MAX_LOG_LOOKBACK));
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

/// Moves a finished task out of the queue into the permanent store, indexes
/// it under its user and returns the id it was stored under.
fn archive_task(task: &BridgeLog) -> Result<u64, String> {
    let mut log = BridgeLogLocal::from(task.clone());
    // Scheduling state means nothing once a task is done; left at its
    // default it is not stored at all.
    log.next_poll_at = 0;
    log.poll_attempts = 0;
    log.payout_mined = false;
    let log_id = BRIDGE_LOGS
        .with_borrow_mut(|r| r.append(&log))
        .map_err(|err| format!("failed to append to BRIDGE_LOGS: {}", format_error(err)))?;
    pending::remove(task.task_id);
    USER_LOG_INDEX.with_borrow_mut(|index| {
        index.insert(UserLogKey::new(&task.user, log_id), ());
    });
    migration::archive_appended(log_id, task);
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
        s.finalize_bridging_round.0
    });

    pending::wake_all();
    schedule_finalize(Duration::from_secs(0));
    round
}

/// Lets the next round act on an administrator's edit right away and lifts
/// the circuit breaker. The edited task is due now; the rest of the queue
/// keeps its own backoff instead of being woken and re-polled all at once.
fn run_after_edit() {
    STATE.with_borrow_mut(|s| s.error_rounds = 0);
    schedule_finalize(Duration::ZERO);
}

/// Releases a round lock that `ensure_finalize_bridging_idle` found
/// abandoned. The generation moves first, so a late callback of that round
/// can neither merge nor broadcast over the edit that goes with this.
fn release_stale_round(stale: bool) {
    if stale {
        next_finalize_run_generation();
        STATE.with_borrow_mut(|s| {
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
        });
    }
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
    pending::by_tx(from_tx).ok_or_else(|| "no pending task matches this transaction".into())
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
    let amount = bridge_amount_after_fee(task.icp_amount, task.fee)?;
    if let Some(decimals) = chain_decimals(s, &target) {
        check_payout_precision(amount, s.token_decimals, decimals)?;
    }
    checked_chain_amount(s, &target, amount)?;
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
pub fn can_retry_task(task: &BridgeLog) -> Result<(), String> {
    ensure_task_reconciled(task)?;
    if task.is_finalized() || task.to_tx.as_ref().is_some_and(BridgeTx::is_finalized) {
        return Err("payout is already finalized".into());
    }
    if task.payout_resolution == Some(PayoutResolution::Incomplete) {
        return Err("partial payout must be reconciled before a new amount is chosen".into());
    }
    if let Some(id) = task.payout_attempt {
        if !journal::safe_to_reset(id) {
            return Err(format!(
                "resolve payout operation {id} before resetting its deduplication key"
            ));
        }
    } else if task.payout_may_execute() && task.payout_resolution != Some(PayoutResolution::Failed)
    {
        return Err("payout may still execute; verify and reconcile it before retrying".into());
    }
    Ok(())
}

pub fn retry_pending_task(
    from_tx: &BridgeTx,
    to: Option<BridgeTarget>,
    to_addr: Option<String>,
) -> Result<BridgeLog, String> {
    let stale = ensure_finalize_bridging_idle()?;
    let mut task = pending_task(from_tx)?;
    can_retry_task(&task)?;
    let redirect =
        STATE.with_borrow(|s| plan_retry_redirect(s, &task, to.as_ref(), to_addr.as_deref()))?;
    if let Some((to, addr)) = redirect {
        task.to = to;
        task.to_addr = addr;
    }
    if let Some(id) = task.payout_attempt {
        journal::failed(
            id,
            "administrator reset a provably unexecuted attempt".into(),
            false,
            false,
        );
        journal::handled(id);
    }
    task.to_tx = None;
    task.to_meta = None;
    task.payout_attempt = None;
    task.payout_resolution = None;
    task.payout_mined = false;
    task.error = None;
    task.error_chain = None;
    task.stuck = false;
    task.payout_started_at = 0;
    task.poll_attempts = 0;
    task.next_poll_at = now_ms();
    pending::insert(&task);
    release_stale_round(stale);
    run_after_edit();
    Ok(task)
}

/// Clears a task's error and has the next round look at it again.
///
/// Unlike the administrative edits this does not wait for a running round:
/// it touches neither the payout nor its reservation, and a round that is
/// working on the task writes its own fresher result over these fields.
pub fn recheck_task(from_tx: &BridgeTx, owner: Principal, controller: bool) -> Result<(), String> {
    let task = pending_task(from_tx)?;
    if !controller && task.user != owner {
        return Err("task belongs to another user".into());
    }
    ensure_task_reconciled(&task)?;
    pending::update(task.task_id, |task| {
        task.stuck = false;
        task.error = None;
        task.error_chain = None;
        task.next_poll_at = now_ms();
    });
    release_stale_round(ensure_finalize_bridging_idle().unwrap_or(false));
    schedule_finalize(Duration::ZERO);
    Ok(())
}

pub fn can_close_task(task: &BridgeLog, force: bool) -> Result<(), String> {
    let held = pending::reconciliation_error(task).is_some();
    if task.is_finalized() && !held {
        return Err("task is already finalized".into());
    }
    if !force && (!task.stuck || !task.from_tx.is_finalized() || task.payout_may_execute()) {
        return Err("closing would discard an unresolved deposit or payout; reconcile it first, or explicitly force an externally settled closure".into());
    }
    if let Some(id) = task.payout_attempt
        && !journal::get(id).is_some_and(|entry| {
            entry.handled
                || (held
                    && matches!(entry.phase, journal::Phase::Completed(ref tx)
                        if tx.is_finalized() && task.to_tx.as_ref() == Some(tx)))
        })
    {
        return Err(format!(
            "resolve payout operation {id} before closing its task"
        ));
    }
    Ok(())
}

pub fn close_pending_task(from_tx: &BridgeTx, now: u64, force: bool) -> Result<BridgeLog, String> {
    let stale = ensure_finalize_bridging_idle()?;
    let mut task = pending_task(from_tx)?;
    can_close_task(&task, force)?;
    task.finalized_at = now;
    if task.error.is_none() {
        task.error = Some("closed by administrator after external settlement".into());
    }
    let id = archive_task(&task)?;
    if let Some(operation) = task.payout_attempt {
        // can_close_task requires a handled attempt or the exact finalized
        // payout of a quarantined duplicate. Closing never sends another one.
        journal::handled(operation);
    }
    task.id = Some(id);
    release_stale_round(stale);
    run_after_edit();
    Ok(task)
}

pub fn can_change_ledger() -> bool {
    pending::is_empty()
        && !journal::unresolved()
        && BRIDGE_LOGS.with_borrow(|logs| logs.is_empty())
        && STATE.with_borrow(|s| s.legacy_pending.is_empty() && s.total_withdrawn_fees == 0)
}

pub fn validate_resolution(
    id: u64,
    revision: u64,
    resolution: &Resolution,
    evidence: &str,
) -> Result<(), String> {
    journal::check_resolution(
        &journal::get(id).ok_or_else(|| "operation not found".to_string())?,
        revision,
        resolution,
        evidence,
    )
}

pub(super) fn apply_payout_resolution(id: u64, task_id: u64, resolution: &Resolution) {
    let task_exists = pending::update(task_id, |task| {
        match resolution {
            Resolution::Completed(tx) => {
                task.to_tx = Some(tx.clone());
                task.payout_resolution = Some(PayoutResolution::Completed);
                task.stuck = false;
                task.error = None;
                task.error_chain = None;
            }
            Resolution::NotExecuted => {
                task.payout_resolution = Some(PayoutResolution::Failed);
                task.stuck = true;
                task.error =
                    Some("payout reconciled as unexecuted; retry or redirect the task".into());
            }
        }
        task.next_poll_at = now_ms();
    })
    .is_some();
    if matches!(resolution, Resolution::NotExecuted) || !task_exists {
        journal::handled(id);
    }
}

pub fn resolve_operation(
    id: u64,
    revision: u64,
    resolution: Resolution,
    evidence: String,
    controller: Principal,
) -> Result<(), String> {
    let stale = ensure_finalize_bridging_idle()?;
    if matches!(resolution, Resolution::NotExecuted)
        && journal::get(id)
            .is_some_and(|entry| matches!(entry.purpose, journal::Purpose::Deposit(_)))
        && pending::get(id).is_some_and(|task| {
            task.payout_may_execute() || task.to_tx.as_ref().is_some_and(BridgeTx::is_finalized)
        })
    {
        return Err("reconcile the outgoing payment before closing its deposit".into());
    }
    let entry = journal::resolve(id, revision, resolution.clone(), evidence, controller)?;
    match entry.purpose {
        journal::Purpose::Payout(task_id) => {
            apply_payout_resolution(id, task_id, &resolution);
        }
        journal::Purpose::Deposit(_) => {
            if matches!(resolution, Resolution::NotExecuted) {
                if let Some(mut task) = pending::get(id) {
                    task.error = Some("deposit externally reconciled as not executed".into());
                    task.stuck = false;
                    task.finalized_at = now_ms();
                    task.from_meta = None;
                    archive_task(&task)?;
                }
                journal::handled(id);
            } else if let Some(task) = pending::get(id) {
                pending::update(task.task_id, |task| {
                    if let Resolution::Completed(tx) = &resolution {
                        task.from_tx = tx.clone();
                        task.from_meta = None;
                    }
                    task.stuck = false;
                    task.error = None;
                    task.error_chain = None;
                    task.next_poll_at = now_ms();
                });
            }
            // A completed deposit without a pending task is resumed with its
            // immutable plan by the owner (resume_deposit) or the next round.
        }
        _ => journal::handled(id),
    }
    release_stale_round(stale);
    run_after_edit();
    Ok(())
}

fn legacy_resolution_candidate(
    from_tx: &BridgeTx,
    task_id: u64,
    resolution: &Resolution,
    evidence: &str,
) -> Result<journal::Entry, String> {
    let task = pending_task(from_tx)?;
    if task.task_id != task_id || task.payout_attempt.is_some() || !task.payout_may_execute() {
        return Err("legacy task changed or already has a journal operation".into());
    }
    let mut entry = journal::draft(task.user, journal::Purpose::Payout(task_id), now_ms());
    entry.request = Some(journal::Request::LegacyPayout(Box::new(task.clone())));
    entry.signed = task.to_tx.clone().zip(task.to_meta.clone());
    entry.phase = journal::Phase::NeedsReview("legacy payment has no complete journal".into());
    journal::check_resolution(&entry, entry.revision, resolution, evidence)?;
    Ok(entry)
}

pub fn validate_legacy_resolution(
    from_tx: &BridgeTx,
    task_id: u64,
    resolution: &Resolution,
    evidence: &str,
) -> Result<OperationInfo, String> {
    legacy_resolution_candidate(from_tx, task_id, resolution, evidence).map(journal::info)
}

pub fn resolve_legacy_payout(
    from_tx: BridgeTx,
    task_id: u64,
    resolution: Resolution,
    evidence: String,
    controller: Principal,
) -> Result<(), String> {
    let stale = ensure_finalize_bridging_idle()?;
    let mut entry = legacy_resolution_candidate(&from_tx, task_id, &resolution, &evidence)?;
    let created = journal::create(entry.owner, entry.purpose.clone(), entry.created_at);
    entry.id = created.id;
    entry.revision = created.revision;
    journal::put(&mut entry);
    pending::update(task_id, |task| task.payout_attempt = Some(entry.id));
    release_stale_round(stale);
    resolve_operation(entry.id, entry.revision, resolution, evidence, controller)
}

pub async fn resume_operation(id: u64, owner: Principal) -> Result<BridgeTx, String> {
    let _active = acquire_active_bridge_user(owner)?;
    let entry = journal::get(id)
        .filter(|e| e.owner == owner)
        .ok_or_else(|| "operation not found".to_string())?;
    run_operation(entry).await
}

/// Carries an operation as far as it can go by itself: its owner's resume
/// and a finalization round's recovery both come here.
pub(super) async fn run_operation(entry: journal::Entry) -> Result<BridgeTx, String> {
    match entry.purpose {
        journal::Purpose::Deposit(_) => resume_deposit_entry(entry).await,
        journal::Purpose::Withdrawal { .. } => {
            let tx = journal::execute(entry.id).await?;
            journal::handled(entry.id);
            Ok(tx)
        }
        journal::Purpose::Payout(_) => {
            Err("payouts are resumed through their pending tasks".into())
        }
        journal::Purpose::LegacyConflict { .. } => {
            Err("this operation requires controller reconciliation".into())
        }
    }
}

pub fn cancel_operation(id: u64, owner: Principal) -> Result<(), String> {
    let entry = journal::get(id)
        .filter(|e| e.owner == owner)
        .ok_or_else(|| "operation not found".to_string())?;
    if matches!(entry.purpose, journal::Purpose::Payout(_)) || !journal::safe_to_reset(id) {
        return Err("an unresolved payment cannot be cancelled".into());
    }
    journal::failed(id, "cancelled before execution".into(), false, false);
    journal::handled(id);
    Ok(())
}

pub fn operation(id: u64) -> Result<OperationInfo, String> {
    journal::get(id)
        .map(journal::info)
        .ok_or_else(|| "operation not found".into())
}

pub fn operations(owner: Principal, take: usize, before: Option<u64>) -> Vec<OperationInfo> {
    journal::page(owner, take, before)
}

/// The ledger's current transfer fee. Only a withdrawal proposal shows it:
/// transfers leave `fee` unset and the ledger charges its own.
async fn ledger_fee(ledger: Principal) -> Result<u128, String> {
    let fee: Nat = crate::helper::read_call(ledger, "icrc1_fee", ()).await?;
    u128::try_from(&fee.0).map_err(|_| "ICP: ledger fee exceeds supported range".into())
}

pub async fn fee_withdrawal_preview(to: Principal, amount: u128) -> Result<(u128, u128), String> {
    if amount == 0 {
        return Err("amount must be positive".into());
    }
    let ledger = STATE.with_borrow(|s| {
        if !s.ledger_verified {
            return Err("ledger verification is incomplete".to_string());
        }
        validate_destination(s, &BridgeTarget::Icp, Some(&to.to_text()))?;
        Ok(s.token_ledger)
    })?;
    let fee = ledger_fee(ledger).await?;
    STATE.with_borrow(|s| {
        let available = journal::available_withdrawal(s);
        if amount > available {
            return Err("withdrawal exceeds earned ICP fees".into());
        }
        Ok((fee, available))
    })
}

fn payout_operation(task: &mut BridgeLog, run_generation: u64) -> Result<Option<u64>, TaskFault> {
    if !finalize_run_is_current(run_generation) {
        return Ok(None);
    }
    let Some(live) = pending::get(task.task_id) else {
        return Ok(None);
    };
    ensure_task_reconciled(&live).map_err(TaskFault::Stuck)?;
    if let Some(id) = live.payout_attempt {
        task.payout_attempt = Some(id);
        return Ok(Some(id));
    }
    let entry = journal::create(task.user, journal::Purpose::Payout(task.task_id), now_ms());
    pending::update(task.task_id, |t| {
        t.payout_attempt = Some(entry.id);
        t.payout_started_at = entry.created_at;
        t.payout_resolution = None;
    });
    task.payout_attempt = Some(entry.id);
    task.payout_started_at = entry.created_at;
    Ok(Some(entry.id))
}

fn payout_fault(task: &BridgeLog, error: String) -> TaskFault {
    if let Some(error) = pending::reconciliation_error(task) {
        return TaskFault::Stuck(error);
    }
    let id = task
        .payout_attempt
        .or_else(|| pending::get(task.task_id).and_then(|t| t.payout_attempt));
    if id
        .and_then(journal::get)
        .is_some_and(|entry| journal::needs_review(&entry))
    {
        TaskFault::Stuck(error)
    } else {
        TaskFault::Transient(error)
    }
}

pub(super) fn ensure_task_reconciled(task: &BridgeLog) -> Result<(), String> {
    if !STATE.with_borrow(|s| s.icp_collected_fees_migrated) {
        return Err("financial history migration is in progress".into());
    }
    pending::reconciliation_error(task).map_or(Ok(()), Err)
}

pub async fn collect_fees(
    owner: Principal,
    to: Principal,
    amount: u128,
) -> Result<BridgeTx, String> {
    if !STATE.with_borrow(|s| s.ledger_verified) {
        return Err("ledger verification is incomplete".into());
    }
    if amount == 0 {
        return Err("amount must be positive".into());
    }
    STATE.with_borrow(|s| validate_destination(s, &BridgeTarget::Icp, Some(&to.to_text())))?;
    let ledger = STATE.with_borrow(|s| s.token_ledger);
    let entry = match journal::open_withdrawal(owner) {
        Some(entry) => {
            if !matches!(&entry.purpose, journal::Purpose::Withdrawal { to: old_to, amount: old_amount, .. } if *old_to == to && *old_amount == amount)
            {
                return Err(format!("resolve withdrawal operation {} first", entry.id));
            }
            entry
        }
        None => journal::create(
            owner,
            journal::Purpose::Withdrawal { to, amount, ledger },
            now_ms(),
        ),
    };
    let journal::Purpose::Withdrawal { ledger, .. } = entry.purpose else {
        unreachable!()
    };
    if entry.request.is_none() {
        journal::prepare(
            entry.id,
            journal::Request::Transfer {
                ledger,
                args: TransferArg {
                    from_subaccount: None,
                    to: Account {
                        owner: to,
                        subaccount: None,
                    },
                    // The ledger charges its current fee, so a fee change can
                    // never reject the persisted request; resending the same
                    // arguments still deduplicates.
                    fee: None,
                    memo: Some(journal::memo(entry.id)),
                    created_at_time: Some(entry.created_at.saturating_mul(1_000_000)),
                    amount: amount.into(),
                },
            },
        )?;
    }
    schedule_finalize(Duration::from_secs(5));
    let tx = journal::execute(entry.id)
        .await
        .map_err(|e| format!("withdrawal operation {}: {e}", entry.id))?;
    journal::handled(entry.id);
    Ok(tx)
}
