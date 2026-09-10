use http::Uri;
use ic_cdk_management_canister::{
    HttpHeader, HttpMethod, HttpRequestArgs, HttpRequestResult, http_request,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeSet;
use std::future::Future;

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

/// Compatibility budget for full block responses when a provider does not
/// implement the compact header RPC. Includes HTTP headers and stays within
/// the management canister's response limit.
pub const BLOCK_RESPONSE: u64 = 2_000_000;

/// How much of a provider's answer an error message quotes. Answers are up to
/// [`LARGE_RESPONSE`] long and the message ends up in the pending queue, the
/// archive and users' error strings.
const ERROR_BODY_EXCERPT: usize = 200;

/// # Trust model
///
/// Non-replicated requests are an explicit cost/trust choice. Controllers must
/// configure independent official providers. Financial evidence needs two
/// provider identities, but this does not authenticate an HTTPS response against
/// a malicious IC replica: the bridge also trusts the replicas serving outcalls.
/// A broadcast needs one answer. A recent blockhash candidate is independently
/// validated before use; its advertised expiry height is never a death proof.
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
pub fn same<T: PartialEq>(a: T, b: T) -> Result<T, String> {
    if a == b {
        Ok(a)
    } else {
        Err("the providers disagree".to_string())
    }
}

/// Two answers are reconciled to the lower one: the fewer confirmations, the
/// lower balance, the earlier block.
pub fn lower<T: Ord>(a: T, b: T) -> Result<T, String> {
    Ok(a.min(b))
}

/// Two fee quotes are reconciled to the higher one. A low quote can leave an
/// EIP-1559 transaction permanently below the chain's base fee, while the
/// configured transaction and hourly limits still bound the higher quote.
pub fn higher<T: Ord>(a: T, b: T) -> Result<T, String> {
    Ok(a.max(b))
}

/// The decoded result, as it is.
pub fn as_is<T>(value: T) -> Result<T, String> {
    Ok(value)
}

/// A verdict that two providers have to reach separately, each from its own
/// view alone.
///
/// `verdict` is handed one provider at a time and answers whether, as far as
/// that provider can see, the thing being asked about is true. Only a `true`
/// both of them reached counts, so one provider lagging behind the other
/// cannot carry a decision on its own. `what` names the check in the error a
/// shortage of answers produces.
///
/// The alternative — asking one question of two providers and the next
/// question of two others — is what this exists to avoid: a verdict that
/// combines a fact from a provider that has caught up with a fact from one
/// that has not is exactly the mixed view that reads a live transaction as
/// dead.
pub async fn two_provider_verdict<'a, F, Fut>(
    providers: &'a [String],
    what: &str,
    verdict: F,
) -> Result<bool, String>
where
    F: Fn(&'a [String]) -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    let mut answered = 0;
    let mut identities = BTreeSet::new();
    let mut last_err = "no provider answered".to_string();
    for provider in providers {
        if !identities.insert(provider_identity(provider)?) {
            continue;
        }
        match verdict(std::slice::from_ref(provider)).await {
            Ok(false) => return Ok(false),
            Ok(true) => {
                answered += 1;
                if answered == 2 {
                    return Ok(true);
                }
            }
            Err(err) => last_err = err,
        }
    }
    Err(public_error(
        &format!("only {answered} provider(s) answered the {what}; last failure: {last_err}"),
        providers,
    ))
}

/// Sends a JSON-RPC request to the providers in turn until enough of them have
/// answered, decoding each `result` with `interpret`, and returns the answer
/// `agreement` makes of them.
///
/// A transport failure, a non-2xx status, a body that is not a JSON-RPC
/// response and a result `interpret` rejects all move on to the next
/// provider. JSON-RPC errors also fail over: providers use them for rate limits,
/// node lag and unsupported methods as well as transaction rejection. Broadcasts
/// are idempotent signed transactions, so retrying the same bytes is safe.
pub async fn json_rpc_call<H: HttpOutcall, R: DeserializeOwned, T>(
    outcall: &H,
    providers: &[String],
    call: RpcCall<'_>,
    interpret: impl Fn(R) -> Result<T, String>,
    agreement: Agreement<T>,
) -> Result<T, String> {
    json_rpc_call_with_fallback(outcall, providers, call, None, interpret, agreement).await
}

