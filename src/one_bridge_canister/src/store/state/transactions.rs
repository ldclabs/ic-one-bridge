use super::*;

/// What an EVM transaction should do, before the nonce, the gas price and
/// the signature are attached.
struct EvmTxPlan {
    /// Where the transaction is sent: the token contract, or the recipient
    /// itself for a native transfer.
    to: Address,
    /// The address the funds end up at. It is the transaction destination for
    /// a native transfer and the `transfer` argument for an ERC-20 one, and
    /// paying our own address is always a mistake.
    recipient: Address,
    value: u128,
    input: Vec<u8>,
    gas_limit: u64,
    /// The token units an ERC-20 transfer moves, checked against the
    /// sender's balance when the sender is a user.
    token_transfer: Option<u128>,
}

fn signing_operation(funding: Funding) -> Result<Option<u64>, String> {
    let id = match funding {
        Funding::Verify => return Ok(None),
        Funding::Deposit(id) => id,
        Funding::Payout {
            task_id,
            run_generation,
        } => {
            let mut task =
                pending::get(task_id).ok_or_else(|| "payout task disappeared".to_string())?;
            payout_operation(&mut task, run_generation)
                .map_err(|_| "payout operation unavailable".to_string())?
                .ok_or_else(|| "payout round was superseded".to_string())?
        }
    };
    let entry = journal::get(id).ok_or_else(|| "signature operation disappeared".to_string())?;
    if !matches!(entry.phase, journal::Phase::Planning) {
        return Err(format!(
            "operation {id} already has a signature attempt; reconcile or reuse it"
        ));
    }
    Ok(Some(id))
}

fn signing_run_current(funding: Funding) -> Result<(), String> {
    if let Funding::Payout { run_generation, .. } = funding
        && !finalize_run_is_current(run_generation)
    {
        return Err("payout round was superseded".into());
    }
    if let Funding::Payout { task_id, .. } = funding {
        let task = pending::get(task_id).ok_or_else(|| "payout task disappeared".to_string())?;
        ensure_task_reconciled(&task)?;
    }
    Ok(())
}

