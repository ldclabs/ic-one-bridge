use candid::{
    CandidType, IDLValue, Principal, pretty::candid::value::pp_value, utils::ArgumentEncoder,
};
use std::collections::BTreeSet;

const ANONYMOUS: Principal = Principal::anonymous();

pub static APP_AGENT: &str = concat!(
    "Mozilla/5.0 ICP canister ",
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
);

pub fn msg_caller() -> Result<Principal, String> {
    let caller = ic_cdk::api::msg_caller();
    check_auth(&caller)?;
    Ok(caller)
}

pub fn check_auth(user: &Principal) -> Result<(), String> {
    if user == &ANONYMOUS {
        Err("anonymous user is not allowed".to_string())
    } else {
        Ok(())
    }
}

pub fn validate_principals(principals: &BTreeSet<Principal>) -> Result<(), String> {
    if principals.is_empty() {
        return Err("principals cannot be empty".to_string());
    }
    if principals.contains(&ANONYMOUS) {
        return Err("anonymous user is not allowed".to_string());
    }
    Ok(())
}

pub fn format_error<T>(err: T) -> String
where
    T: std::fmt::Debug,
{
    format!("{:?}", err)
}

pub fn convert_amount(
    src_amount: u128,
    src_decimals: u8,
    target_decimals: u8,
) -> Result<u128, String> {
    if src_decimals == target_decimals {
        Ok(src_amount)
    } else if src_decimals < target_decimals {
        let factor = 10u128
            .checked_pow((target_decimals - src_decimals) as u32)
            .ok_or_else(|| "exponent too large".to_string())?;
        src_amount
            .checked_mul(factor)
            .ok_or_else(|| "multiplication overflow".to_string())
    } else {
        let factor = 10u128
            .checked_pow((src_decimals - target_decimals) as u32)
            .ok_or_else(|| "exponent too large".to_string())?;
        Ok(src_amount / factor)
    }
}

pub fn bridge_amount_after_fee(amount: u128, fee: u128) -> Result<u128, String> {
    amount
        .checked_sub(fee)
        .filter(|amount| *amount > 0)
        .ok_or_else(|| format!("amount {amount} must be greater than bridge fee {fee}"))
}

/// Calls another canister and decodes its reply.
///
/// The call waits unbounded on purpose. Every use of this is a ledger transfer,
/// and a bounded-wait call that times out reports an unknown outcome: the
/// transfer may still have gone through, and the finalization round would then
/// retry it and pay the recipient twice.
pub async fn call<In, Out>(id: Principal, method: &str, args: In) -> Result<Out, String>
where
    In: ArgumentEncoder + Send,
    Out: candid::CandidType + for<'a> candid::Deserialize<'a>,
{
    let res = ic_cdk::call::Call::unbounded_wait(id, method)
        .with_args(&args)
        .await
        .map_err(|err| format!("failed to call {} on {:?}, error: {:?}", method, id, err))?;
    res.candid().map_err(|err| {
        format!(
            "failed to decode response from {} on {:?}, error: {:?}",
            method, id, err
        )
    })
}

pub fn pretty_format<T>(data: &T) -> Result<String, String>
where
    T: CandidType,
{
    let val = IDLValue::try_from_candid_type(data).map_err(|err| format!("{err:?}"))?;
    let doc = pp_value(7, &val);

    Ok(format!("{}", doc.pretty(120)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_amount_after_fee_requires_positive_net_amount() {
        assert_eq!(bridge_amount_after_fee(100, 1), Ok(99));
        assert!(bridge_amount_after_fee(100, 100).is_err());
        assert!(bridge_amount_after_fee(100, 101).is_err());
    }

    #[test]
    fn convert_amount_downscales_with_flooring() {
        assert_eq!(convert_amount(123_456_789, 8, 6), Ok(1_234_567));
        assert_eq!(convert_amount(99, 8, 6), Ok(0));
    }
}