/// A method-unsupported response may use a semantically equivalent RPC on the
/// same provider. The fallback is still only one vote from that identity.
pub async fn json_rpc_call_with_fallback<H: HttpOutcall, R: DeserializeOwned, T>(
    outcall: &H,
    providers: &[String],
    call: RpcCall<'_>,
    fallback: Option<RpcCall<'_>>,
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
    let mut identities = BTreeSet::new();
    let providers: Vec<String> = providers
        .iter()
        .filter_map(|url| {
            let id = provider_identity(url).ok()?;
            identities.insert(id).then(|| url.clone())
        })
        .collect();
    if providers.len() < needed {
        return Err(format!("{needed} independent HTTPS providers are required"));
    }

    let args = request_args(&call)?;
    let fallback_args = fallback.as_ref().map(request_args).transpose()?;

    let mut answers: Vec<T> = Vec::with_capacity(needed);
    let mut cursor = 0;
    let mut last_error = "no provider answered".to_string();
    let mut rpc_error = None;
    while answers.len() < needed && cursor < providers.len() {
        let count = (needed - answers.len()).min(providers.len() - cursor);
        // At most the missing votes are in flight: the common two-provider
        // path pays for exactly two requests and waits for them concurrently.
        let replies =
            futures::future::join_all(providers[cursor..cursor + count].iter().map(|url| {
                let mut args = args.clone();
                args.url = url.clone();
                let interpret = &interpret;
                let fallback_args = fallback_args.as_ref();
                async move {
                    match provider_reply(outcall, args, interpret).await {
                        Err(error) if error.method_unsupported => {
                            if let Some(fallback) = fallback_args {
                                let mut fallback = fallback.clone();
                                fallback.url = url.clone();
                                provider_reply(outcall, fallback, interpret).await
                            } else {
                                Err(error)
                            }
                        }
                        result => result,
                    }
                }
            }))
            .await;
        cursor += count;
        for reply in replies {
            match reply {
                Ok(value) => answers.push(value),
                Err(error) => {
                    last_error = public_error(&error.message, &providers);
                    if error.rpc && rpc_error.is_none() {
                        rpc_error = Some(last_error.clone());
                    }
                }
            }
        }
    }
    if answers.len() < needed {
        return Err(public_error(
            &if answers.is_empty() {
                rpc_error.unwrap_or(last_error)
            } else {
                format!(
                    "only {} of the {needed} answers needed came back; last failure: {last_error}",
                    answers.len()
                )
            },
            &providers,
        ));
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

fn request_args(call: &RpcCall<'_>) -> Result<HttpRequestArgs, String> {
    let body = serde_json::to_vec(&RPCRequest {
        jsonrpc: "2.0",
        method: call.method,
        params: call.params,
        id: 1,
    })
    .map_err(|err| err.to_string())?;
    Ok(HttpRequestArgs {
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
    })
}

struct ProviderError {
    message: String,
    rpc: bool,
    method_unsupported: bool,
}
async fn provider_reply<H: HttpOutcall, R: DeserializeOwned, T>(
    outcall: &H,
    args: HttpRequestArgs,
    interpret: &impl Fn(R) -> Result<T, String>,
) -> Result<T, ProviderError> {
    let host = provider_host(&args.url);
    let response = outcall
        .request(&args)
        .await
        .map_err(|error| ProviderError {
            message: format!("provider {host} is unreachable: {error}"),
            rpc: false,
            method_unsupported: false,
        })?;
    if response.status < 200u64 || response.status >= 300u64 {
        return Err(ProviderError {
            message: format!(
                "provider {host} answered with status {}: {}",
                response.status,
                excerpt(&response.body)
            ),
            rpc: false,
            method_unsupported: false,
        });
    }
    let answer = match serde_json::from_slice::<RPCResponse<R>>(&response.body) {
        Ok(RPCResponse {
            error: Some(error), ..
        }) => {
            return Err(ProviderError {
                message: format!(
                    "provider {host}: JSON-RPC {}: {}",
                    error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("invalid error response")
                ),
                rpc: true,
                method_unsupported: matches!(
                    error.get("code").and_then(Value::as_i64),
                    Some(-32601 | -32004)
                ),
            });
        }
        Ok(RPCResponse {
            result: Some(result),
            ..
        }) => interpret(result),
        Ok(RPCResponse { result: None, .. }) => serde_json::from_value::<R>(Value::Null)
            .map_err(|_| "neither a result nor an error".to_string())
            .and_then(interpret),
        Err(error) => Err(format!(
            "undecodable body: {error}, body: {}",
            excerpt(&response.body)
        )),
    };
    answer.map_err(|error| ProviderError {
        message: format!("provider {host} answered with {error}"),
        rpc: false,
        method_unsupported: false,
    })
}

/// The host of a provider URL: the part that identifies the provider without
/// the path and query, which may carry an API key.
pub fn provider_host(url: &str) -> String {
    url.parse::<Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_string))
        .unwrap_or_else(|| "<invalid url>".to_string())
}

