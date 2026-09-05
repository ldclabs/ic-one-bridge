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
    pub token_bridge_fee: u128,
    pub min_threshold_to_bridge: u128,
    pub governance_canister: Option<Principal>,
    /// Gas limit of the token's ERC-20 `transfer`; omitted, a limit that fits
    /// a plain OpenZeppelin token is used.
    pub erc20_gas_limit: Option<u64>,
}

#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct UpgradeArgs {
    pub token_name: Option<String>,
    pub token_symbol: Option<String>,
    pub token_logo: Option<String>,
    pub token_ledger: Option<Principal>,
    pub token_bridge_fee: Option<u128>,
    pub min_threshold_to_bridge: Option<u128>,
    pub governance_canister: Option<Principal>,
    pub erc20_gas_limit: Option<u64>,
}

fn checked_erc20_gas_limit(gas_limit: Option<u64>) -> Option<u64> {
    if let Some(gas_limit) = gas_limit {
        store::validate_erc20_gas_limit(gas_limit).unwrap_or_else(|err| ic_cdk::trap(&err));
    }
    gas_limit
}

#[ic_cdk::init]
fn init(args: Option<CanisterArgs>) {
    if let Some(CanisterArgs::Init(args)) = args {
        let erc20_gas_limit = checked_erc20_gas_limit(args.erc20_gas_limit);
        store::state::with_mut(|s| {
            s.key_name = args.key_name;
            s.token_name = args.token_name;
            s.token_symbol = args.token_symbol;
            s.token_decimals = args.token_decimals;
            s.token_logo = args.token_logo;
            s.token_ledger = args.token_ledger;
            s.token_bridge_fee = args.token_bridge_fee;
            s.min_threshold_to_bridge = args.min_threshold_to_bridge;
            s.governance_canister = args.governance_canister;
            if let Some(erc20_gas_limit) = erc20_gas_limit {
                s.erc20_gas_limit = erc20_gas_limit;
            }
        });
    } else if let Some(CanisterArgs::Upgrade(_)) = args {
        ic_cdk::trap("cannot init the canister with an Upgrade args. Please provide an Init args.");
    }

    store::state::init_http_certified_data();
    ic_cdk_timers::set_timer(Duration::from_secs(0), store::state::init_public_keys());
}

#[ic_cdk::pre_upgrade]
fn pre_upgrade() {
    store::state::save();
}

#[ic_cdk::post_upgrade]
fn post_upgrade(args: Option<CanisterArgs>) {
    store::state::load();
    store::state::migrate_user_log_index();
    store::state::migrate_icp_collected_fees();

    match args {
        Some(CanisterArgs::Upgrade(args)) => {
            let erc20_gas_limit = checked_erc20_gas_limit(args.erc20_gas_limit);
            store::state::with_mut(|s| {
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
                if let Some(token_bridge_fee) = args.token_bridge_fee {
                    s.token_bridge_fee = token_bridge_fee;
                }
                if let Some(min_threshold_to_bridge) = args.min_threshold_to_bridge {
                    s.min_threshold_to_bridge = min_threshold_to_bridge;
                }
                if let Some(governance_canister) = args.governance_canister {
                    s.governance_canister = Some(governance_canister);
                }
                if let Some(erc20_gas_limit) = erc20_gas_limit {
                    s.erc20_gas_limit = erc20_gas_limit;
                }
            })
        }
        Some(CanisterArgs::Init(_)) => {
            ic_cdk::trap(
                "cannot upgrade the canister with an Init args. Please provide an Upgrade args.",
            );
        }
        _ => {}
    }

    store::state::with_mut(|s| {
        s.finalize_bridging_round.1 = false; // reset the in-progress flag for edge case
        s.finalize_bridging_started_at = 0;
    });
    store::state::init_http_certified_data();
    ic_cdk_timers::set_timer(Duration::from_secs(0), async {
        store::state::try_init_public_keys().await;
    });
    store::state::schedule_finalize(Duration::from_secs(3));
}
