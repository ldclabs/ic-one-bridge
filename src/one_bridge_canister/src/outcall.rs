use http::Uri;
use ic_cdk_management_canister::{
    HttpHeader, HttpMethod, HttpRequestArgs, HttpRequestResult, http_request,
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
/// transaction receipt (logs plus a 512-byte bloom filter), a block header or
/// a parsed Solana account. See [`SMALL_RESPONSE`] for why the budget exists.
pub const LARGE_RESPONSE: u64 = 32 * 1024;

/// How much of a provider's answer an error message quotes. Answers are up to
/// [`LARGE_RESPONSE`] long and the message ends up in the pending queue, the
/// archive and users' error strings.
const ERROR_BODY_EXCERPT: usize = 200;

/// # Trust model
///
/// Every outcall is made by a single replica (`is_replicated: false`): a
/// replicated call costs two orders of magnitude more, and would still trust
/// whichever provider answered it. Instead, an answer that a payout depends on
/// — a receipt, a block height, a nonce, a signature status, a balance — is
/// asked of two providers and only acted on when their answers agree, see
/// [`Agreement`]. One faulty replica or one faulty provider can then delay
/// the bridge, but cannot make it pay for a deposit that never happened.
///
/// A broadcast, a gas price and a recent blockhash are asked of one provider:
/// the worst a wrong answer does is a transaction that never lands, which the
/// finalization rounds detect and recover from.
pub trait HttpOutcall {
    async fn request(&self, args: &HttpRequestArgs) -> Result<HttpRequestResult, String>;
}

pub struct DefaultHttpOutcall;

impl HttpOutcall for DefaultHttpOutcall {
    async fn request(&self, args: &HttpRequestArgs) -> Result<HttpRequestResult, String> {
        http_request(args).await.map_err(|err| format!("{err}"))
    }
}

/// A JSON-RPC request and the response budget reserved for its answer.
pub struct RpcCall<'a> {
    pub method: &'a str,
    pub params: &'a [Value],
    pub max_response_bytes: u64,
}

/// How many providers have to answer a call, and how their answers become the
/// one that is acted on.
pub enum Agreement<T> {
    /// The first provider to answer is believed.
    First,
    /// Two providers have to answer, and the function turns their two answers
    /// into one — the same value, the more conservative of the two, or an
    /// error when they cannot be reconciled.
    Two(fn(T, T) -> Result<T, String>),
}

/// Two answers agree only when they are identical.
pub fn same<T: PartialEq + std::fmt::Debug>(a: T, b: T) -> Result<T, String> {
    if a == b {
        Ok(a)
    } else {
        Err(format!("the providers disagree: {a:?} and {b:?}"))
    }
}

/// Two answers are reconciled to the lower one: the fewer confirmations, the
/// lower balance, the earlier block.
pub fn lower<T: Ord>(a: T, b: T) -> Result<T, String> {
    Ok(a.min(b))
}

/// The decoded result, as it is.
pub fn as_is<T>(value: T) -> Result<T, String> {
    Ok(value)
}

