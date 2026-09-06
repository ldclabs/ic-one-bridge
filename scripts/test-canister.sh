#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

task_target="${CARGO_TARGET_DIR:-target}"
fixture_target="$task_target/test-fixture"
mkdir -p "$task_target/test-wasm"
cargo build --locked --release --target wasm32-unknown-unknown -p one_bridge_canister --target-dir "$task_target"
cp "$task_target/wasm32-unknown-unknown/release/one_bridge_canister.wasm" "$task_target/test-wasm/bridge.wasm"
# A separate target keeps the fault-injection artifact out of the deploy path.
cargo build --locked --release --target wasm32-unknown-unknown -p one_bridge_canister --features test-hooks --target-dir "$task_target/integration-hooks"
cp "$task_target/integration-hooks/wasm32-unknown-unknown/release/one_bridge_canister.wasm" "$task_target/test-wasm/bridge-hooks.wasm"
cargo build --locked --manifest-path src/one_bridge_canister/tests/fixtures/Cargo.toml --release --target wasm32-unknown-unknown --target-dir "$fixture_target"
cp "$fixture_target/wasm32-unknown-unknown/release/bridge_test_fixture.wasm" "$task_target/test-wasm/ledger.wasm"
cargo build --locked --manifest-path src/one_bridge_canister/tests/fixtures/Cargo.toml --release --target wasm32-unknown-unknown --target-dir "$fixture_target" --features legacy
cp "$fixture_target/wasm32-unknown-unknown/release/bridge_test_fixture.wasm" "$task_target/test-wasm/legacy.wasm"
export BRIDGE_TEST_WASM_DIR="$(cd "$task_target/test-wasm" && pwd)"
cargo test --locked -p one_bridge_canister --test recovery -- --ignored --nocapture
