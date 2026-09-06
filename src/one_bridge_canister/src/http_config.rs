//! Cached, certified configuration. Live operational statistics remain available
//! through info()/the legacy HTTP endpoint; funds addresses have an exact proof.
use crate::{api_http::HttpResponse, store};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ic_auth_types::cbor_into_vec;
use ic_http_certification::{
    CERTIFICATE_EXPRESSION_HEADER_NAME, DefaultCelBuilder, DefaultResponseCertification,
    HttpCertification, HttpCertificationPath, HttpCertificationTree, HttpCertificationTreeEntry,
    HttpRequest, HttpResponse as CertifiedHttpResponse,
};
use serde::Serialize;
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Serialize)]
struct Configuration {
    canister_id: String,
    token_name: String,
    token_symbol: String,
    token_decimals: u8,
    token_ledger: String,
    token_bridge_fee: u128,
    min_threshold_to_bridge: u128,
    ledger_verified: bool,
    evm_address: Option<String>,
    svm_address: Option<String>,
    keys_ready: (bool, bool),
    svm_mint_verified: bool,
    evm_token_contracts: BTreeMap<String, (String, u8, u64)>,
    svm_token_address: (String, u8, String),
    resource_limits: store::ResourceLimits,
    evm_fee_limits: BTreeMap<String, store::EvmFeeLimits>,
}

