use candid::Principal;
use serde_bytes::ByteBuf;
use std::collections::BTreeSet;

mod api;
mod api_admin;
mod api_http;
mod api_init;
mod ecdsa;
mod evm;
mod helper;
mod http_config;
mod outcall;
mod schnorr;
mod store;
mod svm;
#[cfg(feature = "test-hooks")]
mod test_hooks;
mod types;

use api_init::CanisterArgs;

ic_cdk::export_candid!();
