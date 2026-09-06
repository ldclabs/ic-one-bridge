use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_plan_rejects_unencodable_destination_amount_before_any_payment() {
        let mut state = State::new();
        state.ledger_verified = true;
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
    verify_ledger_metadata().await;
    crate::http_config::refresh();
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
    let (mint, providers, verified) = STATE.with_borrow(|s| {
        (
            s.svm_token_address.0,
            s.svm_providers.clone(),
            s.svm_mint_verified,
        )
    });
    if mint != Pubkey::default() && !verified {
        let client = SvmClient::new(providers.clone(), DefaultHttpOutcall);
        match client.get_mint_config(&mint.to_string()).await {
            Ok(config) => STATE.with_borrow_mut(|s| {
                if s.svm_providers == providers
                    && s.svm_token_address.0 == mint
                    && s.svm_token_address.1 == config.decimals
                    && s.svm_token_address.2.to_string() == config.program
                {
                    s.svm_mint_verified = true;
                    s.svm_token_account_size = config.token_account_size;
                }
            }),
            Err(err) => ic_cdk::api::debug_print(format!("SOL mint verification failed: {err}")),
        }
    }
    verify_ledger_metadata().await;
    crate::http_config::refresh();
}

async fn verify_ledger_metadata() {
    let (ledger, decimals, verified) =
        STATE.with_borrow(|s| (s.token_ledger, s.token_decimals, s.ledger_verified));
    if verified {
        return;
    }
    let result = async {
        let actual: u8 = crate::helper::read_call(ledger, "icrc1_decimals", ()).await?;
        let minting: Option<Account> =
            crate::helper::read_call(ledger, "icrc1_minting_account", ()).await?;
        if actual != decimals {
            return Err("configured decimals disagree with the ledger".to_string());
        }
        if minting
            == Some(Account {
                owner: crate::helper::canister_id(),
                subaccount: None,
            })
        {
            return Err("a lock/release bridge must not be the ledger minting account".to_string());
        }
        STATE.with_borrow_mut(|s| {
            if s.token_ledger == ledger && s.token_decimals == decimals {
                s.ledger_verified = true;
                s.ledger_minting_account = minting;
            }
        });
        Ok::<_, String>(())
    }
    .await;
    if let Err(error) = result {
        ic_cdk::api::debug_print(format!("ICP ledger verification failed: {error}"));
    } else {
        schedule_finalize(Duration::ZERO);
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
}

pub fn info() -> StateInfo {
    let total_bridge_count = BRIDGE_LOGS.with_borrow(|r| r.len());
    STATE.with_borrow(|s| StateInfo::new(s, total_bridge_count))
}

pub fn initialize_fee_accounting() {
    STATE.with_borrow_mut(|s| {
        if s.fee_accounting_version == 0 {
            s.withdrawals_baseline = s.total_withdrawn_fees;
            s.spendable_icp_fees = 0;
            s.fee_accounting_version = 1;
        }
    });
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
    if !s.legacy_pending.is_empty() {
        return Err("pending migration is in progress".into());
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
    if from_chain == to_chain {
        return Err("from_chain and to_chain cannot be the same".into());
    }
    let _active = acquire_active_bridge_user(user)?;
    // An explicit request ID is checked against the immutable original plan,
    // so a completed retry never depends on today's fee or admission state.
    if let Some(id) = request_id.as_deref()
        && let Some(entry) = journal::find_request(user, id)?
    {
        let journal::Purpose::Deposit(plan) = &entry.purpose else {
            return Err("request ID is not a deposit".into());
        };
        if plan.from.name() != from_chain
            || plan.to.name() != to_chain
            || plan.amount != amount
            || check_destination(
                &plan.to,
                to_addr.as_deref(),
                &ForbiddenDestinations::default(),
            )? != plan.to_addr
        {
            return Err("request ID belongs to different bridge arguments".into());
        }
        return resume_deposit_entry(entry).await;
    }
    if let Some(entry) = journal::open_deposit(user) {
        let journal::Purpose::Deposit(plan) = &entry.purpose else {
            unreachable!()
        };
        if plan.from.name() == from_chain
            && plan.to.name() == to_chain
            && plan.amount == amount
            && check_destination(
                &plan.to,
                to_addr.as_deref(),
                &ForbiddenDestinations::default(),
            )? == plan.to_addr
        {
            if let Some(request_id) = request_id.as_deref() {
                journal::bind_request(user, request_id, entry.id)?;
            }
            return resume_deposit_entry(entry).await;
        }
        return Err(format!("resume deposit operation {} first", entry.id));
    }
    let plan = STATE.with_borrow(|s| {
        plan_bridge(s, &from_chain, &to_chain, amount, to_addr.as_deref(), user)
    })?;
    let entry = journal::for_deposit(
        journal::DepositPlan {
            user,
            from: plan.from,
            to: plan.to,
            to_addr: plan.to_addr,
            ledger: plan.token_ledger,
            amount,
            fee: plan.fee,
        },
        request_id.as_deref().map(Vec::as_slice),
        now,
    )?;
    resume_deposit_entry(entry).await
}

pub async fn resume_deposit(id: u64, owner: Principal) -> Result<BridgeTx, String> {
    let _active = acquire_active_bridge_user(owner)?;
    let entry = journal::get(id)
        .filter(|e| e.owner == owner)
        .ok_or_else(|| "deposit operation not found".to_string())?;
    resume_deposit_entry(entry).await
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
    let deposit = if let journal::Phase::Completed(tx) = entry.phase.clone() {
        Deposit::Settled(tx)
    } else {
        match &plan.from {
            BridgeTarget::Icp => {
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
                        0,
                    )?;
                }
                schedule_finalize(Duration::from_secs(5));
                Deposit::Settled(journal::execute(entry.id).await?)
            }
            BridgeTarget::Evm(chain) => {
                let client = evm_client(chain)?;
                let (tx, meta) = if let Some(signed) = entry.signed {
                    signed
                } else {
                    let bridge = STATE.with_borrow(|s| s.evm_address);
                    let (_, signed) = build_erc20_transfer_tx(
                        chain,
                        &plan.user,
                        &bridge,
                        plan.amount,
                        now_ms(),
                        Funding::Deposit(entry.id),
                    )
                    .await?;
                    let tx = BridgeTx::Evm(false, <[u8; 32]>::from(*signed.hash()).into());
                    let meta = TxMeta {
                        deadline: TxDeadline::Nonce(signed.tx().nonce),
                        raw: Some(signed.encoded_2718().into()),
                        svm_validity: None,
                    };
                    (tx, meta)
                };
                Deposit::Evm { tx, meta, client }
            }
            BridgeTarget::Sol => {
                let client = svm_client();
                let (tx, meta) = if let Some(signed) = entry.signed {
                    signed
                } else {
                    let bridge = STATE.with_borrow(|s| s.svm_address);
                    let (_, signed, validity) = build_spl_transfer_tx(
                        &plan.user,
                        &bridge,
                        plan.amount,
                        Funding::Deposit(entry.id),
                    )
                    .await?;
                    let tx = BridgeTx::Sol(false, <[u8; 64]>::from(signed.signatures[0]).into());
                    let meta = TxMeta {
                        deadline: TxDeadline::BlockHeight(validity.last_valid_block_height),
                        raw: Some(
                            bincode::serialize(&signed)
                                .map_err(|e| e.to_string())?
                                .into(),
                        ),
                        svm_validity: Some(validity),
                    };
                    (tx, meta)
                };
                Deposit::Sol { tx, meta, client }
            }
        }
    };
    let (from_tx, from_meta) = deposit.record();
    if pending::by_tx(&from_tx).is_none() {
        pending::insert(&BridgeLog {
            runtime: Some(LogRuntime {
                task_id: entry.id,
                ledger: Some(plan.ledger),
                payout_attempt: None,
                payout_resolution: None,
                next_poll_at: now_ms(),
                poll_attempts: 0,
                error_chain: None,
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
            from_meta,
            to_meta: None,
        });
    }
    journal::handled(entry.id);
    STATE.with_borrow_mut(|s| s.idle_rounds = 0);
    schedule_finalize(Duration::from_secs(
        if matches!(deposit, Deposit::Settled(_)) {
            0
        } else {
            5
        },
    ));
    if let Err(error) = deposit.broadcast().await
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

/// Moves a log into the permanent store and indexes it under its user,
/// returning the id it was stored under.
fn archive_bridge_log(log: &BridgeLog) -> Result<u64, String> {
    let log_id = BRIDGE_LOGS
        .with_borrow_mut(|r| r.append(&log.clone().into()))
        .map_err(|err| format!("failed to append to BRIDGE_LOGS: {}", format_error(err)))?;
    USER_LOG_INDEX.with_borrow_mut(|index| {
        index.insert(UserLogKey::new(&log.user, log_id), ());
    });
    migration::archive_appended(log_id, log);
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

    pending::wake_all();
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
    task.error = None;
    task.error_chain = None;
    task.stuck = false;
    task.payout_started_at = 0;
    task.poll_attempts = 0;
    task.next_poll_at = now_ms();
    pending::insert(&task);
    if stale {
        next_finalize_run_generation();
        STATE.with_borrow_mut(|s| {
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
        });
    }
    restart_finalize_bridging();
    Ok(task)
}

pub fn recheck_task(from_tx: &BridgeTx, owner: Principal, controller: bool) -> Result<(), String> {
    let stale = ensure_finalize_bridging_idle()?;
    let task = pending_task(from_tx)?;
    if !controller && task.user != owner {
        return Err("task belongs to another user".into());
    }
    pending::update(task.task_id, |task| {
        task.stuck = false;
        task.error = None;
        task.error_chain = None;
        task.next_poll_at = now_ms();
    });
    if stale {
        next_finalize_run_generation();
        STATE.with_borrow_mut(|s| {
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
        });
    }
    schedule_finalize(Duration::ZERO);
    Ok(())
}

pub fn can_close_task(task: &BridgeLog, force: bool) -> Result<(), String> {
    if task.is_finalized() {
        return Err("task is already finalized".into());
    }
    if !force && (!task.stuck || !task.from_tx.is_finalized() || task.payout_may_execute()) {
        return Err("closing would discard an unresolved deposit or payout; reconcile it first, or explicitly force an externally settled closure".into());
    }
    if let Some(id) = task.payout_attempt
        && !journal::get(id).is_some_and(|entry| entry.handled)
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
    let id = archive_bridge_log(&task)?;
    pending::remove(task.task_id);
    task.id = Some(id);
    if stale {
        next_finalize_run_generation();
        STATE.with_borrow_mut(|s| {
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
        });
    }
    restart_finalize_bridging();
    Ok(task)
}

pub fn can_change_ledger() -> bool {
    pending::is_empty()
        && !journal::unresolved()
        && BRIDGE_LOGS.with_borrow(|logs| logs.is_empty())
        && STATE.with_borrow(|s| {
            s.legacy_pending.is_empty() && s.ledger_fee_credit == 0 && s.total_withdrawn_fees == 0
        })
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
                    archive_bridge_log(&task)?;
                    pending::remove(id);
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
    if stale {
        next_finalize_run_generation();
        STATE.with_borrow_mut(|s| {
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
        });
    }
    restart_finalize_bridging();
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
    entry.id = journal::create(entry.owner, entry.purpose.clone(), entry.created_at).id;
    journal::put(&entry);
    let entry = journal::get(entry.id).expect("legacy reconciliation entry");
    pending::update(task_id, |task| task.payout_attempt = Some(entry.id));
    if stale {
        next_finalize_run_generation();
        STATE.with_borrow_mut(|s| {
            s.finalize_bridging_round.1 = false;
            s.finalize_bridging_started_at = 0;
        });
    }
    resolve_operation(entry.id, entry.revision, resolution, evidence, controller)
}

pub async fn resume_operation(id: u64, owner: Principal) -> Result<BridgeTx, String> {
    let _active = acquire_active_bridge_user(owner)?;
    let entry = journal::get(id)
        .filter(|e| e.owner == owner)
        .ok_or_else(|| "operation not found".to_string())?;
    match entry.purpose {
        journal::Purpose::Deposit(_) => resume_deposit_entry(entry).await,
        journal::Purpose::Withdrawal { .. } | journal::Purpose::FeeFunding { .. } => {
            let tx = journal::execute(id).await?;
            journal::handled(id);
            Ok(tx)
        }
        journal::Purpose::Payout(_) => {
            Err("payouts are resumed through their pending tasks".into())
        }
        journal::Purpose::FeeRecognition { .. } | journal::Purpose::LegacyConflict { .. } => {
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
        let needed = amount
            .checked_add(fee)
            .ok_or_else(|| "withdrawal amount overflow".to_string())?;
        if amount > available || needed > journal::available_operating_funds(s) {
            return Err("withdrawal and its ledger fee exceed available funds".into());
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
    let id = task
        .payout_attempt
        .or_else(|| pending::get(task.task_id).and_then(|t| t.payout_attempt));
    let needs_review = id.and_then(journal::get).is_some_and(|entry| {
        matches!(
            entry.phase,
            journal::Phase::NeedsReview(_) | journal::Phase::Rejected(_)
        ) || matches!(
            (entry.phase, entry.request),
            (
                journal::Phase::Submitted,
                Some(journal::Request::Signature { .. })
            )
        )
    });
    if needs_review {
        TaskFault::Stuck(error)
    } else {
        TaskFault::Transient(error)
    }
}

pub fn validate_legacy_fee_recognition(total: u128, evidence: &str) -> Result<u128, String> {
    if !pending::is_empty() || journal::unresolved() {
        return Err("resolve all pending payments before recognizing historical fees".into());
    }
    if !(20..=2000).contains(&evidence.trim().len()) {
        return Err("provide the external balance and backing reconciliation evidence".into());
    }
    STATE.with_borrow(|s| {
        if !s.icp_collected_fees_migrated {
            return Err("archive migration is incomplete".into());
        }
        let historic_ceiling = s
            .icp_collected_fees
            .saturating_sub(
                s.spendable_icp_fees
                    .saturating_sub(s.legacy_fees_recognized),
            )
            .saturating_sub(s.withdrawals_baseline);
        if total < s.legacy_fees_recognized || total > historic_ceiling {
            return Err(
                "verified total exceeds historical earned fees or decreases an earlier recognition"
                    .into(),
            );
        }
        Ok(total - s.legacy_fees_recognized)
    })
}

pub async fn recognize_legacy_fees(
    total: u128,
    evidence: String,
    controller: Principal,
) -> Result<(), String> {
    let delta = validate_legacy_fee_recognition(total, &evidence)?;
    if delta == 0 {
        return Ok(());
    }
    let watermark = pending::id_high_water();
    let ledger = STATE.with_borrow(|s| s.token_ledger);
    let balance: Nat = crate::helper::read_call(
        ledger,
        "icrc1_balance_of",
        (Account {
            owner: crate::helper::canister_id(),
            subaccount: None,
        },),
    )
    .await?;
    let balance = u128::try_from(&balance.0)
        .map_err(|_| "ledger balance exceeds supported range".to_string())?;
    if pending::id_high_water() != watermark {
        return Err("financial operations changed during reconciliation; review again".into());
    }
    let delta = validate_legacy_fee_recognition(total, &evidence)?;
    if balance < delta {
        return Err("ledger balance is below the proposed fee allocation".into());
    }
    // This capped, cumulative controller attestation asserts the funds are
    // free of backing obligations; a balance alone does not establish that.
    journal::record_fee_recognition(controller, total, evidence);
    STATE.with_borrow_mut(|s| {
        s.spendable_icp_fees = s.spendable_icp_fees.saturating_add(delta);
        s.legacy_fees_recognized = total;
    });
    Ok(())
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
        let fee = ledger_fee(ledger).await?;
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
                    fee: Some(fee.into()),
                    memo: Some(journal::memo(entry.id)),
                    created_at_time: Some(entry.created_at.saturating_mul(1_000_000)),
                    amount: amount.into(),
                },
            },
            fee,
        )?;
    }
    schedule_finalize(Duration::from_secs(5));
    let tx = journal::execute(entry.id)
        .await
        .map_err(|e| format!("withdrawal operation {}: {e}", entry.id))?;
    journal::handled(entry.id);
    Ok(tx)
}

pub async fn fund_ledger_fees(owner: Principal, amount: u128) -> Result<BridgeTx, String> {
    if !STATE.with_borrow(|s| s.ledger_verified) {
        return Err("ledger verification is incomplete".into());
    }
    if amount == 0 {
        return Err("amount must be positive".into());
    }
    let _active = acquire_active_bridge_user(owner)?;
    let ledger = STATE.with_borrow(|s| s.token_ledger);
    let entry = match journal::open_funding(owner) {
        Some(entry) => {
            if !matches!(entry.purpose,journal::Purpose::FeeFunding {amount:old,ledger:old_ledger} if old==amount && old_ledger==ledger)
            {
                return Err(format!("resolve funding operation {} first", entry.id));
            }
            entry
        }
        None => journal::create(
            owner,
            journal::Purpose::FeeFunding { amount, ledger },
            now_ms(),
        ),
    };
    journal::prepare(
        entry.id,
        journal::Request::TransferFrom {
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
                fee: None,
                memo: Some(journal::memo(entry.id)),
                created_at_time: Some(entry.created_at.saturating_mul(1_000_000)),
                amount: amount.into(),
            },
        },
        0,
    )?;
    schedule_finalize(Duration::from_secs(5));
    let tx = journal::execute(entry.id)
        .await
        .map_err(|e| format!("fee funding operation {}: {e}", entry.id))?;
    journal::handled(entry.id);
    Ok(tx)
}
