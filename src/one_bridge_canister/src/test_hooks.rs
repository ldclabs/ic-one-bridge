use std::cell::Cell;
thread_local! { static TRAP_AFTER_LEDGER: Cell<bool> = const { Cell::new(false) }; }
thread_local! { static UNBOUNDED_LEDGER: Cell<bool> = const { Cell::new(false) }; }

fn controller() -> Result<(), String> {
    if ic_cdk::api::is_controller(&ic_cdk::api::msg_caller()) {
        Ok(())
    } else {
        Err("controller only".into())
    }
}
#[ic_cdk::update(guard = "controller", hidden = true)]
fn test_trap_after_ledger(enabled: bool) {
    TRAP_AFTER_LEDGER.with(|flag| flag.set(enabled));
}

pub fn after_ledger_reply() {
    if TRAP_AFTER_LEDGER.with(Cell::get) {
        ic_cdk::trap("PocketIC: trap after ledger applied transfer, before callback commit");
    }
}

#[ic_cdk::update(guard = "controller", hidden = true)]
fn test_unbounded_ledger(enabled: bool) {
    UNBOUNDED_LEDGER.with(|flag| flag.set(enabled));
}
pub fn unbounded_ledger() -> bool {
    UNBOUNDED_LEDGER.with(Cell::get)
}
