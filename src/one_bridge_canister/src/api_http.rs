use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use candid::CandidType;
use http::Uri;
use ic_auth_types::{ByteBufB64, cbor_into_vec};
use ic_http_certification::{HeaderField, HttpRequest};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::store;

#[derive(CandidType, Deserialize, Serialize, Clone, Default)]
pub struct HttpResponse {
    pub status_code: u16,
    pub headers: Vec<HeaderField>,
    pub body: ByteBufB64,
    pub upgrade: Option<bool>,
}

static CBOR: &str = "application/cbor";
static JSON: &str = "application/json";
static IC_CERTIFICATE_HEADER: &str = "ic-certificate";
static IC_CERTIFICATE_EXPRESSION_HEADER: &str = "ic-certificateexpression";
static CERTIFIED_EXPR_PATH: LazyLock<String> = LazyLock::new(|| {
    BASE64.encode(
        cbor_into_vec(&store::state::DEFAULT_EXPR_PATH.to_expr_path())
            .expect("failed to serialize expr path"),
    )
});

#[ic_cdk::query(hidden = true)]
fn http_request(request: HttpRequest<'static>) -> HttpResponse {
    let witness = store::state::http_tree_with(|t| {
        t.witness(&store::state::DEFAULT_CERT_ENTRY, request.url())
            .expect("get witness failed")
    });

    let certified_data = ic_cdk::api::data_certificate().expect("no data certificate available");

    let mut headers = vec![
        ("x-content-type-options".to_string(), "nosniff".to_string()),
        (
            IC_CERTIFICATE_EXPRESSION_HEADER.to_string(),
            store::state::DEFAULT_CEL_EXPR.clone(),
        ),
        (
            IC_CERTIFICATE_HEADER.to_string(),
            format!(
                "certificate=:{}:, tree=:{}:, expr_path=:{}:, version=2",
                BASE64.encode(certified_data),
                BASE64.encode(cbor_into_vec(&witness).expect("failed to serialize witness")),
                CERTIFIED_EXPR_PATH.as_str(),
            ),
        ),
    ];

    let req_uri = match parse_uri(request.url()) {
        Ok(url) => url,
        Err(err) => {
            headers.push(("content-type".to_string(), "text/plain".to_string()));
            return HttpResponse {
                status_code: 400,
                headers,
                body: err.into_bytes().into(),
                upgrade: None,
            };
        }
    };

    let in_cbor = supports_cbor(request.headers());

    let rt = match (request.method().as_str(), req_uri.path()) {
        ("HEAD", _) => Ok(Vec::new()),
        ("GET", "/") => {
            let info = store::state::info();
            if in_cbor {
                cbor_into_vec(&info)
                    .map_err(|err| format!("failed to serialize info to cbor: {err}"))
            } else {
                serde_json::to_vec(&info)
                    .map_err(|err| format!("failed to serialize info to json: {err}"))
            }
        }
        (method, path) => Err(format!("http_request, method {method}, path: {path}")),
    };

    match rt {
        Ok(body) => {
            if in_cbor {
                headers.push(("content-type".to_string(), CBOR.to_string()));
            } else {
                headers.push(("content-type".to_string(), JSON.to_string()));
            }
            headers.push(("content-length".to_string(), body.len().to_string()));
            HttpResponse {
                status_code: 200,
                headers,
                body: body.into(),
                upgrade: None,
            }
        }
        Err(err) => {
            headers.push(("content-type".to_string(), "text/plain".to_string()));
            HttpResponse {
                status_code: 400,
                headers,
                body: err.into_bytes().into(),
                upgrade: None,
            }
        }
    }
}

fn parse_uri(s: &str) -> Result<Uri, String> {
    let uri = s
        .parse::<Uri>()
        .map_err(|err| format!("failed to parse url {s}, error: {err}"))?;
    if s.starts_with('/') || (uri.scheme().is_some() && uri.authority().is_some()) {
        Ok(uri)
    } else {
        Err(format!(
            "url must be an absolute URI or start with '/': {s}"
        ))
    }
}

fn supports_cbor(headers: &[HeaderField]) -> bool {
    headers.iter().any(|(name, value)| {
        (name.eq_ignore_ascii_case("accept") || name.eq_ignore_ascii_case("content-type"))
            && value.split(',').any(|part| {
                part.trim()
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .eq_ignore_ascii_case(CBOR)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_cbor_matches_header_names_and_values_case_insensitively() {
        assert!(supports_cbor(&[(
            "Accept".to_string(),
            "Application/CBOR".to_string()
        )]));
        assert!(supports_cbor(&[(
            "CONTENT-TYPE".to_string(),
            "application/cbor; charset=binary".to_string()
        )]));
        assert!(!supports_cbor(&[(
            "accept".to_string(),
            "application/json".to_string()
        )]));
    }

    #[test]
    fn parses_gateway_and_absolute_request_uris() {
        assert_eq!(parse_uri("/?id=1").unwrap().path(), "/");
        assert_eq!(
            parse_uri("https://bridge.example/info?id=1")
                .unwrap()
                .path(),
            "/info"
        );
        assert!(parse_uri("not-an-origin-form-uri").is_err());
    }
}
