use ic_auth_types::ByteBufB64;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::types::*;
use crate::outcall::{HttpOutcall, LARGE_RESPONSE, Replication, SMALL_RESPONSE, json_rpc_call};

/// Commitment every request is made at. `confirmed` is the level a blockhash is
/// fetched and a transaction is sent at; finality is checked separately against
/// the `confirmationStatus` a signature status reports.
const COMMITMENT: &str = "confirmed";

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

impl<H: HttpOutcall> SvmClient<H> {
    pub fn new(providers: Vec<String>, outcall: H) -> Self {
        Self { providers, outcall }
    }

    pub async fn get_latest_blockhash(&self, now_ms: u64) -> Result<Hash, String> {
        let res: RpcContextValue<LatestBlockhash> = self
            .call(
                now_ms,
                "getLatestBlockhash",
                &[json!({ "commitment": COMMITMENT })],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;

        res.value.to_hash()
    }

    pub async fn get_signature_statuses(
        &self,
        now_ms: u64,
        signature: &str,
    ) -> Result<Option<SignatureStatus>, String> {
        let res: RpcContextValue<Vec<Option<SignatureStatus>>> = self
            .call(
                now_ms,
                "getSignatureStatuses",
                &[
                    Value::Array(vec![signature.into()]),
                    json!({ "searchTransactionHistory": true }),
                ],
                SMALL_RESPONSE,
                Replication::Single,
            )
            .await?;
        res.value
            .into_iter()
            .next()
            .ok_or_else(|| "missing signature status".to_string())
    }

    /// Preflight is always skipped: the canister signs complete transactions
    /// and polls their status itself, and a preflight simulation would only add
    /// a second chance for a provider to reject a transaction it has already
    /// been handed.
    pub async fn send_transaction(
        &self,
        now_ms: u64,
        transaction: ByteBufB64,
    ) -> Result<String, String> {
        self.call(
            now_ms,
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
            Replication::Single,
        )
        .await
    }

    pub async fn get_account_info(
        &self,
        now_ms: u64,
        pubkey: &str,
    ) -> Result<Option<UiAccount>, String> {
        let res: RpcContextValue<Option<UiAccount>> = self
            .call(
                now_ms,
                "getAccountInfo",
                &[
                    Value::String(pubkey.to_string()),
                    json!({ "commitment": COMMITMENT, "encoding": "jsonParsed" }),
                ],
                LARGE_RESPONSE,
                Replication::Single,
            )
            .await?;

        Ok(res.value)
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        now_ms: u64,
        method: &str,
        params: &[Value],
        max_response_bytes: u64,
        replication: Replication,
    ) -> Result<T, String> {
        json_rpc_call(
            &self.outcall,
            &self.providers,
            now_ms,
            method,
            params,
            max_response_bytes,
            replication,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcall::tests::{MockHttpOutcall, success_response};

    #[test]
    fn test_get_latest_blockhash() {
        let mock = MockHttpOutcall::new(vec![success_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "context": { "slot": 1234 },
                "value": {
                    "blockhash": "3Xdj6drp4pKAM9PH2vZ4w8NHygd8Epp7FKCvzX29VLLH",
                    "lastValidBlockHeight": 355385114
                }
            }
        }))]);

        let client = SvmClient::new(vec!["https://solana.rpc".to_string()], mock.clone());
        let response = futures::executor::block_on(client.get_latest_blockhash(1000)).unwrap();

        assert_eq!(
            response.to_string(),
            "3Xdj6drp4pKAM9PH2vZ4w8NHygd8Epp7FKCvzX29VLLH"
        );
        assert_eq!(mock.urls(), vec!["https://solana.rpc".to_string()]);
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE)]);
    }

    #[test]
    fn test_http_request_fallbacks_between_providers() {
        let mock = MockHttpOutcall::new(vec![
            Err("timeout".to_string()),
            success_response(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": { "slot": 1234 },
                    "value": {
                        "blockhash": "3Xdj6drp4pKAM9PH2vZ4w8NHygd8Epp7FKCvzX29VLLH",
                        "lastValidBlockHeight": 355385114
                    }
                }
            })),
        ]);

        let client = SvmClient::new(
            vec!["https://first".to_string(), "https://second".to_string()],
            mock.clone(),
        );

        futures::executor::block_on(client.get_latest_blockhash(2_000)).unwrap();

        assert_eq!(
            mock.urls(),
            vec!["https://first".to_string(), "https://second".to_string()]
        );
    }

    #[test]
    fn test_call_handles_error_payload() {
        let mock = MockHttpOutcall::new(vec![success_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32000, "message": "custom error"}
        }))]);
        let client = SvmClient::new(vec!["https://sol".to_string()], mock);

        let result: Result<Value, _> = futures::executor::block_on(client.call(
            1,
            "method",
            &[],
            SMALL_RESPONSE,
            Replication::Single,
        ));
        assert!(result.unwrap_err().contains("custom error"));
    }

    #[test]
    fn test_send_transaction_returns_signature() {
        let mock = MockHttpOutcall::new(vec![success_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "5N7signature"
        }))]);

        let client = SvmClient::new(vec!["https://sol".to_string()], mock);
        let signature =
            futures::executor::block_on(client.send_transaction(1_234, [1, 2, 3, 4].into()))
                .unwrap();

        assert_eq!(signature, "5N7signature");
    }

    #[test]
    fn test_get_signature_statuses_rejects_empty_status_array() {
        let mock = MockHttpOutcall::new(vec![success_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "context": {"slot": 321},
                "value": []
            }
        }))]);

        let client = SvmClient::new(vec!["https://sol".to_string()], mock);
        let err = futures::executor::block_on(client.get_signature_statuses(1_111, "signature"))
            .unwrap_err();

        assert!(err.contains("missing signature status"));
    }

    #[test]
    fn test_get_signature_statuses_returns_status() {
        let mock = MockHttpOutcall::new(vec![success_response(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "context": {"slot": 321},
                "value": [{
                    "slot": 320,
                    "confirmations": null,
                    "confirmationStatus": "finalized",
                    "err": null
                }]
            }
        }))]);

        let client = SvmClient::new(vec!["https://sol".to_string()], mock.clone());
        let status = futures::executor::block_on(client.get_signature_statuses(1_111, "signature"))
            .unwrap()
            .unwrap();

        assert!(status.is_finalized());
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE)]);
    }
}
