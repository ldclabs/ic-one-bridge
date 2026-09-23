use super::*;

thread_local! {
    static RUNNING: Cell<bool> = const { Cell::new(false) };
    static RETRY_TIMER: RefCell<Option<ic_cdk_timers::TimerId>> = const { RefCell::new(None) };
    static FAILURES: Cell<u32> = const { Cell::new(0) };
}

struct InitFailure {
    message: String,
    retry: bool,
}

impl InitFailure {
    fn transient(message: String) -> Self {
        Self {
            message,
            retry: true,
        }
    }
    fn configuration(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry: false,
        }
    }
}

struct InitGuard;
impl Drop for InitGuard {
    fn drop(&mut self) {
        RUNNING.set(false);
    }
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs((5u64 << attempt.min(6)).min(300))
}

/// Refresh missing prerequisites independently. Temporary outages retry with a
/// single bounded-backoff timer; verified keys and metadata are reused.
pub async fn try_init_public_keys() {
    if RUNNING.replace(true) {
        return;
    }
    let _guard = InitGuard;
    if let Some(timer) = RETRY_TIMER.with_borrow_mut(Option::take) {
        ic_cdk_timers::clear_timer(timer);
    }
    let results = futures::join!(
        init_ecdsa_public_key(),
        init_ed25519_public_key(),
        verify_ledger_metadata(),
        verify_svm_mint(),
    );
    let mut retry = false;
    for result in [results.0, results.1, results.2, results.3] {
        if let Err(error) = result {
            retry |= error.retry;
            ic_cdk::api::debug_print(format!("bridge initialization failed: {}", error.message));
        }
    }
    crate::http_config::refresh();
    if retry {
        let attempt = FAILURES.get();
        FAILURES.set(attempt.saturating_add(1));
        let timer = ic_cdk_timers::set_timer(retry_delay(attempt), async {
            RETRY_TIMER.with_borrow_mut(Option::take);
            // Box breaks the recursive timer/future type, not the retry chain.
            Box::pin(try_init_public_keys()).await;
        });
        RETRY_TIMER.with_borrow_mut(|slot| *slot = Some(timer));
    } else {
        FAILURES.set(0);
    }
}

async fn init_ecdsa_public_key() -> Result<(), InitFailure> {
    let (key, missing) =
        STATE.with_borrow(|s| (s.key_name.clone(), s.ecdsa_public_key.public_key.is_empty()));
    if !missing {
        return Ok(());
    }
    let root = ecdsa_public_key(key, vec![])
        .await
        .map_err(InitFailure::transient)?;
    STATE.with_borrow_mut(|s| {
        let address =
            derive_evm_address(&root, &s.icp_address).map_err(InitFailure::configuration)?;
        s.ecdsa_public_key = root;
        s.evm_address = address;
        Ok(())
    })
}

async fn init_ed25519_public_key() -> Result<(), InitFailure> {
    let (key, missing) = STATE.with_borrow(|s| {
        (
            s.key_name.clone(),
            s.ed25519_public_key.public_key.is_empty(),
        )
    });
    if !missing {
        return Ok(());
    }
    let root = schnorr_public_key(key, vec![])
        .await
        .map_err(InitFailure::transient)?;
    STATE.with_borrow_mut(|s| {
        let address =
            derive_svm_address(&root, &s.icp_address).map_err(InitFailure::configuration)?;
        s.ed25519_public_key = root;
        s.svm_address = address;
        Ok(())
    })
}

async fn verify_ledger_metadata() -> Result<(), InitFailure> {
    let (ledger, decimals, verified) =
        STATE.with_borrow(|s| (s.token_ledger, s.token_decimals, s.ledger_verified));
    if verified {
        return Ok(());
    }
    let actual: u8 = crate::helper::read_call(ledger, "icrc1_decimals", ())
        .await
        .map_err(InitFailure::transient)?;
    let minting: Option<Account> = crate::helper::read_call(ledger, "icrc1_minting_account", ())
        .await
        .map_err(InitFailure::transient)?;
    if actual != decimals {
        return Err(InitFailure::configuration(
            "configured decimals disagree with the ledger",
        ));
    }
    if minting
        == Some(Account {
            owner: crate::helper::canister_id(),
            subaccount: None,
        })
    {
        return Err(InitFailure::configuration(
            "a lock/release bridge must not be the ledger minting account",
        ));
    }
    STATE.with_borrow_mut(|s| {
        if s.token_ledger != ledger || s.token_decimals != decimals {
            return Err(InitFailure::transient(
                "ledger configuration changed during verification".into(),
            ));
        }
        s.ledger_verified = true;
        s.ledger_minting_account = minting;
        Ok(())
    })?;
    schedule_finalize(Duration::ZERO);
    Ok(())
}

async fn verify_svm_mint() -> Result<(), InitFailure> {
    let (mint, providers, verified) = STATE.with_borrow(|s| {
        (
            s.svm_token_address,
            s.svm_providers.clone(),
            s.svm_mint_verified,
        )
    });
    if mint.0 == Pubkey::default() || verified {
        return Ok(());
    }
    let client = SvmClient::new(providers.clone(), DefaultHttpOutcall);
    let config = client
        .get_mint_config(&mint.0.to_string())
        .await
        .map_err(InitFailure::transient)?;
    if mint.1 != config.decimals || mint.2.to_string() != config.program {
        return Err(InitFailure::configuration(
            "configured SOL mint metadata disagrees with the providers",
        ));
    }
    STATE.with_borrow_mut(|s| {
        if s.svm_providers != providers || s.svm_token_address != mint {
            return Err(InitFailure::transient(
                "SOL configuration changed during verification".into(),
            ));
        }
        s.svm_mint_verified = true;
        s.svm_token_account_size = config.token_account_size;
        Ok(())
    })
}
