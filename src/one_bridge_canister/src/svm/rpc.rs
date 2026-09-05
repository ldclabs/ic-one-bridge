use ic_auth_types::ByteBufB64;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::types::*;
use crate::outcall::{
    Agreement, HttpOutcall, LARGE_RESPONSE, RpcCall, SMALL_RESPONSE, as_is, json_rpc_call, lower,
    same,
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

    /// A recent blockhash and the last block height it is valid at. The first
    /// provider's answer is used: a bad one only yields a transaction that
    /// never lands, which the expiry check recovers from.
    pub async fn get_latest_blockhash(&self) -> Result<LatestBlockhash, String> {
        self.call(
            "getLatestBlockhash",
            &[json!({ "commitment": COMMITMENT })],
            SMALL_RESPONSE,
            |res: RpcContextValue<LatestBlockhash>| Ok(res.value),
            Agreement::First,
        )
        .await
    }

    /// The status of a transaction, as two providers support it together; see
    /// [`SolTxStatus::reconcile`].
    pub async fn get_signature_status(&self, signature: &str) -> Result<SolTxStatus, String> {
        self.call(
            "getSignatureStatuses",
            &[
                Value::Array(vec![signature.into()]),
                json!({ "searchTransactionHistory": true }),
            ],
            SMALL_RESPONSE,
            |res: RpcContextValue<Vec<Option<SignatureStatus>>>| {
                res.value
                    .into_iter()
                    .next()
                    .map(SolTxStatus::from_signature_status)
                    .ok_or_else(|| "missing signature status".to_string())
            },
            Agreement::Two(SolTxStatus::reconcile),
        )
        .await
    }

    /// Whether a transaction whose blockhash was valid until
    /// `last_valid_block_height` can no longer land: the finalized block height
    /// is past it by `margin`, yet the signature is unknown.
    ///
    /// Both facts are read from the same provider, one provider at a time —
    /// a provider whose finalized height is past the deadline has the
    /// transaction if it landed — and two providers have to reach the verdict.
    pub async fn expired(
        &self,
        signature: &str,
        last_valid_block_height: u64,
        margin: u64,
    ) -> Result<bool, String> {
        let deadline = last_valid_block_height.saturating_add(margin);
        let mut verdicts: Vec<bool> = Vec::with_capacity(2);
        let mut last_err = "no provider answered".to_string();
        for provider in &self.providers {
            let one = std::slice::from_ref(provider);
            let verdict = async {
                let height: u64 = json_rpc_call(
                    &self.outcall,
                    one,
                    RpcCall {
                        method: "getBlockHeight",
                        params: &[json!({ "commitment": FINALIZED })],
                        max_response_bytes: SMALL_RESPONSE,
                    },
                    as_is,
                    Agreement::First,
                )
                .await?;
                if height <= deadline {
                    return Ok::<bool, String>(false);
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
            }
            .await;
            match verdict {
                Ok(verdict) => {
                    verdicts.push(verdict);
                    if verdicts.len() == 2 {
                        break;
                    }
                }
                Err(err) => last_err = err,
            }
        }
        match verdicts.as_slice() {
            [a, b] => Ok(*a && *b),
            _ => Err(format!(
                "only {} provider(s) answered the expiry check; last failure: {last_err}",
                verdicts.len()
            )),
        }
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

    pub async fn get_account_info(&self, pubkey: &str) -> Result<Option<UiAccount>, String> {
        self.call(
            "getAccountInfo",
            &[
                Value::String(pubkey.to_string()),
                json!({ "commitment": COMMITMENT, "encoding": "jsonParsed" }),
            ],
            LARGE_RESPONSE,
            |res: RpcContextValue<Option<UiAccount>>| Ok(res.value),
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
        let mock = MockHttpOutcall::new(vec![result(blockhash_json())]);

        let response =
            futures::executor::block_on(client(&mock, 2).get_latest_blockhash()).unwrap();

        assert_eq!(
            response.to_hash().unwrap().to_string(),
            "3Xdj6drp4pKAM9PH2vZ4w8NHygd8Epp7FKCvzX29VLLH"
        );
        assert_eq!(response.last_valid_block_height, 355385114);
        assert_eq!(mock.urls(), vec!["https://sol0".to_string()]);
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE)]);
    }

    #[test]
    fn test_http_request_fallbacks_between_providers() {
        let mock = MockHttpOutcall::new(vec![Err("timeout".to_string()), result(blockhash_json())]);

        futures::executor::block_on(client(&mock, 2).get_latest_blockhash()).unwrap();

        assert_eq!(
            mock.urls(),
            vec!["https://sol0".to_string(), "https://sol1".to_string()]
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
    fn an_expiry_verdict_needs_two_providers_each_past_the_deadline_without_the_signature() {
        let unknown = json!({"context": {"slot": 1}, "value": [null]});
        let landed = json!({
            "context": {"slot": 1},
            "value": [{"slot": 1, "confirmations": 3, "confirmationStatus": "confirmed", "err": null}]
        });

        let mock = MockHttpOutcall::new(vec![
            result(150.into()),
            result(unknown.clone()),
            result(151.into()),
            result(unknown.clone()),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).expired("sig", 100, 32)),
            Ok(true)
        );
        assert_eq!(
            mock.methods(),
            vec![
                "getBlockHeight",
                "getSignatureStatuses",
                "getBlockHeight",
                "getSignatureStatuses"
            ]
        );

        // not past the deadline plus margin yet
        let mock = MockHttpOutcall::new(vec![result(132.into()), result(132.into())]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).expired("sig", 100, 32)),
            Ok(false)
        );

        // a provider that has the transaction: it landed in time
        let mock = MockHttpOutcall::new(vec![
            result(150.into()),
            result(landed),
            result(150.into()),
            result(unknown),
        ]);
        assert_eq!(
            futures::executor::block_on(client(&mock, 2).expired("sig", 100, 32)),
            Ok(false)
        );
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
