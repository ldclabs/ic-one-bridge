use ic_dummy_getrandom_for_wasm as _;
#[cfg(feature = "legacy")]
mod legacy;

#[cfg(not(feature = "legacy"))]
mod ledger {
    use candid::{CandidType, Nat, Principal};
    use icrc_ledger_types::{
        icrc1::{
            account::Account,
            transfer::{TransferArg, TransferError},
        },
        icrc2::transfer_from::{TransferFromArgs, TransferFromError},
    };
    use serde::{Deserialize, Serialize};
    use std::{cell::RefCell, collections::BTreeMap};
    #[derive(Default, CandidType, Serialize, Deserialize)]
    struct Ledger {
        mode: u8,
        next: u64,
        incoming: u64,
        outgoing: u64,
        last_amount: u128,
        last_to: Option<Principal>,
        dedup: BTreeMap<Vec<u8>, u64>,
        #[serde(default)]
        metadata_calls: u64,
        #[serde(default)]
        fee_calls: u64,
    }
    thread_local! { static STATE:RefCell<Ledger> = RefCell::new(Ledger::default()); }
    #[derive(CandidType, Serialize, Deserialize)]
    struct Stats {
        incoming: u64,
        outgoing: u64,
        last_amount: u128,
        last_to: Option<Principal>,
        metadata_calls: u64,
        fee_calls: u64,
    }
    #[ic_cdk::query]
    fn stats() -> Stats {
        STATE.with_borrow(|s| Stats {
            incoming: s.incoming,
            outgoing: s.outgoing,
            last_amount: s.last_amount,
            last_to: s.last_to,
            metadata_calls: s.metadata_calls,
            fee_calls: s.fee_calls,
        })
    }
    #[ic_cdk::query]
    fn icrc1_fee() -> Nat {
        STATE.with_borrow_mut(|s| s.fee_calls += 1);
        10u64.into()
    }
    #[ic_cdk::query]
    fn icrc1_decimals() -> u8 {
        STATE.with_borrow_mut(|s| {
            s.metadata_calls += 1;
            match s.mode {
                3 => ic_cdk::trap("temporary metadata outage"),
                4 => 9,
                _ => 8,
            }
        })
    }
    #[ic_cdk::query]
    fn icrc1_minting_account() -> Option<Account> {
        None
    }
    #[ic_cdk::query]
    fn icrc1_balance_of(_: Account) -> Nat {
        1_000_000_000_000_000_000u64.into()
    }
    #[ic_cdk::update]
    fn set_mode(mode: u8) {
        assert!(ic_cdk::api::is_controller(&ic_cdk::api::msg_caller()));
        STATE.with_borrow_mut(|s| s.mode = mode);
    }
    #[ic_cdk::query]
    fn pulse() {}
    fn key<T: Serialize>(kind: u8, args: &T) -> Vec<u8> {
        let mut key = vec![kind];
        key.extend(ic_auth_types::cbor_into_vec(&(ic_cdk::api::msg_caller(), args)).unwrap());
        key
    }
    #[ic_cdk::update(manual_reply = true)]
    fn icrc2_transfer_from(args: TransferFromArgs) {
        let key = key(2, &args);
        let (result, bad) = STATE.with_borrow_mut(|s| {
            let error = match s.mode {
                5 => Some(TransferFromError::BadFee {
                    expected_fee: 20u64.into(),
                }),
                6 => Some(TransferFromError::InsufficientFunds {
                    balance: 0u64.into(),
                }),
                7 => Some(TransferFromError::TemporarilyUnavailable),
                8 => Some(TransferFromError::TooOld),
                _ => None,
            };
            if let Some(error) = error {
                return (Err(error), false);
            }
            if args.created_at_time.is_some()
                && let Some(id) = s.dedup.get(&key)
            {
                return (
                    Err(TransferFromError::Duplicate {
                        duplicate_of: (*id).into(),
                    }),
                    false,
                );
            }
            let id = s.next;
            s.next += 1;
            s.incoming += 1;
            if args.created_at_time.is_some() {
                s.dedup.insert(key, id);
            }
            let bad = s.mode == 1;
            if bad {
                s.mode = 0;
            }
            (Ok::<Nat, TransferFromError>(id.into()), bad)
        });
        if bad {
            ic_cdk::api::msg_reply(candid::encode_one("malformed successful reply").unwrap());
        } else {
            ic_cdk::api::msg_reply(candid::encode_one(result).unwrap());
        }
    }
    #[ic_cdk::update]
    async fn icrc1_transfer(args: TransferArg) -> Result<Nat, TransferError> {
        let error = STATE.with_borrow(|s| match s.mode {
            5 => Some(TransferError::BadFee {
                expected_fee: 20u64.into(),
            }),
            6 => Some(TransferError::InsufficientFunds {
                balance: 0u64.into(),
            }),
            7 => Some(TransferError::TemporarilyUnavailable),
            8 => Some(TransferError::TooOld),
            _ => None,
        });
        if let Some(error) = error {
            return Err(error);
        }
        let key = key(1, &args);
        let (id, duplicate, hold) = STATE.with_borrow_mut(|s| {
            if args.created_at_time.is_some()
                && let Some(id) = s.dedup.get(&key)
            {
                return (*id, true, false);
            }
            let id = s.next;
            s.next += 1;
            s.outgoing += 1;
            s.last_amount = u128::try_from(&args.amount.0).unwrap();
            s.last_to = Some(args.to.owner);
            if args.created_at_time.is_some() {
                s.dedup.insert(key, id);
            }
            (id, false, s.mode == 2)
        });
        if duplicate {
            return Err(TransferError::Duplicate {
                duplicate_of: id.into(),
            });
        }
        if hold {
            while STATE.with_borrow(|s| s.mode == 2) {
                ic_cdk::call::Call::unbounded_wait(ic_cdk::api::canister_self(), "pulse")
                    .await
                    .unwrap();
            }
        }
        Ok(id.into())
    }
    #[ic_cdk::pre_upgrade]
    fn save() {
        STATE.with_borrow(|s| ic_cdk::storage::stable_save((s,)).unwrap());
    }
    #[ic_cdk::post_upgrade]
    fn load() {
        let (state,): (Ledger,) = ic_cdk::storage::stable_restore().unwrap();
        STATE.with_borrow_mut(|s| *s = state);
    }
}