/// Attaches the nonce, the gas price and the signature to a planned
/// transaction, refreshing the cached gas price when it has gone stale.
async fn sign_evm_tx(
    chain: &str,
    from: &Principal,
    plan: EvmTxPlan,
    now_ms: u64,
    funding: Funding,
) -> Result<SignedTransfer, String> {
    let operation = signing_operation(funding)?;
    let EvmTxPlan {
        to,
        recipient,
        value,
        input,
        gas_limit,
        token_transfer,
    } = plan;

    let (key_name, from_pk, mut tx, gas_updated_at) = STATE.with_borrow(|s| {
        let (_, _, chain_id) = s
            .evm_token_contracts
            .get(chain)
            .ok_or_else(|| format!("chain {chain} not found"))?;
        let from_pk = derive_public_key(&s.ecdsa_public_key, vec![from.as_slice().to_vec()])
            .map_err(|err| format!("derive_public_key failed: {err}"))?;

        let (gas_updated_at, gas_price, max_priority_fee_per_gas) =
            s.evm_latest_gas.get(chain).cloned().unwrap_or_default();
        let max_priority_fee_per_gas = bump_priority_fee(max_priority_fee_per_gas)?;
        let max_fee_per_gas = calculate_max_fee_per_gas(gas_price, max_priority_fee_per_gas)?;

        Ok::<_, String>((
            s.key_name.clone(),
            from_pk,
            TxEip1559 {
                chain_id: *chain_id,
                nonce: 0u64,
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                to: to.into(),
                value: value.try_into().map_err(|_| "invalid amount".to_string())?,
                input: input.into(),
                ..Default::default()
            },
            gas_updated_at,
        ))
    })?;

    let from_addr = from_pk.to_evm_address()?;
    if from_addr == recipient {
        return Err("from and to cannot be the same".to_string());
    }

    let client = evm_client(chain)?;
    // The reads are independent, so they share one round of outcalls: the
    // nonce, a gas quote when the cached one is stale, and, for a sender
    // other than the bridge, the balances it pays with.
    let gas_is_fresh = gas_updated_at.saturating_add(120_000) >= now_ms;
    let verify_funds = !matches!(funding, Funding::Payout { .. });
    let gas_quote = async {
        if gas_is_fresh {
            Ok(None)
        } else {
            futures::future::try_join(client.gas_price(), client.max_priority_fee_per_gas())
                .await
                .map(Some)
        }
    };
    let funds = async {
        if !verify_funds {
            return Ok(None);
        }
        let token_balance = async {
            match token_transfer {
                Some(_) => client.erc20_balance_of(&to, &from_addr).await.map(Some),
                None => Ok(None),
            }
        };
        futures::future::try_join(client.get_balance(&from_addr), token_balance)
            .await
            .map(Some)
    };
    let (nonce, gas_quote, funds) =
        futures::future::try_join3(client.get_transaction_count(&from_addr), gas_quote, funds)
            .await?;
    tx.nonce = nonce;
    if let Some((gas_price, max_priority_fee_per_gas)) = gas_quote {
        tx.max_priority_fee_per_gas = bump_priority_fee(max_priority_fee_per_gas)?;
        tx.max_fee_per_gas = calculate_max_fee_per_gas(gas_price, tx.max_priority_fee_per_gas)?;
        STATE.with_borrow_mut(|s| {
            s.evm_latest_gas.insert(
                chain.to_string(),
                (now_ms, gas_price, max_priority_fee_per_gas),
            );
        })
    }
    if let Some((balance, token_balance)) = funds {
        check_evm_funds(&from_addr, &tx, balance, token_transfer.zip(token_balance))?;
    }

    signing_run_current(funding)?;
    let fee_limits =
        STATE.with_borrow(|s| s.evm_fee_limits.get(chain).cloned().unwrap_or_default());
    let cost = fee_limits.transaction_cost(
        tx.gas_limit,
        tx.max_fee_per_gas,
        tx.max_priority_fee_per_gas,
    )?;
    if matches!(funding, Funding::Payout { .. }) {
        budget::reserve_gas(chain, cost, now_ms, fee_limits.max_hourly_fee)?;
    }
    let msg_hash = tx.signature_hash();
    if let Some(id) = operation {
        journal::prepare(
            id,
            journal::Request::Signature {
                scheme: "ecdsa".into(),
                key_name: key_name.clone(),
                sender: *from,
                message: msg_hash.to_vec().into(),
                deadline: TxDeadline::Nonce(tx.nonce),
                validity: None,
            },
        )?;
        journal::start_signature(id)?;
    }
    let sig = crate::ecdsa::sign_with_ecdsa_result(
        key_name,
        vec![from.as_slice().to_vec()],
        msg_hash.to_vec(),
    )
    .await
    .map_err(|e| {
        if let Some(id) = operation {
            journal::failed(id, e.message.clone(), e.ambiguous, false);
        }
        e.message
    })?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::after_signature_reply();
    if sig.len() != 64 {
        return Err(format!("invalid ECDSA signature length: {}", sig.len()));
    }
    let signature = Signature::new(
        U256::from_be_slice(&sig[0..32]),  // r
        U256::from_be_slice(&sig[32..64]), // s
        y_parity(msg_hash.as_slice(), &sig, from_pk.public_key.as_slice())?,
    );

    let signed = tx.into_signed(signature);
    let raw = signed.encoded_2718();
    let transfer = SignedTransfer {
        tx: BridgeTx::Evm(
            false,
            <[u8; 32]>::from(alloy_primitives::keccak256(&raw)).into(),
        ),
        meta: TxMeta {
            deadline: TxDeadline::Nonce(signed.tx().nonce),
            raw: Some(raw.into()),
            svm_validity: None,
        },
    };
    if let Some(id) = operation {
        journal::record_signed(id, &transfer, sig)?;
    }
    Ok(transfer)
}

/// Refuses to sign for an address that cannot pay for the transaction:
/// its native balance must cover the value and the gas, and for a token
/// transfer its token balance, `(amount, held)`, must cover the amount.
fn check_evm_funds(
    from: &Address,
    tx: &TxEip1559,
    balance: U256,
    token: Option<(u128, U256)>,
) -> Result<(), String> {
    let gas = U256::from(tx.gas_limit).saturating_mul(U256::from(tx.max_fee_per_gas));
    let needed = gas.saturating_add(tx.value);
    if balance < needed {
        return Err(format!(
            "address {from} holds {balance} wei, and the transaction needs {needed} for its value and gas"
        ));
    }
    if let Some((amount, held)) = token
        && held < U256::from(amount)
    {
        return Err(format!(
            "address {from} holds {held} token units, and the transfer needs {amount}"
        ));
    }
    Ok(())
}