/// Sends a JSON-RPC request to the providers in turn until enough of them have
/// answered, decoding each `result` with `interpret`, and returns the answer
/// `agreement` makes of them.
///
/// A transport failure, a non-2xx status, a body that is not a JSON-RPC
/// response and a result `interpret` rejects all move on to the next
/// provider. A well-formed JSON-RPC error does not: it is the chain's answer,
/// not a provider fault, and re-asking every other provider would just pay
/// for the same answer again.
pub async fn json_rpc_call<H: HttpOutcall, R: DeserializeOwned, T>(
    outcall: &H,
    providers: &[String],
    call: RpcCall<'_>,
    interpret: impl Fn(R) -> Result<T, String>,
    agreement: Agreement<T>,
) -> Result<T, String> {
    let needed = match agreement {
        Agreement::First => 1,
        Agreement::Two(_) => 2,
    };
    if providers.len() < needed {
        return Err(format!(
            "{} provider(s) configured, {needed} must answer",
            providers.len()
        ));
    }

    let body = serde_json::to_vec(&RPCRequest {
        jsonrpc: "2.0",
        method: call.method,
        params: call.params,
        id: 1,
    })
    .map_err(|err| err.to_string())?;
    let mut args = HttpRequestArgs {
        url: String::new(),
        max_response_bytes: Some(call.max_response_bytes),
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
        ],
        body: Some(body),
        transform: None,
        is_replicated: Some(false),
    };

    let mut answers: Vec<T> = Vec::with_capacity(needed);
    let mut last_err = "no provider answered".to_string();
    for p in providers {
        args.url = p.clone();
        let host = provider_host(p);
        let res = match outcall.request(&args).await {
            Ok(res) => res,
            Err(err) => {
                last_err = format!("provider {host} is unreachable: {err}");
                continue;
            }
        };
        if res.status < 200u64 || res.status >= 300u64 {
            last_err = format!(
                "provider {host} answered with status {}: {}",
                res.status,
                excerpt(&res.body)
            );
            continue;
        }

        let answer = match serde_json::from_slice::<RPCResponse<R>>(&res.body) {
            Ok(RPCResponse {
                error: Some(error), ..
            }) => {
                return Err(serde_json::to_string(&error).map_err(|err| err.to_string())?);
            }
            Ok(RPCResponse {
                result: Some(result),
                ..
            }) => interpret(result),
            // `null` is a valid answer for an optional result, such as a
            // receipt that does not exist yet; for anything else the provider
            // has not answered the question.
            Ok(RPCResponse { result: None, .. }) => serde_json::from_value::<R>(Value::Null)
                .map_err(|_| "neither a result nor an error".to_string())
                .and_then(&interpret),
            Err(err) => Err(format!(
                "an undecodable body: {err}, body: {}",
                excerpt(&res.body)
            )),
        };
        match answer {
            Ok(value) => {
                answers.push(value);
                if answers.len() == needed {
                    break;
                }
            }
            Err(err) => last_err = format!("provider {host} answered with {err}"),
        }
    }

    if answers.len() < needed {
        return Err(if answers.is_empty() {
            last_err
        } else {
            format!(
                "only {} of the {needed} answers needed came back; last failure: {last_err}",
                answers.len()
            )
        });
    }
    match agreement {
        Agreement::First => Ok(answers.pop().expect("one answer")),
        Agreement::Two(reconcile) => {
            let b = answers.pop().expect("two answers");
            let a = answers.pop().expect("two answers");
            reconcile(a, b)
        }
    }
}

/// The host of a provider URL: the part that identifies the provider without
/// the path and query, which may carry an API key.
fn provider_host(url: &str) -> String {
    url.parse::<Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_string))
        .unwrap_or_else(|| "<invalid url>".to_string())
}

