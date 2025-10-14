use candid::{CandidType, Principal};
use serde::Deserialize;
use std::time::Duration;

use crate::store;

#[derive(Clone, Debug, CandidType, Deserialize)]
pub enum CanisterArgs {
    Init(InitArgs),
    Upgrade(UpgradeArgs),
}

#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct InitArgs {
    pub key_name: String,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_logo: String,
    pub token_ledger: Principal,
    pub min_threshold_to_bridge: u128,
    pub governance_canister: Option<Principal>,
}

#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct UpgradeArgs {
    pub token_name: Option<String>,
    pub token_symbol: Option<String>,
    pub token_logo: Option<String>,
    pub token_ledger: Option<Principal>,
    pub min_threshold_to_bridge: Option<u128>,
    pub governance_canister: Option<Principal>,
}

#[ic_cdk::init]
fn init(args: Option<CanisterArgs>) {
    if let Some(CanisterArgs::Init(args)) = args {
        store::state::with_mut(|s| {
            s.key_name = args.key_name;
            s.token_name = args.token_name;
            s.token_symbol = args.token_symbol;
            s.token_decimals = args.token_decimals;
            s.token_logo = args.token_logo;
            s.token_ledger = args.token_ledger;
            s.min_threshold_to_bridge = args.min_threshold_to_bridge;
            s.governance_canister = args.governance_canister;
        });
    } else if let Some(CanisterArgs::Upgrade(_)) = args {
        ic_cdk::trap("cannot init the canister with an Upgrade args. Please provide an Init args.");
    }

    store::state::init_http_certified_data();
    ic_cdk_timers::set_timer(Duration::from_secs(0), store::state::init_public_key());
}

#[ic_cdk::pre_upgrade]
fn pre_upgrade() {
    store::state::save();
}

#[ic_cdk::post_upgrade]
fn post_upgrade(args: Option<CanisterArgs>) {
    store::state::load();

    match args {
        Some(CanisterArgs::Upgrade(args)) => store::state::with_mut(|s| {
            if let Some(token_name) = args.token_name {
                s.token_name = token_name;
            }
            if let Some(token_symbol) = args.token_symbol {
                s.token_symbol = token_symbol;
            }
            if let Some(token_logo) = args.token_logo {
                s.token_logo = token_logo;
            }
            if let Some(token_ledger) = args.token_ledger {
                s.token_ledger = token_ledger;
            }
            if let Some(min_threshold_to_bridge) = args.min_threshold_to_bridge {
                s.min_threshold_to_bridge = min_threshold_to_bridge;
            }
            if let Some(governance_canister) = args.governance_canister {
                s.governance_canister = Some(governance_canister);
            }
        }),
        Some(CanisterArgs::Init(_)) => {
            ic_cdk::trap(
                "cannot upgrade the canister with an Init args. Please provide an Upgrade args.",
            );
        }
        _ => {}
    }

    let round = store::state::with_mut(|s| {
        s.finalize_bridging_round.1 = false; // reset the in-progress flag for edge case
        s.finalize_bridging_round.0
    });
    store::state::init_http_certified_data();
    ic_cdk_timers::set_timer(
        Duration::from_secs(3),
        store::state::finalize_bridging(round),
    );
}
