use alloy_primitives::{hex, keccak256};
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

/// How many replicas make an outcall.
///
/// A replicated request is made by every replica of the subnet and their
/// answers have to agree, so the response is as trustworthy as the subnet. A
/// single-replica request is made by one replica and its answer is used as is:
/// far cheaper, and exactly as trustworthy as that one replica. Every RPC
/// method states which it wants, so the choice is visible where the response
/// is acted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replication {
    Single,
    // No RPC method asks for it yet; the finality reads are the candidates.
    #[allow(dead_code)]
    Replicated,
}

pub trait HttpOutcall {
    /// The transform a replicated request is answered through. It has to strip
    /// whatever differs between replicas, such as dated headers, or the
    /// replicas' answers never agree.
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
/// `result` of the first one that answers.
///
/// A transport failure, a non-2xx status and a body that is not a JSON-RPC
/// response all move on to the next provider. A well-formed JSON-RPC error
/// does not: it is the chain's answer, not a provider fault, and re-asking
/// every other provider would just pay for the same answer again.
pub async fn json_rpc_call<H: HttpOutcall, T: DeserializeOwned>(
    outcall: &H,
    providers: &[String],
    now_ms: u64,
    method: &str,
    params: &[Value],
    max_response_bytes: u64,
    replication: Replication,
) -> Result<T, String> {
    if providers.is_empty() {
        return Err("no available provider".to_string());
    }

    let body = serde_json::to_vec(&RPCRequest {
        jsonrpc: "2.0",
        method,
        params,
        id: 1,
    })
    .map_err(|err| err.to_string())?;
    let mut args = request_args(
        outcall,
        method,
        now_ms,
        body,
        max_response_bytes,
        replication,
    );

    let mut last_err = "No provider succeeded".to_string();
    for p in providers {
        args.url = p.clone();
        let res = match outcall.request(&args).await {
            Ok(res) => res,
            Err(err) => {
                last_err = format!("failed to request provider: {p}, error: {err}");
                continue;
            }
        };
        if res.status < 200u64 || res.status >= 300u64 {
            last_err = format!(
                "request provider: {}, status: {}, body: {}",
                p,
                res.status,
                String::from_utf8_lossy(&res.body),
            );
            continue;
        }

        match serde_json::from_slice::<RPCResponse<T>>(&res.body) {
            Ok(RPCResponse {
                error: Some(error), ..
            }) => {
                return Err(serde_json::to_string(&error).map_err(|err| err.to_string())?);
            }
            Ok(RPCResponse {
                result: Some(result),
                ..
            }) => return Ok(result),
            Ok(RPCResponse { result: None, .. }) => {
                // `null` is a valid answer for an optional result, such as a
                // receipt that does not exist yet; for anything else the
                // provider has not answered the question.
                if let Ok(result) = serde_json::from_value(Value::Null) {
                    return Ok(result);
                }
                last_err = format!("provider {p} answered with neither a result nor an error");
            }
            Err(err) => {
                last_err = format!(
                    "provider {p} answered with an undecodable body: {err}, body: {}",
                    String::from_utf8_lossy(&res.body),
                );
            }
        }
    }

    Err(last_err)
}

fn request_args<H: HttpOutcall>(
    outcall: &H,
    method: &str,
    now_ms: u64,
    body: Vec<u8>,
    max_response_bytes: u64,
    replication: Replication,
) -> HttpRequestArgs {
    let mut headers = vec![
        HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        },
        HttpHeader {
            name: "user-agent".to_string(),
            value: APP_AGENT.to_string(),
        },
    ];
    let (transform, is_replicated) = match replication {
        Replication::Single => (None, false),
        Replication::Replicated => {
            // Every replica sends its own copy of the request, and the key is
            // what lets a provider recognise the copies as one request. It has
            // to differ between requests that differ in any way, so it covers
            // the whole body rather than just the method name.
            headers.push(HttpHeader {
                name: "idempotency-key".to_string(),
                value: idempotency_key(method, now_ms, &body),
            });
            (outcall.transform_context(), true)
        }
    };

    HttpRequestArgs {
        url: String::new(),
        max_response_bytes: Some(max_response_bytes),
        method: HttpMethod::POST,
        headers,
        body: Some(body),
        transform,
        is_replicated: Some(is_replicated),
    }
}