/// Browser RPCs are explicitly public, or anonymous origins with no path/query.
/// Never turn a credential-bearing URL into a fabricated, nonworking endpoint.
pub fn browser_providers(private: &[String], public: Option<&Vec<String>>) -> Vec<String> {
    if let Some(public) = public {
        return public.clone();
    }
    private
        .iter()
        .filter(|url| {
            url.parse::<Uri>().is_ok_and(|uri| {
                uri.query().is_none()
                    && matches!(uri.path(), "" | "/")
                    && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
            })
        })
        .cloned()
        .collect()
}

/// URLs differing only in credentials, paths, port or known provider subdomains
/// do not provide independent evidence. Other hosts remain controller-approved.
pub fn provider_identity(url: &str) -> Result<String, String> {
    let uri: Uri = url
        .parse()
        .map_err(|_| "invalid provider URL".to_string())?;
    if uri.scheme_str() != Some("https") || uri.authority().is_none_or(|a| a.as_str().contains('@'))
    {
        return Err("providers must use HTTPS without user information".to_string());
    }
    let host = uri
        .host()
        .ok_or_else(|| "provider host is missing".to_string())?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err("provider host is missing".into());
    }
    for family in [
        "alchemy.com",
        "alchemyapi.io",
        "ankr.com",
        "nodereal.io",
        "publicnode.com",
        "bnbchain.org",
        "infura.io",
        "quiknode.pro",
        "quicknode.com",
        "solana.com",
    ] {
        if host == family || host.ends_with(&format!(".{family}")) {
            return Ok(match family {
                "alchemyapi.io" => "alchemy.com",
                "quiknode.pro" => "quicknode.com",
                other => other,
            }
            .to_string());
        }
    }
    Ok(host)
}

pub fn public_error(message: &str, providers: &[String]) -> String {
    let mut message = message.to_string();
    for url in providers {
        message = message.replace(url, &format!("https://{}", provider_host(url)));
        if let Ok(uri) = url.parse::<Uri>() {
            for secret in uri
                .path()
                .split('/')
                .chain(uri.query().unwrap_or_default().split(['&', '=']))
            {
                if secret.len() >= 8 {
                    message = message.replace(secret, "<redacted>");
                }
            }
        }
    }
    let mut text: String = message.chars().take(240).collect();
    if message.chars().count() > 240 {
        text.push('…');
    }
    text
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

    #[test]
    fn aliases_of_one_provider_cannot_supply_two_votes() {
        let mock = MockHttpOutcall::new(vec![]);
        let urls = vec![
            "https://rpc.example/key-one".into(),
            "https://RPC.example:443/key-two".into(),
        ];
        assert!(call::<u64>(&mock, &urls, Agreement::Two(lower)).is_err());
        assert!(mock.urls().is_empty());
    }

    #[test]
    fn browser_endpoints_preserve_anonymous_origins_but_never_guess_private_paths() {
        let urls = vec![
            "https://public.example".into(),
            "https://private.example/v3/secret-key".into(),
            "https://other.example?key=secret-key".into(),
        ];
        assert_eq!(
            browser_providers(&urls, None),
            vec!["https://public.example".to_string()]
        );
        let public = vec!["https://anonymous.example/eth".into()];
        assert_eq!(browser_providers(&urls, Some(&public)), public);
        assert!(
            !public_error(
                "failed https://private.example/v3/secret-key; secret-key",
                &urls
            )
            .contains("secret-key")
        );
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
    fn json_rpc_errors_fail_over_to_another_provider() {
        let mock = MockHttpOutcall::new(vec![
            success_response(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32000, "message": "execution reverted"}
            })),
            result("0x1".into()),
        ]);

        let value = call::<String>(&mock, &providers(2), Agreement::First).unwrap();
        assert_eq!(value, "0x1");
        assert_eq!(mock.urls().len(), 2);
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