pub async fn build_erc20_transfer_tx(
    chain: &str,
    from: &Principal,
    to_addr: &Address,
    icp_amount: u128,
    now_ms: u64,
    funding: Funding,
) -> Result<SignedTransfer, String> {
    let plan = STATE.with_borrow(|s| {
        let (contract, decimals, _) = s
            .evm_token_contracts
            .get(chain)
            .ok_or_else(|| format!("chain {chain} not found"))?;

        let value = convert_amount(icp_amount, s.token_decimals, *decimals)?;
        if value == 0 {
            return Err(format!(
                "{chain}: amount {icp_amount} is too small for target token decimals {decimals}"
            ));
        }

        Ok::<_, String>(EvmTxPlan {
            to: *contract,
            recipient: *to_addr,
            value: 0,
            input: encode_erc20_transfer(to_addr, value),
            gas_limit: s.erc20_gas_limit,
            token_transfer: Some(value),
        })
    })?;

    sign_evm_tx(chain, from, plan, now_ms, funding).await
}

pub async fn build_evm_transfer_tx(
    chain: &str,
    from: &Principal,
    to_addr: &Address,
    amount: u128,
    now_ms: u64,
    funding: Funding,
) -> Result<SignedTransfer, String> {
    if amount == 0 {
        return Err("amount must be greater than 0".to_string());
    }

    let plan = EvmTxPlan {
        to: *to_addr,
        recipient: *to_addr,
        value: amount,
        input: Vec::new(),
        gas_limit: NATIVE_TRANSFER_GAS_LIMIT,
        token_transfer: None,
    };

    sign_evm_tx(chain, from, plan, now_ms, funding).await
}

pub async fn build_spl_transfer_tx(
    from: &Principal,
    to_addr: &Pubkey,
    icp_amount: u128,
    funding: Funding,
) -> Result<SignedTransfer, String> {
    let (from_addr, from_ata, to_ata, amount, ixs) = STATE.with_borrow(|s| {
        if !s.svm_mint_verified && !matches!(funding, Funding::Verify) {
            return Err("SOL mint has not passed the supported-token checks".to_string());
        }
        let (mint_pubkey, decimals, token_program_id) = s.svm_token_address;
        if mint_pubkey == Pubkey::default()
            || ![crate::svm::TOKEN_PROGRAM, crate::svm::TOKEN_2022_PROGRAM]
                .contains(&token_program_id.to_string().as_str())
        {
            return Err("unsupported SOL token program".into());
        }

        let amount = convert_amount(icp_amount, s.token_decimals, decimals)?;
        if amount == 0 {
            return Err(format!(
                "amount {icp_amount} is too small for target token decimals {decimals}"
            ));
        }
        let amount: u64 = amount
            .try_into()
            .map_err(|_| format!("amount is too large: {}", amount))?;
        let from_addr = derive_svm_address(&s.ed25519_public_key, from)?;
        if &from_addr == to_addr {
            return Err("from and to cannot be the same".to_string());
        }

        let from_ata = get_associated_token_address(&from_addr, &mint_pubkey, &token_program_id);
        let to_ata = get_associated_token_address(to_addr, &mint_pubkey, &token_program_id);
        let ix0 = create_associated_token_account_idempotent(
            &from_addr,
            to_addr,
            &to_ata,
            &mint_pubkey,
            &token_program_id,
        );
        let ix = transfer_checked_instruction(
            &token_program_id,
            &from_ata,
            &mint_pubkey,
            &to_ata,
            &from_addr,
            &[],
            amount,
            decimals,
        );

        Ok::<_, String>((from_addr, from_ata, to_ata, amount, vec![ix0, ix]))
    })?;

    let client = svm_client();
    if !matches!(funding, Funding::Payout { .. }) {
        let (lamports, tokens, to_ata_exists) = futures::future::try_join3(
            client.get_balance(&from_addr.to_string()),
            client.get_token_account_balance(&from_ata.to_string()),
            client.account_exists(&to_ata.to_string()),
        )
        .await
        .map_err(|err| format!("failed to read the balances of {from_addr}: {err}"))?;
        if tokens < amount {
            return Err(format!(
                "address {from_addr} holds {tokens} token units, and the transfer needs {amount}"
            ));
        }
        // the first instruction opens the recipient's token account when it
        // is missing, and the fee payer is the one who funds its rent
        let needed = if to_ata_exists {
            SOL_TX_FEE_LAMPORTS
        } else {
            let size = STATE.with_borrow(|s| s.svm_token_account_size);
            if size == 0 {
                return Err("create the recipient's token account before withdrawing a legacy mint with unsupported extensions".into());
            }
            SOL_TX_FEE_LAMPORTS.saturating_add(client.account_rent(size).await?)
        };
        if lamports < needed {
            return Err(format!(
                "address {from_addr} holds {lamports} lamports, and the transaction needs {needed} for its fee{}",
                if to_ata_exists {
                    ""
                } else {
                    " and the recipient's new token account"
                }
            ));
        }
    }

    sign_svm_tx(client, from, from_addr, &ixs, funding).await
}