fn idempotency_key(method: &str, now_ms: u64, body: &[u8]) -> String {
    format!("{method}-{now_ms}-{}", hex::encode(&keccak256(body)[..8]))
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

    /// An [`HttpOutcall`] that replays canned responses and records the
    /// requests it was asked to make, shared by the EVM and Solana client tests.
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

        pub fn requests(&self) -> Vec<HttpRequestArgs> {
            self.requests.lock().unwrap().clone()
        }

        pub fn urls(&self) -> Vec<String> {
            self.requests().into_iter().map(|args| args.url).collect()
        }

        pub fn max_response_bytes(&self) -> Vec<Option<u64>> {
            self.requests()
                .into_iter()
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
            Some(TransformContext {
                function: TransformFunc::new(
                    Principal::anonymous(),
                    "inner_transform_response".to_string(),
                ),
                context: vec![],
            })
        }
    }

    pub fn success_response(body: serde_json::Value) -> Result<HttpRequestResult, String> {
        Ok(HttpRequestResult {
            status: 200u64.into(),
            body: serde_json::to_vec(&body).unwrap(),
            headers: vec![],
        })
    }

    fn raw_response(body: &str) -> Result<HttpRequestResult, String> {
        Ok(HttpRequestResult {
            status: 200u64.into(),
            body: body.as_bytes().to_vec(),
            headers: vec![],
        })
    }

    fn call<T: DeserializeOwned>(
        mock: &MockHttpOutcall,
        providers: &[&str],
        replication: Replication,
    ) -> Result<T, String> {
        let providers: Vec<String> = providers.iter().map(|p| p.to_string()).collect();
        futures::executor::block_on(json_rpc_call(
            mock,
            &providers,
            1_000,
            "eth_blockNumber",
            &[],
            SMALL_RESPONSE,
            replication,
        ))
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

        let value: String = call(&mock, &["https://rpc"], Replication::Single).unwrap();

        assert_eq!(value, "0x1");
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE)]);
    }

    #[test]
    fn undecodable_answers_fail_over_to_the_next_provider() {
        // A proxy's HTML error page, a `null` where a value is required, and a
        // value of the wrong shape are all answers to a different question.
        let mock = MockHttpOutcall::new(vec![
            raw_response("<html>502 Bad Gateway</html>"),
            success_response(serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": null})),
            success_response(serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": 42})),
            success_response(serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": "0x2a"})),
        ]);

        let value: String = call(
            &mock,
            &["https://a", "https://b", "https://c", "https://d"],
            Replication::Single,
        )
        .unwrap();

        assert_eq!(value, "0x2a");
        assert_eq!(mock.urls().len(), 4);
    }

    #[test]
    fn a_null_result_is_an_answer_when_the_result_is_optional() {
        let mock = MockHttpOutcall::new(vec![success_response(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": null
        }))]);

        let value: Option<String> =
            call(&mock, &["https://a", "https://b"], Replication::Single).unwrap();

        assert_eq!(value, None);
        assert_eq!(mock.urls().len(), 1);
    }

    #[test]
    fn json_rpc_errors_do_not_fail_over() {
        let mock = MockHttpOutcall::new(vec![
            success_response(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32000, "message": "execution reverted"}
            })),
            success_response(serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": "0x1"})),
        ]);

        let err =
            call::<String>(&mock, &["https://a", "https://b"], Replication::Single).unwrap_err();

        assert!(err.contains("execution reverted"));
        assert_eq!(mock.urls(), vec!["https://a".to_string()]);
    }

    #[test]
    fn every_provider_failing_reports_the_last_failure() {
        let mock = MockHttpOutcall::new(vec![
            Err("timeout".to_string()),
            Ok(HttpRequestResult {
                status: 429u64.into(),
                body: b"slow down".to_vec(),
                headers: vec![],
            }),
        ]);

        let err =
            call::<String>(&mock, &["https://a", "https://b"], Replication::Single).unwrap_err();

        assert!(err.contains("https://b"));
        assert!(err.contains("429"));
        assert!(err.contains("slow down"));
    }

    #[test]
    fn single_replica_requests_carry_no_consensus_plumbing() {
        let mock = MockHttpOutcall::new(vec![success_response(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x1"
        }))]);

        call::<String>(&mock, &["https://rpc"], Replication::Single).unwrap();

        let request = mock.requests().remove(0);
        assert_eq!(request.is_replicated, Some(false));
        assert!(request.transform.is_none());
        assert!(
            !request
                .headers
                .iter()
                .any(|header| header.name == "idempotency-key")
        );
    }

    #[test]
    fn replicated_requests_carry_a_transform_and_a_body_specific_idempotency_key() {
        let mock = MockHttpOutcall::new(vec![success_response(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x1"
        }))]);

        call::<String>(&mock, &["https://rpc"], Replication::Replicated).unwrap();

        let request = mock.requests().remove(0);
        assert_eq!(request.is_replicated, Some(true));
        assert!(request.transform.is_some());
        let key = request
            .headers
            .iter()
            .find(|header| header.name == "idempotency-key")
            .map(|header| header.value.clone())
            .expect("a replicated request names its idempotency key");
        assert_eq!(
            key,
            idempotency_key("eth_blockNumber", 1_000, request.body.as_deref().unwrap())
        );

        // requests that differ only in their parameters must not share a key
        assert_ne!(
            idempotency_key("m", 1, b"[1]"),
            idempotency_key("m", 1, b"[2]")
        );
    }
}