fn excerpt(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut out: String = text.chars().take(ERROR_BODY_EXCERPT).collect();
    if text.chars().nth(ERROR_BODY_EXCERPT).is_some() {
        out.push('…');
    }
    out
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

        /// The JSON-RPC methods that were called, in order.
        pub fn methods(&self) -> Vec<String> {
            self.requests()
                .into_iter()
                .map(|args| {
                    let body: Value = serde_json::from_slice(&args.body.unwrap()).unwrap();
                    body["method"].as_str().unwrap().to_string()
                })
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
    }

    pub fn success_response(body: serde_json::Value) -> Result<HttpRequestResult, String> {
        Ok(HttpRequestResult {
            status: 200u64.into(),
            body: serde_json::to_vec(&body).unwrap(),
            headers: vec![],
        })
    }

    /// A successful JSON-RPC response carrying `result`.
    pub fn result(result: serde_json::Value) -> Result<HttpRequestResult, String> {
        success_response(serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": result}))
    }

    fn raw_response(body: &str) -> Result<HttpRequestResult, String> {
        Ok(HttpRequestResult {
            status: 200u64.into(),
            body: body.as_bytes().to_vec(),
            headers: vec![],
        })
    }

    fn providers(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("https://rpc{i}.example/v1/secret-key-{i}"))
            .collect()
    }

    fn call<T>(
        mock: &MockHttpOutcall,
        providers: &[String],
        agreement: Agreement<T>,
    ) -> Result<T, String>
    where
        T: DeserializeOwned,
    {
        futures::executor::block_on(json_rpc_call(
            mock,
            providers,
            RpcCall {
                method: "eth_blockNumber",
                params: &[],
                max_response_bytes: SMALL_RESPONSE,
            },
            as_is,
            agreement,
        ))
    }

    /// Every outcall must cap the response it reserves; an uncapped one silently
    /// reserves — and pays for — 2 MB.
    #[test]
    fn every_request_reserves_a_bounded_response_from_a_single_replica() {
        let mock = MockHttpOutcall::new(vec![result("0x1".into())]);

        let value: String = call(&mock, &providers(1), Agreement::First).unwrap();

        assert_eq!(value, "0x1");
        assert_eq!(mock.max_response_bytes(), vec![Some(SMALL_RESPONSE)]);
        let request = mock.requests().remove(0);
        assert_eq!(request.is_replicated, Some(false));
        assert!(request.transform.is_none());
    }

    #[test]
    fn undecodable_answers_fail_over_to_the_next_provider() {
        // A proxy's HTML error page, a `null` where a value is required, a
        // value of the wrong shape and one `interpret` rejects are all
        // answers to a different question.
        let mock = MockHttpOutcall::new(vec![
            raw_response("<html>502 Bad Gateway</html>"),
            result(Value::Null),
            result(42.into()),
            result("not hex".into()),
            result("0x2a".into()),
        ]);

        let value = futures::executor::block_on(json_rpc_call(
            &mock,
            &providers(5),
            RpcCall {
                method: "eth_blockNumber",
                params: &[],
                max_response_bytes: SMALL_RESPONSE,
            },
            |hex: String| {
                u64::from_str_radix(hex.trim_start_matches("0x"), 16).map_err(|e| e.to_string())
            },
            Agreement::First,
        ))
        .unwrap();

        assert_eq!(value, 42);
        assert_eq!(mock.urls().len(), 5);
    }

    #[test]
    fn a_null_result_is_an_answer_when_the_result_is_optional() {
        let mock = MockHttpOutcall::new(vec![result(Value::Null)]);

        let value: Option<String> = call(&mock, &providers(2), Agreement::First).unwrap();

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
            result("0x1".into()),
        ]);

        let err = call::<String>(&mock, &providers(2), Agreement::First).unwrap_err();

        assert!(err.contains("execution reverted"));
        assert_eq!(mock.urls().len(), 1);
    }

    #[test]
    fn two_providers_have_to_answer_before_an_agreed_value_is_used() {
        let mock = MockHttpOutcall::new(vec![
            Err("timeout".to_string()),
            result("0x10".into()),
            result("0x10".into()),
        ]);

        let value: String = call(&mock, &providers(3), Agreement::Two(same)).unwrap();

        assert_eq!(value, "0x10");
        assert_eq!(mock.urls().len(), 3);

        // one answer is not enough, however good it looks
        let mock = MockHttpOutcall::new(vec![result("0x10".into()), Err("timeout".to_string())]);
        let err = call::<String>(&mock, &providers(2), Agreement::Two(same)).unwrap_err();
        assert!(err.contains("only 1 of the 2"), "{err}");

        // and a single configured provider can never reach agreement
        let err = call::<String>(&mock, &providers(1), Agreement::Two(same)).unwrap_err();
        assert!(err.contains("1 provider(s) configured"), "{err}");
    }

    #[test]
    fn disagreeing_answers_are_reconciled_or_rejected() {
        let mock = MockHttpOutcall::new(vec![result(7.into()), result(9.into())]);
        assert_eq!(
            call::<u64>(&mock, &providers(2), Agreement::Two(lower)),
            Ok(7)
        );

        let mock = MockHttpOutcall::new(vec![result(7.into()), result(9.into())]);
        let err = call::<u64>(&mock, &providers(2), Agreement::Two(same)).unwrap_err();
        assert!(err.contains("disagree"), "{err}");
    }

    #[test]
    fn errors_name_the_provider_host_only_and_quote_a_short_excerpt() {
        let long_body = "x".repeat(ERROR_BODY_EXCERPT + 50);
        let mock = MockHttpOutcall::new(vec![
            Err("timeout".to_string()),
            Ok(HttpRequestResult {
                status: 429u64.into(),
                body: long_body.into_bytes(),
                headers: vec![],
            }),
        ]);

        let err = call::<String>(&mock, &providers(2), Agreement::First).unwrap_err();

        assert!(err.contains("rpc1.example"), "{err}");
        assert!(!err.contains("secret-key"), "{err}");
        assert!(err.contains("429"), "{err}");
        assert!(err.len() < ERROR_BODY_EXCERPT + 100, "{err}");
        assert!(err.ends_with('…'), "{err}");
    }
}
