//! Admission and gas budgets are charged before external work and survive an
//! upgrade. Public work cannot spend the reserve used by outstanding payments.
use super::*;

const HOUR_MS: u64 = 3_600_000;

#[derive(Clone, Debug, CandidType, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub requests_per_hour: u32,
    pub requests_per_user_hour: u32,
    pub max_active_requests: u32,
    pub max_pending: u32,
    pub max_pending_per_user: u32,
    pub min_cycles_reserve: u128,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            requests_per_hour: 120,
            requests_per_user_hour: 12,
            max_active_requests: 16,
            max_pending: 512,
            max_pending_per_user: 32,
            min_cycles_reserve: 2_000_000_000_000,
        }
    }
}
impl ResourceLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.requests_per_hour == 0
            || self.requests_per_hour > 10_000
            || self.requests_per_user_hour == 0
            || self.requests_per_user_hour > self.requests_per_hour
            || self.max_active_requests == 0
            || self.max_active_requests > 128
            || self.max_pending == 0
            || self.max_pending > 100_000
            || self.max_pending_per_user == 0
            || self.max_pending_per_user > 1000
            || self.max_pending_per_user > self.max_pending
            || self.min_cycles_reserve < 100_000_000_000
        {
            return Err(
                "invalid resource limits: nonzero bounded quotas and a cycles reserve are required"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, CandidType, Serialize, Deserialize)]
pub struct EvmFeeLimits {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub max_transaction_fee: u128,
    pub max_hourly_fee: u128,
}
impl Default for EvmFeeLimits {
    fn default() -> Self {
        Self {
            max_fee_per_gas: 500_000_000_000,
            max_priority_fee_per_gas: 25_000_000_000,
            max_transaction_fee: 50_000_000_000_000_000,
            max_hourly_fee: 500_000_000_000_000_000,
        }
    }
}
impl EvmFeeLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_fee_per_gas == 0
            || self.max_priority_fee_per_gas > self.max_fee_per_gas
            || self.max_transaction_fee == 0
            || self.max_transaction_fee > self.max_hourly_fee
        {
            return Err("invalid EVM fee limits".into());
        }
        Ok(())
    }
    pub fn transaction_cost(&self, gas: u64, max_fee: u128, tip: u128) -> Result<u128, String> {
        let cost = u128::from(gas)
            .checked_mul(max_fee)
            .ok_or_else(|| "gas cost overflow".to_string())?;
        if max_fee > self.max_fee_per_gas
            || tip > self.max_priority_fee_per_gas
            || tip > max_fee
            || cost > self.max_transaction_fee
        {
            return Err("gas quote exceeds configured transaction or priority fee limits".into());
        }
        Ok(cost)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Usage {
    hour: u64,
    calls: u32,
    users: HashMap<Principal, u32>,
    gas: HashMap<String, (u64, u128)>,
}
thread_local! {
    static USAGE: RefCell<StableCell<Cbor<Usage>, Memory>> = RefCell::new(StableCell::init(memory(10), Cbor(Usage::default())));
}

fn charge(
    usage: &mut Usage,
    limits: &ResourceLimits,
    user: Principal,
    now: u64,
    cycles: u128,
    active: usize,
) -> Result<(), String> {
    if cycles < limits.min_cycles_reserve {
        return Err("public work is paused to preserve cycles for pending payments".into());
    }
    if active >= limits.max_active_requests as usize {
        return Err("too many active requests; retry later".into());
    }
    let hour = now / HOUR_MS;
    if usage.hour != hour {
        usage.hour = hour;
        usage.calls = 0;
        usage.users.clear();
    }
    let used = usage.users.get(&user).copied().unwrap_or(0);
    if usage.calls >= limits.requests_per_hour || used >= limits.requests_per_user_hour {
        return Err("hourly request budget exhausted; retry in the next hour".into());
    }
    usage.calls += 1;
    usage.users.insert(user, used + 1);
    Ok(())
}

pub fn admit(user: Principal) -> Result<(), String> {
    let limits = STATE.with_borrow(|s| s.resource_limits.clone());
    let active = ACTIVE_BRIDGE_USERS.with_borrow(BTreeSet::len);
    USAGE.with_borrow_mut(|cell| {
        let mut usage = cell.get().0.clone();
        charge(
            &mut usage,
            &limits,
            user,
            now_ms(),
            ic_cdk::api::canister_cycle_balance(),
            active,
        )?;
        cell.set(Cbor(usage));
        Ok(())
    })
}

pub fn reserve_gas(chain: &str, cost: u128, now: u64, limit: u128) -> Result<(), String> {
    USAGE.with_borrow_mut(|cell| {
        let mut usage = cell.get().0.clone();
        let bucket = usage.gas.entry(chain.into()).or_insert((now / HOUR_MS, 0));
        if bucket.0 != now / HOUR_MS {
            *bucket = (now / HOUR_MS, 0);
        }
        let total = bucket
            .1
            .checked_add(cost)
            .ok_or_else(|| "gas budget overflow".to_string())?;
        if total > limit {
            return Err("hourly bridge gas budget exhausted".into());
        }
        bucket.1 = total;
        cell.set(Cbor(usage));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn funded_repeated_requests_and_sybil_requests_are_bounded() {
        let limits = ResourceLimits {
            requests_per_hour: 3,
            requests_per_user_hour: 2,
            ..Default::default()
        };
        let mut usage = Usage::default();
        let alice = Principal::from_slice(&[1]);
        let bob = Principal::from_slice(&[2]);
        assert!(charge(&mut usage, &limits, alice, 0, u128::MAX, 0).is_ok());
        assert!(charge(&mut usage, &limits, alice, 1, u128::MAX, 0).is_ok());
        assert!(charge(&mut usage, &limits, alice, 2, u128::MAX, 0).is_err());
        assert!(charge(&mut usage, &limits, bob, 3, u128::MAX, 0).is_ok());
        assert!(
            charge(
                &mut usage,
                &limits,
                Principal::from_slice(&[3]),
                4,
                u128::MAX,
                0
            )
            .is_err()
        );
        assert!(charge(&mut usage, &limits, alice, HOUR_MS, u128::MAX, 0).is_ok());
        assert!(charge(&mut usage, &limits, alice, HOUR_MS, 0, 0).is_err());
    }
    #[test]
    fn an_affordable_extreme_tip_is_still_rejected() {
        let limits = EvmFeeLimits::default();
        assert!(
            limits
                .transaction_cost(84_000, 1_200_000_002_000_000, 1_200_000_000_000_000)
                .is_err()
        );
        assert!(
            limits
                .transaction_cost(84_000, 3_000_000_000, 1_000_000_000)
                .is_ok()
        );
    }
}
