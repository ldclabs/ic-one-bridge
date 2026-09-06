use ic_auth_types::ByteBufB64;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::types::*;
use crate::outcall::{
    Agreement, HttpOutcall, LARGE_RESPONSE, RpcCall, SMALL_RESPONSE, as_is, json_rpc_call, lower,
    same, two_provider_verdict,
};

/// Commitment a blockhash is fetched and a transaction is sent at, and that
/// balances are read at.
const COMMITMENT: &str = "confirmed";

/// Commitment the block height is read at when deciding whether a transaction
/// can still land: a finalized height lags the tip, which errs towards waiting.
const FINALIZED: &str = "finalized";

pub struct SvmClient<T: HttpOutcall> {
    pub providers: Vec<String>,
    outcall: T,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct RpcContext {
    pub slot: u64,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct RpcContextValue<T> {
    pub context: RpcContext,
    pub value: T,
}

#[derive(Debug, Deserialize)]
struct TokenAmount {
    amount: String,
}

impl<H: HttpOutcall> SvmClient<H> {
    pub fn new(providers: Vec<String>, outcall: H) -> Self {
        Self { providers, outcall }
    }

    /// A candidate is obtained once, then two providers must recognize that
    /// exact hash at a view at least as new as the candidate's context.
    pub async fn get_latest_blockhash(&self) -> Result<LatestBlockhash, String> {
        let candidate = self
            .call(
                "getLatestBlockhash",
                &[json!({ "commitment": COMMITMENT })],
                SMALL_RESPONSE,
                |res: RpcContextValue<LatestBlockhash>| {
                    let mut value = res.value;
                    value.to_hash()?;
                    value.context_slot = res.context.slot;
                    Ok(value)
                },
                Agreement::First,
            )
            .await?;
        let valid = self
            .call(
                "isBlockhashValid",
                &[
                    json!(candidate.blockhash),
                    json!({"commitment": COMMITMENT, "minContextSlot": candidate.context_slot}),
                ],
                SMALL_RESPONSE,
                |res: RpcContextValue<bool>| {
                    if res.context.slot < candidate.context_slot {
                        return Err("blockhash provider is behind its required context".into());
                    }
                    Ok(res.value)
                },
                Agreement::Two(|a, b| Ok(a && b)),
            )
            .await?;
        if !valid {
            return Err("two providers must validate the recent blockhash".into());
        }
        Ok(candidate)
    }

    /// The status of a transaction, as two providers support it together; see
    /// [`SolTxStatus::reconcile`].
    pub async fn get_signature_status(&self, signature: &str) -> Result<SolTxStatus, String> {
        self.get_signature_statuses(&[signature.to_string()])
            .await?
            .pop()
            .ok_or_else(|| "missing signature status".to_string())
    }

    pub async fn get_signature_statuses(
        &self,
        signatures: &[String],
    ) -> Result<Vec<SolTxStatus>, String> {
        if signatures.is_empty() {
            return Ok(Vec::new());
        }
        if signatures.len() > 32 {
            return Err("at most 32 signatures per batch".into());
        }
        self.call(
            "getSignatureStatuses",
            &[
                json!(signatures),
                json!({ "searchTransactionHistory": true }),
            ],
            SMALL_RESPONSE,
            |res: RpcContextValue<Vec<Option<SignatureStatus>>>| {
                if res.value.len() != signatures.len() {
                    return Err("missing signature status".into());
                }
                Ok(res
                    .value
                    .into_iter()
                    .map(SolTxStatus::from_signature_status)
                    .collect::<Vec<_>>())
            },
            Agreement::Two(|a: Vec<SolTxStatus>, b: Vec<SolTxStatus>| {
                if a.len() != b.len() {
                    return Err("signature batch lengths disagree".into());
                }
                a.into_iter()
                    .zip(b)
                    .map(|(a, b)| SolTxStatus::reconcile(a, b))
                    .collect()
            }),
        )
        .await
    }

    /// Never uses the advertised lastValidBlockHeight as evidence. Each provider
    /// must have finalized the original context, reject the actual hash, retain
    /// the relevant history and report no signature. Unknown history is an error.
    pub async fn expired(&self, signature: &str, validity: &SolValidity) -> Result<bool, String> {
        two_provider_verdict(&self.providers, "expiry check", |one| async move {
            let valid: RpcContextValue<bool> = json_rpc_call(
                &self.outcall,
                one,
                RpcCall {
                    method: "isBlockhashValid",
                    params: &[json!(validity.blockhash), json!({ "commitment": FINALIZED, "minContextSlot": validity.context_slot })],
                    max_response_bytes: SMALL_RESPONSE,
                },
                as_is,
                Agreement::First,
            )
            .await?;
            if valid.context.slot < validity.context_slot || valid.value {
                return Ok(false);
            }
            let first_slot: u64 = json_rpc_call(&self.outcall, one, RpcCall {
                method: "getFirstAvailableBlock", params: &[], max_response_bytes: SMALL_RESPONSE,
            }, as_is, Agreement::First).await?;
            if first_slot > validity.context_slot {
                return Err("provider history no longer covers this transaction; manual reconciliation required".into());
            }
            let status = json_rpc_call(
                &self.outcall,
                one,
                RpcCall {
                    method: "getSignatureStatuses",
                    params: &[
                        Value::Array(vec![signature.into()]),
                        json!({ "searchTransactionHistory": true }),
                    ],
                    max_response_bytes: SMALL_RESPONSE,
                },
                |res: RpcContextValue<Vec<Option<SignatureStatus>>>| {
                    res.value
                        .into_iter()
                        .next()
                        .map(SolTxStatus::from_signature_status)
                        .ok_or_else(|| "missing signature status".to_string())
                },
                Agreement::First,
            )
            .await?;
            Ok(status == SolTxStatus::Unknown)
        })
        .await
    }

    /// The lamports `pubkey` holds, the lower of two providers' views.
    pub async fn get_balance(&self, pubkey: &str) -> Result<u64, String> {
        self.call(
            "getBalance",
            &[
                Value::String(pubkey.to_string()),
                json!({ "commitment": COMMITMENT }),
            ],
            SMALL_RESPONSE,
            |res: RpcContextValue<u64>| Ok(res.value),
            Agreement::Two(lower),
        )
        .await
    }

    /// The token units a token account holds, the lower of two providers'
    /// views. An account that does not exist is a JSON-RPC error.
    pub async fn get_token_account_balance(&self, account: &str) -> Result<u64, String> {
        self.call(
            "getTokenAccountBalance",
            &[
                Value::String(account.to_string()),
                json!({ "commitment": COMMITMENT }),
            ],
            SMALL_RESPONSE,
            |res: RpcContextValue<TokenAmount>| {
                res.value
                    .amount
                    .parse::<u64>()
                    .map_err(|err| format!("token amount {}: {err}", res.value.amount))
            },
            Agreement::Two(lower),
        )
        .await
    }

    /// Preflight is always skipped: the canister signs complete transactions
    /// and polls their status itself, and a preflight simulation would only add
    /// a second chance for a provider to reject a transaction it has already
    /// been handed.
    pub async fn send_transaction(&self, transaction: ByteBufB64) -> Result<String, String> {
        self.call(
            "sendTransaction",
            &[
                Value::String(transaction.to_base64()),
                json!({
                    "encoding": "base64",
                    "commitment": COMMITMENT,
                    "skipPreflight": true,
                }),
            ],
            SMALL_RESPONSE,
            as_is,
            Agreement::First,
        )
        .await
    }

    pub async fn get_mint_config(&self, pubkey: &str) -> Result<MintConfig, String> {
        self.call(
            "getAccountInfo",
            &[
                Value::String(pubkey.to_string()),
                json!({ "commitment": COMMITMENT, "encoding": "jsonParsed" }),
            ],
            LARGE_RESPONSE,
            |res: RpcContextValue<Option<UiAccount>>| {
                mint_config(
                    &res.value
                        .ok_or_else(|| "mint account does not exist".to_string())?,
                )
            },
            Agreement::Two(same),
        )
        .await
    }

    pub async fn account_exists(&self, pubkey: &str) -> Result<bool, String> {
        self.call("getAccountInfo", &[json!(pubkey), json!({"commitment": COMMITMENT, "encoding":"base64", "dataSlice":{"offset":0,"length":0}})],
            SMALL_RESPONSE, |res: RpcContextValue<Option<Value>>| Ok(res.value.is_some()),
            Agreement::Two(|a,b| Ok(a && b))).await
    }

    pub async fn account_rent(&self, size: u64) -> Result<u64, String> {
        self.call(
            "getMinimumBalanceForRentExemption",
            &[json!(size), json!({"commitment": COMMITMENT})],
            SMALL_RESPONSE,
            as_is,
            Agreement::Two(same),
        )
        .await
    }

    async fn call<R: DeserializeOwned, T>(
        &self,
        method: &str,
        params: &[Value],
        max_response_bytes: u64,
        interpret: impl Fn(R) -> Result<T, String>,
        agreement: Agreement<T>,
    ) -> Result<T, String> {
        json_rpc_call(
            &self.outcall,
            &self.providers,
            RpcCall {
                method,
                params,
                max_response_bytes,
            },
            interpret,
            agreement,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcall::tests::{MockHttpOutcall, result, success_response};

    fn client(mock: &MockHttpOutcall, providers: usize) -> SvmClient<MockHttpOutcall> {
        SvmClient::new(
            (0..providers).map(|i| format!("https://sol{i}")).collect(),
            mock.clone(),
        )
    }

    fn blockhash_json() -> Value {
        json!({
            "context": { "slot": 1234 },
            "value": {
                "blockhash": "3Xdj6drp4pKAM9PH2vZ4w8NHygd8Epp7FKCvzX29VLLH",
                "lastValidBlockHeight": 355385114
            }
        })
    }

    #[test]
    fn test_get_latest_blockhash() {
        let mock = MockHttpOutcall::new(vec![
            result(blockhash_json()),
            result(json!({"context":{"slot":1234},"value":true})),
            result(json!({"context":{"slot":1235},"value":true})),
        ]);

        let response =
            futures::executor::block_on(client(&mock, 2).get_latest_blockhash()).unwrap();

        assert_eq!(
            response.to_hash().unwrap().to_string(),
            "3Xdj6drp4pKAM9PH2vZ4w8NHygd8Epp7FKCvzX29VLLH"
        );
        assert_eq!(response.last_valid_block_height, 355385114);
        assert_eq!(
            mock.urls(),
            vec![
                "https://sol0".to_string(),
                "https://sol0".to_string(),
                "https://sol1".to_string()
            ]
        );
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE); 3]);
    }

    #[test]
    fn test_http_request_fallbacks_between_providers() {
        let mock = MockHttpOutcall::new(vec![
            Err("timeout".to_string()),
            result(blockhash_json()),
            result(json!({"context":{"slot":1234},"value":true})),
            result(json!({"context":{"slot":1235},"value":true})),
        ]);

        futures::executor::block_on(client(&mock, 2).get_latest_blockhash()).unwrap();

        assert_eq!(
            mock.urls(),
            vec![
                "https://sol0".to_string(),
                "https://sol1".to_string(),
                "https://sol0".to_string(),
                "https://sol1".to_string()
            ]
        );
    }

    #[test]
    fn test_call_handles_error_payload() {
        let mock = MockHttpOutcall::new(vec![success_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32000, "message": "custom error"}
        }))]);

        let result = futures::executor::block_on(client(&mock, 2).get_balance("pk"));
        assert!(result.unwrap_err().contains("custom error"));
    }

    #[test]
    fn test_send_transaction_returns_signature() {
        let mock = MockHttpOutcall::new(vec![result("5N7signature".into())]);

        let signature =
            futures::executor::block_on(client(&mock, 2).send_transaction([1, 2, 3, 4].into()))
                .unwrap();

        assert_eq!(signature, "5N7signature");
    }

    #[test]
    fn signature_status_is_the_verdict_two_providers_support() {
        let finalized = json!({
            "context": {"slot": 321},
            "value": [{"slot": 320, "confirmations": null, "confirmationStatus": "finalized", "err": null}]
        });
        let confirmed = json!({
            "context": {"slot": 321},
            "value": [{"slot": 320, "confirmations": 5, "confirmationStatus": "confirmed", "err": null}]
        });
        let unknown = json!({"context": {"slot": 321}, "value": [null]});

        let mock = MockHttpOutcall::new(vec![result(finalized.clone()), result(finalized.clone())]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_signature_status("sig")),
            Ok(SolTxStatus::Finalized)
        );
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE); 2]);

        // one provider is still catching up: landed, not yet final
        let mock = MockHttpOutcall::new(vec![result(finalized.clone()), result(confirmed)]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_signature_status("sig")),
            Ok(SolTxStatus::Landed)
        );

        // one provider has it and one does not: it exists somewhere
        let mock = MockHttpOutcall::new(vec![result(finalized), result(unknown.clone())]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_signature_status("sig")),
            Ok(SolTxStatus::Landed)
        );

        let mock = MockHttpOutcall::new(vec![result(unknown.clone()), result(unknown)]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_signature_status("sig")),
            Ok(SolTxStatus::Unknown)
        );

        let empty = json!({"context": {"slot": 321}, "value": []});
        let mock = MockHttpOutcall::new(vec![result(empty.clone()), result(empty)]);
        let err =
            futures::executor::block_on(client(&mock, 2).get_signature_status("sig")).unwrap_err();
        assert!(err.contains("missing signature status"), "{err}");
    }

    #[test]
    fn expiry_uses_the_hash_context_and_history_not_an_advertised_height() {
        let validity = SolValidity {
            blockhash: "hash".into(),
            context_slot: 100,
            last_valid_block_height: 1,
        };
        let unknown = json!({"context":{"slot":300},"value":[null]});
        let expired = json!({"context":{"slot":300},"value":false});
        let mock = MockHttpOutcall::new(vec![
            result(expired.clone()),
            result(json!(50)),
            result(unknown.clone()),
            result(expired.clone()),
            result(json!(50)),
            result(unknown.clone()),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).expired("sig", &validity)),
            Ok(true)
        );
        assert_eq!(
            mock.methods(),
            vec![
                "isBlockhashValid",
                "getFirstAvailableBlock",
                "getSignatureStatuses",
                "isBlockhashValid",
                "getFirstAvailableBlock",
                "getSignatureStatuses"
            ]
        );
        let mock = MockHttpOutcall::new(vec![result(json!({"context":{"slot":300},"value":true}))]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).expired("sig", &validity)),
            Ok(false)
        );
        let mock = MockHttpOutcall::new(vec![result(json!({"context":{"slot":99},"value":false}))]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).expired("sig", &validity)),
            Ok(false)
        );
        let mock = MockHttpOutcall::new(vec![
            result(expired.clone()),
            result(json!(101)),
            result(expired),
            result(json!(101)),
        ]);
        assert!(futures::executor::block_on(client(&mock, 2).expired("sig", &validity)).is_err());
    }

    #[test]
    fn ata_existence_does_not_depend_on_a_mutable_balance() {
        let mock = MockHttpOutcall::new(vec![
            result(json!({"context":{"slot":1},"value":{"balance":10}})),
            result(json!({"context":{"slot":2},"value":{"balance":11}})),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).account_exists("ata")),
            Ok(true)
        );
    }

    #[test]
    fn a_signature_batch_uses_only_two_outcalls() {
        let response = json!({"context":{"slot":1},"value":[null,null,null]});
        let mock = MockHttpOutcall::new(vec![result(response.clone()), result(response)]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_signature_statuses(&[
                "a".into(),
                "b".into(),
                "c".into()
            ])),
            Ok(vec![SolTxStatus::Unknown; 3])
        );
        assert_eq!(mock.urls().len(), 2);
    }

    #[test]
    fn balances_take_the_lower_view() {
        let mock = MockHttpOutcall::new(vec![
            result(json!({"context": {"slot": 1}, "value": 5000})),
            result(json!({"context": {"slot": 1}, "value": 4000})),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_balance("pk")),
            Ok(4000)
        );

        let mock = MockHttpOutcall::new(vec![
            result(
                json!({"context": {"slot": 1}, "value": {"amount": "12", "decimals": 8, "uiAmount": 1.2e-7}}),
            ),
            result(
                json!({"context": {"slot": 1}, "value": {"amount": "10", "decimals": 8, "uiAmount": 1.0e-7}}),
            ),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).get_token_account_balance("ata")),
            Ok(10)
        );
    }
}