struct Cached {
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    status: u16,
    entry: HttpCertificationTreeEntry<'static>,
}
thread_local! {
    static CACHE: RefCell<BTreeMap<(String,String),Cached>> = const { RefCell::new(BTreeMap::new()) };
    static LAST_JSON: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

pub fn refresh() {
    let config = store::state::with(|s| Configuration {
        canister_id: s.icp_address.to_text(),
        token_name: s.token_name.clone(),
        token_symbol: s.token_symbol.clone(),
        token_decimals: s.token_decimals,
        token_ledger: s.token_ledger.to_text(),
        token_bridge_fee: s.token_bridge_fee,
        min_threshold_to_bridge: s.min_threshold_to_bridge,
        ledger_verified: s.ledger_verified,
        evm_address: (!s.ecdsa_public_key.public_key.is_empty()).then(|| s.evm_address.to_string()),
        svm_address: (!s.ed25519_public_key.public_key.is_empty())
            .then(|| s.svm_address.to_string()),
        keys_ready: (
            !s.ecdsa_public_key.public_key.is_empty(),
            !s.ed25519_public_key.public_key.is_empty(),
        ),
        svm_mint_verified: s.svm_mint_verified,
        evm_token_contracts: s
            .evm_token_contracts
            .iter()
            .map(|(k, (a, d, id))| (k.clone(), (a.to_string(), *d, *id)))
            .collect(),
        svm_token_address: (
            s.svm_token_address.0.to_string(),
            s.svm_token_address.1,
            s.svm_token_address.2.to_string(),
        ),
        resource_limits: s.resource_limits.clone(),
        evm_fee_limits: s
            .evm_token_contracts
            .keys()
            .map(|chain| {
                (
                    chain.clone(),
                    s.evm_fee_limits.get(chain).cloned().unwrap_or_default(),
                )
            })
            .collect(),
    });
    let json = serde_json::to_vec(&config).expect("configuration JSON");
    if LAST_JSON.with_borrow(|last| *last == json) {
        return;
    }
    let cbor = cbor_into_vec(&config).expect("configuration CBOR");
    let expression = DefaultCelBuilder::full_certification()
        .with_request_headers(vec![])
        .with_request_query_parameters(vec![])
        .with_response_certification(DefaultResponseCertification::certified_response_headers(
            vec![
                "content-type",
                "content-length",
                "cache-control",
                "x-content-type-options",
            ],
        ))
        .build();
    let mut tree = HttpCertificationTree::default();
    tree.insert(&store::state::DEFAULT_CERT_ENTRY);
    let mut cache = BTreeMap::new();
    for (path, body, content_type) in [
        ("/config", json.clone(), "application/json"),
        ("/config.cbor", cbor, "application/cbor"),
    ] {
        for method in ["GET", "HEAD"] {
            let bytes = if method == "HEAD" {
                vec![]
            } else {
                body.clone()
            };
            let headers = vec![
                ("content-type".into(), content_type.into()),
                ("content-length".into(), body.len().to_string()),
                ("cache-control".into(), "no-cache".into()),
                ("x-content-type-options".into(), "nosniff".into()),
                (
                    CERTIFICATE_EXPRESSION_HEADER_NAME.into(),
                    expression.to_string(),
                ),
            ];
            let request = HttpRequest::builder()
                .with_url(path)
                .with_method(method.parse().expect("HTTP method"))
                .build();
            let response = CertifiedHttpResponse::ok(bytes.clone(), headers.clone()).build();
            let cert = HttpCertification::full(&expression, &request, &response, None)
                .expect("configuration certification");
            let entry = HttpCertificationTreeEntry::new(HttpCertificationPath::exact(path), cert);
            tree.insert(&entry);
            cache.insert(
                (path.to_string(), method.to_string()),
                Cached {
                    body: bytes,
                    headers,
                    status: 200,
                    entry,
                },
            );
        }
        let expr = DefaultCelBuilder::response_only_certification()
            .with_response_certification(DefaultResponseCertification::certified_response_headers(
                vec!["content-type", "allow"],
            ))
            .build();
        let body = b"Only GET and HEAD requests with an empty body are supported".to_vec();
        let headers = vec![
            ("content-type".into(), "text/plain".into()),
            ("allow".into(), "GET, HEAD".into()),
            (CERTIFICATE_EXPRESSION_HEADER_NAME.into(), expr.to_string()),
        ];
        let response =
            CertifiedHttpResponse::method_not_allowed(body.clone(), headers.clone()).build();
        let cert = HttpCertification::response_only(&expr, &response, None)
            .expect("method error certification");
        let entry = HttpCertificationTreeEntry::new(HttpCertificationPath::exact(path), cert);
        tree.insert(&entry);
        cache.insert(
            (path.to_string(), "ERROR".into()),
            Cached {
                body,
                headers,
                status: 405,
                entry,
            },
        );
    }
    store::state::replace_http_tree(tree);
    CACHE.with_borrow_mut(|target| *target = cache);
    LAST_JSON.with_borrow_mut(|last| *last = json);
}

pub fn response(request: &HttpRequest<'_>) -> Option<HttpResponse> {
    let path = request.get_path().ok()?;
    if path != "/config" && path != "/config.cbor" {
        return None;
    }
    let method = if request.body().is_empty() && matches!(request.method().as_str(), "GET" | "HEAD")
    {
        request.method().as_str()
    } else {
        "ERROR"
    };
    CACHE.with_borrow(|cache| {
        let cached = cache.get(&(path.clone(), method.to_string()))?;
        let certificate = ic_cdk::api::data_certificate()?;
        let witness =
            store::state::http_tree_with(|tree| tree.witness(&cached.entry, &path)).ok()?;
        let mut headers = cached.headers.clone();
        headers.push((
            "ic-certificate".into(),
            format!(
                "certificate=:{}:, tree=:{}:, expr_path=:{}:, version=2",
                BASE64.encode(certificate),
                BASE64.encode(cbor_into_vec(&witness).ok()?),
                BASE64.encode(
                    cbor_into_vec(&HttpCertificationPath::exact(path.as_str()).to_expr_path())
                        .ok()?
                )
            ),
        ));
        Some(HttpResponse {
            status_code: cached.status,
            headers,
            body: cached.body.clone().into(),
            upgrade: None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_is_cached_and_certified_without_provider_credentials() {
        store::state::with_mut(|s| {
            s.evm_providers.insert(
                "ETH".into(),
                (0, vec!["https://rpc.example/key-secret123".into()]),
            );
        });
        refresh();
        let before = store::state::http_tree_with(|tree| tree.root_hash());
        refresh();
        assert_eq!(
            before,
            store::state::http_tree_with(|tree| tree.root_hash())
        );
        CACHE.with_borrow(|cache| {
            let body = &cache.get(&("/config".into(), "GET".into())).unwrap().body;
            assert!(!String::from_utf8_lossy(body).contains("secret123"));
            assert_eq!(cache.len(), 6);
        });
        store::state::with_mut(|s| s.token_symbol = "OTHER".into());
        refresh();
        assert_ne!(
            before,
            store::state::http_tree_with(|tree| tree.root_hash())
        );
    }
}
