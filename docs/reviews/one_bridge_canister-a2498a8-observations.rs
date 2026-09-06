// These tests assert the existing problematic behavior. They are evidence,
// not regression tests claiming that the reported problems have been fixed.
#[cfg(test)]
mod audit_observations {
    use super::*;
    use crate::outcall::tests::{MockHttpOutcall, result, success_response};
    use serde_json::json;

    fn task(id: u64, to: BridgeTarget) -> BridgeLog {
        BridgeLog {
            id: None,
            user: Principal::from_slice(&[1, 2, 3]),
            from: BridgeTarget::Icp,
            to,
            icp_amount: 100,
            fee: 1,
            from_tx: BridgeTx::Icp(true, id),
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

    fn evm(mock: &MockHttpOutcall) -> EvmClient<MockHttpOutcall> {
        EvmClient::new(
            vec!["https://a".into(), "https://b".into(), "https://c".into()],
            0,
            mock.clone(),
        )
    }

    fn sol(mock: &MockHttpOutcall) -> SvmClient<MockHttpOutcall> {
        SvmClient::new(
            vec!["https://a".into(), "https://b".into(), "https://c".into()],
            mock.clone(),
        )
    }

    #[test]
    fn audit_observes_sol_failure_before_finality() {
        let one = || {
            crate::svm::SolTxStatus::from_signature_status(Some(crate::svm::SignatureStatus {
                slot: 100,
                confirmations: Some(1),
                confirmation_status: Some("processed".into()),
                err: Some(json!({"InstructionError": [1, "InsufficientFunds"]})),
            }))
        };
        assert!(matches!(
            SolTxStatus::reconcile(one(), one()).unwrap(),
            SolTxStatus::Failed(_)
        ));
    }

    #[test]
    fn audit_observes_expiry_based_on_one_unverified_deadline() {
        // Fixture: the returned blockhash is valid until height 200, but
        // provider A lies about lastValidBlockHeight, supplying 1.
        let unknown = json!({"context":{"slot":100},"value":[null]});
        let mock = MockHttpOutcall::new(vec![
            result(json!({"context":{"slot":100},"value":{
                "blockhash":"3Xdj6drp4pKAM9PH2vZ4w8NHygd8Epp7FKCvzX29VLLH",
                "lastValidBlockHeight":1
            }})),
            result(json!(100)),
            result(unknown.clone()),
            result(json!(100)),
            result(unknown),
        ]);
        let client = sol(&mock);
        let reported = futures::executor::block_on(client.get_latest_blockhash()).unwrap();
        assert_eq!(mock.urls().len(), 1);
        assert!(
            futures::executor::block_on(client.expired(
                "withheld-signature",
                reported.last_valid_block_height,
                32
            ))
            .unwrap()
        );
        assert!(!mock.methods().iter().any(|m| m == "isBlockhashValid"));
    }

    #[test]
    fn audit_observes_a_single_provider_setting_extreme_tip() {
        let tip = 1_000_000_000_000_000u128;
        let mock = MockHttpOutcall::new(vec![result(json!(format!("0x{tip:x}")))]);
        let received = futures::executor::block_on(evm(&mock).max_priority_fee_per_gas()).unwrap();
        assert_eq!(received, tip);
        assert_eq!(mock.urls().len(), 1);
        let bumped = bump_priority_fee(received).unwrap();
        let max_fee = calculate_max_fee_per_gas(1_000_000_000, bumped).unwrap();
        assert!(max_fee >= bumped);
        // 54,000 gas at this accepted tip exceeds 64 native coins.
        assert!(54_000u128 * bumped > 64_000_000_000_000_000_000);
    }

    #[test]
    fn audit_observes_replacement_verdict_from_latest_nonce() {
        let mock = MockHttpOutcall::new(vec![
            result(json!("0x5")),
            result(json!(null)),
            result(json!("0x5")),
            result(json!(null)),
        ]);
        assert!(
            futures::executor::block_on(evm(&mock).replaced(
                &Address::from([1; 20]),
                4,
                &TxHash::from([2; 32])
            ))
            .unwrap()
        );
        for request in mock.requests() {
            let body: serde_json::Value = serde_json::from_slice(&request.body.unwrap()).unwrap();
            if body["method"] == "eth_getTransactionCount" {
                assert_eq!(body["params"][1], "latest");
            }
        }
    }

    #[test]
    fn audit_observes_different_receipt_block_hashes_lost() {
        let value = |b: u8| {
            json!({
                "transactionHash":format!("0x{}", "11".repeat(32)),
                "blockHash":format!("0x{}", format!("{b:02x}").repeat(32)),
                "blockNumber":"0x64", "status":"0x1", "logs":[]
            })
        };
        let a: EvmReceipt = serde_json::from_value(value(1)).unwrap();
        let b: EvmReceipt = serde_json::from_value(value(2)).unwrap();
        assert!(
            crate::evm::same_or_absent(Some(a), Some(b))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn audit_observes_provider_rate_limit_bypassing_healthy_fallbacks() {
        let mock = MockHttpOutcall::new(vec![
            success_response(
                json!({"jsonrpc":"2.0","id":1,"error":{"code":-32005,"message":"rate limit exceeded"}}),
            ),
            result(json!("0x38")),
            result(json!("0x38")),
        ]);
        let error = futures::executor::block_on(evm(&mock).chain_id()).unwrap_err();
        assert!(error.contains("rate limit exceeded"));
        assert_eq!(mock.urls().len(), 1);
    }

    #[test]
    fn audit_observes_three_evm_payouts_starving_a_healthy_sol_task() {
        let mut queue = VecDeque::new();
        for (i, chain) in ["ETH", "BNB", "BASE"].into_iter().enumerate() {
            let mut log = task(i as u64, BridgeTarget::Evm(chain.into()));
            log.to_tx = Some(BridgeTx::Evm(false, [i as u8; 32].into()));
            queue.push_back(log);
        }
        queue.push_back(task(99, BridgeTarget::Sol));
        for _ in 0..100 {
            let picked = select_round_tasks(&queue, ROUND_TASK_LIMIT);
            assert_eq!(picked.len(), 3);
            assert!(picked.iter().all(|t| t.from_tx != BridgeTx::Icp(true, 99)));
            let processed = picked.into_iter().map(|t| t.from_tx).collect::<Vec<_>>();
            rotate_processed(&mut queue, &processed);
        }
    }

    #[test]
    fn audit_observes_precision_validation_accepting_unbuildable_amounts() {
        let amount = 2_000_000_000u128;
        assert!(check_payout_precision(amount, 8, 18).is_ok());
        let in_sol_units = convert_amount(amount, 8, 18).unwrap();
        assert!(u64::try_from(in_sol_units).is_err());
        assert!(check_payout_precision(amount, 8, 46).is_ok());
        assert!(convert_amount(amount, 8, 46).is_err());
    }

    #[test]
    fn audit_observes_rollback_then_upgrade_missing_new_history() {
        use ic_stable_structures::VectorMemory;
        let user = Principal::from_slice(&[1, 2, 3]);
        let mut old: StableBTreeMap<Principal, LegacyUserLogs, VectorMemory> =
            StableBTreeMap::new(VectorMemory::default());
        let mut new: StableBTreeMap<UserLogKey, (), VectorMemory> =
            StableBTreeMap::new(VectorMemory::default());
        old.insert(
            user,
            LegacyUserLogs {
                logs: BTreeSet::from([0]),
            },
        );
        assert_eq!(copy_legacy_user_log_index(&old, &mut new), 1);
        // A rolled-back version appends only to the old index.
        old.insert(
            user,
            LegacyUserLogs {
                logs: BTreeSet::from([0, 1]),
            },
        );
        assert_eq!(copy_legacy_user_log_index(&old, &mut new), 0);
        assert_eq!(user_log_ids(&new, &user, u64::MAX, 100), vec![0]);
    }

    #[test]
    fn audit_observes_spl_business_id_absent_from_transaction_message() {
        let from = Pubkey::new_from_array([1; 32]);
        let to = Pubkey::new_from_array([2; 32]);
        let mint = Pubkey::new_from_array([3; 32]);
        let program = Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
        let from_ata = get_associated_token_address(&from, &mint, &program);
        let to_ata = get_associated_token_address(&to, &mint, &program);
        let ixs = vec![
            create_associated_token_account_idempotent(&from, &to, &mint, &program),
            transfer_checked_instruction(&program, &from_ata, &mint, &to_ata, &from, &[], 100, 8),
        ];
        let hash = crate::svm::Hash::new_from_array([4; 32]);
        let first = Message::new_with_blockhash(&ixs, Some(&from), &hash);
        let second = Message::new_with_blockhash(&ixs, Some(&from), &hash);
        assert_eq!(
            bincode::serialize(&first).unwrap(),
            bincode::serialize(&second).unwrap()
        );
        // ICP signatures are randomized. This demonstrates equal messages,
        // not equal signatures or a proven duplicate-credit vulnerability.
    }

    #[test]
    fn audit_observes_transfer_fee_mint_accepted_without_extension_validation() {
        let account: crate::svm::UiAccount = serde_json::from_value(json!({
            "owner":"TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
            "data":{"program":"spl-token-2022","parsed":{"type":"mint","info":{
                "decimals":8,"isInitialized":true,"extensions":[{
                    "extension":"transferFeeConfig","state":{
                        "newerTransferFee":{"epoch":1,"maximumFee":"1000000000","transferFeeBasisPoints":100}
                    }
                }]
            }}}
        })).unwrap();
        assert_eq!(crate::svm::get_mint_decimals(&account), Ok(8));
    }

    #[test]
    fn audit_observes_existing_ata_rejected_when_its_balance_changes_between_reads() {
        let account = |amount: &str| {
            json!({"context":{"slot":100},"value":{
                "owner":"TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
                "data":{"program":"spl-token","parsed":{"type":"account","info":{
                    "mint":"mint","owner":"recipient","tokenAmount":{"amount":amount,"decimals":8}
                }}}
            }})
        };
        let mock = MockHttpOutcall::new(vec![result(account("10")), result(account("11"))]);
        let error =
            futures::executor::block_on(sol(&mock).get_account_info("recipient-ata")).unwrap_err();
        assert!(error.contains("providers disagree"));
    }

    #[test]
    fn audit_observes_legacy_pending_record_without_fee_failing_decode() {
        // Pending records before f390b32 (2025-10-17) have these field names
        // and no fee. Unlike BridgeLogLocal, BridgeLog has no default for fee.
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
            user: Principal::from_slice(&[1, 2, 3]),
            from: BridgeTarget::Icp,
            to: BridgeTarget::Evm("BNB".into()),
            icp_amount: 100,
            from_tx: BridgeTx::Icp(true, 0),
            to_tx: None,
            to_addr: None,
            created_at: 1,
            finalized_at: 0,
            error: None,
        };
        let bytes = cbor_into_vec(&old).unwrap();
        let result: Result<BridgeLog, _> = cbor_from_slice(&bytes);
        assert!(result.is_err());
        assert!(format!("{:?}", result.err().unwrap()).contains("fee"));
    }
}
