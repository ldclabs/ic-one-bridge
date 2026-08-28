use candid::Principal;
use ic_cdk_management_canister::{
    HttpHeader, HttpMethod, HttpRequestArgs, HttpRequestResult, TransformArgs, TransformContext,
    TransformFunc, http_request,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    helper::APP_AGENT,
    types::{RPCRequest, RPCResponse},
};

/// Response budget for a JSON-RPC call that returns a scalar or a small object.
///
/// An outcall is billed on `max_response_bytes` — the bytes it *reserves*, not
/// the bytes that come back — and leaving it unset reserves the 2 MB maximum.
/// These calls are non-replicated (`is_replicated: false`), which prices the
/// reservation at ~800 cycles per byte on a 13-node subnet: ~1.7B cycles a
/// call left unset, against ~10M for the budget below. So every method names
/// one.
///
/// The budgets are deliberately far larger than the few hundred bytes of JSON
/// these calls answer with. A response that overruns its budget is rejected
/// outright, and the limit counts the provider's headers as well as the body, so
/// a budget shaved close to the payload would take the bridge down the day a
/// provider started sending fatter headers. Past the base fee the difference
/// between a tight budget and a roomy one is small; the difference between a
/// roomy one and none at all is two orders of magnitude.
pub const SMALL_RESPONSE: u64 = 8 * 1024;

/// Response budget for a JSON-RPC call that returns a document: an EVM
/// transaction receipt (logs plus a 512-byte bloom filter) or a parsed Solana
/// account. See [`SMALL_RESPONSE`] for why the budget exists.
pub const LARGE_RESPONSE: u64 = 32 * 1024;

pub trait HttpOutcall {
    fn transform_context(&self) -> Option<TransformContext>;
    async fn request(&self, args: &HttpRequestArgs) -> Result<HttpRequestResult, String>;
}

pub struct DefaultHttpOutcall(Principal);

impl DefaultHttpOutcall {
    pub fn new(canister_id: Principal) -> Self {
        Self(canister_id)
    }
}

impl HttpOutcall for DefaultHttpOutcall {
    async fn request(&self, args: &HttpRequestArgs) -> Result<HttpRequestResult, String> {
        http_request(args).await.map_err(|err| format!("{err}"))
    }

    fn transform_context(&self) -> Option<TransformContext> {
        Some(TransformContext {
            function: TransformFunc::new(self.0, "inner_transform_response".to_string()),
            context: vec![],
        })
    }
}

/// Sends a JSON-RPC request to the providers in turn, returning the decoded
/// `result` of the first one that answers with a 2xx status.
///
/// Only transport failures and non-2xx statuses move on to the next provider: a
/// well-formed JSON-RPC error is the chain's answer, not a provider fault, and
/// re-asking every other provider would just pay for the same answer again.
pub async fn json_rpc_call<H: HttpOutcall, T: DeserializeOwned>(
    outcall: &H,
    providers: &[String],
    idempotency_key: String,
    method: &str,
    params: &[Value],
    max_response_bytes: u64,
) -> Result<T, String> {
    let body = serde_json::to_vec(&RPCRequest {
        jsonrpc: "2.0",
        method,
        params,
        id: 1,
    })
    .map_err(|err| err.to_string())?;

    let data = request_providers(
        outcall,
        providers,
        idempotency_key,
        body,
        max_response_bytes,
    )
    .await?;

    let output: RPCResponse<T> = serde_json::from_slice(&data).map_err(|err| err.to_string())?;
    if let Some(error) = output.error {
        return Err(serde_json::to_string(&error).map_err(|err| err.to_string())?);
    }

    match output.result {
        Some(result) => Ok(result),
        None => serde_json::from_value(Value::Null).map_err(|_| "missing result".to_string()),
    }
}

async fn request_providers<H: HttpOutcall>(
    outcall: &H,
    providers: &[String],
    idempotency_key: String,
    body: Vec<u8>,
    max_response_bytes: u64,
) -> Result<Vec<u8>, String> {
    if providers.is_empty() {
        return Err("no available provider".to_string());
    }

    let mut args = HttpRequestArgs {
        url: String::new(),
        max_response_bytes: Some(max_response_bytes),
        method: HttpMethod::POST,
        headers: vec![
            HttpHeader {
                name: "content-type".to_string(),
                value: "application/json".to_string(),
            },
            HttpHeader {
                name: "user-agent".to_string(),
                value: APP_AGENT.to_string(),
            },
            HttpHeader {
                name: "idempotency-key".to_string(),
                value: idempotency_key.clone(),
            },
        ],
        body: Some(body),
        transform: outcall.transform_context(),
        is_replicated: Some(false),
    };

    let mut last_err = "No provider succeeded".to_string();
    for p in providers {
        args.url = p.clone();
        match outcall.request(&args).await {
            Ok(res) => {
                if res.status >= 200u64 && res.status < 300u64 {
                    return Ok(res.body);
                }
                last_err = format!(
                    "request provider: {}, idempotency-key: {}, status: {}, body: {}",
                    p,
                    idempotency_key,
                    res.status,
                    String::from_utf8_lossy(&res.body),
                );
            }
            Err(err) => {
                last_err = format!("failed to request provider: {p}, error: {err}");
            }
        }
    }

    Err(last_err)
}

#[ic_cdk::query(hidden = true)]
fn inner_transform_response(args: TransformArgs) -> HttpRequestResult {
    HttpRequestResult {
        status: args.response.status,
        body: args.response.body,
        // Remove headers (which may contain a timestamp) for consensus
        headers: vec![],
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    /// An [`HttpOutcall`] that replays canned responses and records the URLs it
    /// was asked for, shared by the EVM and Solana client tests.
    #[derive(Clone, Default)]
    pub struct MockHttpOutcall {
        responses: Arc<Mutex<VecDeque<Result<HttpRequestResult, String>>>>,
        requests: Arc<Mutex<Vec<HttpRequestArgs>>>,
    }

    impl MockHttpOutcall {
        pub fn new(responses: Vec<Result<HttpRequestResult, String>>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses.into_iter().collect())),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub fn urls(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|args| args.url.clone())
                .collect()
        }

        pub fn max_response_bytes(&self) -> Vec<Option<u64>> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|args| args.max_response_bytes)
                .collect()
        }
    }

    impl HttpOutcall for MockHttpOutcall {
        async fn request(&self, args: &HttpRequestArgs) -> Result<HttpRequestResult, String> {
            self.requests.lock().unwrap().push(args.clone());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("no mock response".to_string()))
        }

        fn transform_context(&self) -> Option<TransformContext> {
            None
        }
    }

    pub fn success_response(body: serde_json::Value) -> Result<HttpRequestResult, String> {
        Ok(HttpRequestResult {
            status: 200u64.into(),
            body: serde_json::to_vec(&body).unwrap(),
            headers: vec![],
        })
    }

    /// Every outcall must cap the response it reserves; an uncapped one silently
    /// reserves — and pays for — 2 MB.
    #[test]
    fn every_request_reserves_a_bounded_response() {
        let mock = MockHttpOutcall::new(vec![success_response(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x1"
        }))]);

        let value: String = futures::executor::block_on(json_rpc_call(
            &mock,
            &["https://rpc".to_string()],
            "key".to_string(),
            "eth_blockNumber",
            &[],
            SMALL_RESPONSE,
        ))
        .unwrap();

        assert_eq!(value, "0x1");
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE)]);
    }
}