pub async fn build_sol_transfer_tx(
    from: &Principal,
    to_addr: &Pubkey,
    sol_amount: u64,
    funding: Funding,
) -> Result<SignedTransfer, String> {
    if sol_amount == 0 {
        return Err("amount must be greater than 0".to_string());
    }

    let (from_addr, ixs) = STATE.with_borrow(|s| {
        let from_addr = derive_svm_address(&s.ed25519_public_key, from)?;
        if &from_addr == to_addr {
            return Err("from and to cannot be the same".to_string());
        }

        let ix = system_transfer_instruction(&from_addr, to_addr, sol_amount);
        Ok::<_, String>((from_addr, vec![ix]))
    })?;

    let client = svm_client();
    if !matches!(funding, Funding::Payout { .. }) {
        let lamports = client.get_balance(&from_addr.to_string()).await?;
        let needed = sol_amount.saturating_add(SOL_TX_FEE_LAMPORTS);
        if lamports < needed {
            return Err(format!(
                "address {from_addr} holds {lamports} lamports, and the transfer needs {needed} with its fee"
            ));
        }
    }

    sign_svm_tx(client, from, from_addr, &ixs, funding).await
}

/// Attaches a recent blockhash and the signature of `from` to the planned
/// instructions, with `from_addr` — the address `from` derives to — as
/// the fee payer.
async fn sign_svm_tx(
    client: SvmClient<DefaultHttpOutcall>,
    from: &Principal,
    from_addr: Pubkey,
    ixs: &[Instruction],
    funding: Funding,
) -> Result<SignedTransfer, String> {
    let operation = signing_operation(funding)?;
    let key_name = STATE.with_borrow(|s| s.key_name.clone());
    let blockhash = client
        .get_latest_blockhash()
        .await
        .map_err(|err| format!("failed to get latest blockhash, error: {err}"))?;

    let message_id = operation.unwrap_or_else(pending::next_id);
    let mut instructions = ixs.to_vec();
    instructions.push(Instruction {
        program_id: Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
        accounts: vec![],
        data: format!("1bridge:{message_id}").into_bytes(),
    });
    let message =
        Message::new_with_blockhash(&instructions, Some(&from_addr), &blockhash.to_hash()?);
    let msg = bincode::serialize(&message).map_err(|err| err.to_string())?;
    signing_run_current(funding)?;
    if let Some(id) = operation {
        journal::prepare(
            id,
            journal::Request::Signature {
                scheme: "ed25519".into(),
                key_name: key_name.clone(),
                sender: *from,
                message: msg.clone().into(),
                deadline: TxDeadline::BlockHeight(blockhash.last_valid_block_height),
                validity: Some(blockhash.clone().into()),
            },
        )?;
        journal::start_signature(id)?;
    }
    let sig =
        crate::schnorr::sign_with_schnorr_result(key_name, vec![from.as_slice().to_vec()], msg)
            .await
            .map_err(|e| {
                if let Some(id) = operation {
                    journal::failed(id, e.message.clone(), e.ambiguous, false);
                }
                e.message
            })?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::after_signature_reply();
    let signature: [u8; 64] = sig
        .try_into()
        .map_err(|_| "invalid signature length".to_string())?;
    let transaction = Transaction {
        message,
        signatures: vec![signature.into()],
    };

    let validity: SolValidity = blockhash.into();
    let transfer = SignedTransfer {
        tx: BridgeTx::Sol(false, signature.into()),
        meta: TxMeta {
            deadline: TxDeadline::BlockHeight(validity.last_valid_block_height),
            raw: Some(
                bincode::serialize(&transaction)
                    .map_err(|e| e.to_string())?
                    .into(),
            ),
            svm_validity: Some(validity),
        },
    };
    if let Some(id) = operation {
        journal::record_signed(id, &transfer, signature.to_vec())?;
    }
    Ok(transfer)
}
