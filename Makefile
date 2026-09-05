BUILD_ENV := rust

.PHONY: build-wasm build-did

lint:
	@cargo fmt
	@cargo clippy --all-targets --all-features

fix:
	@cargo clippy --fix --workspace --tests

test:
	@cargo test --workspace -- --nocapture

# cargo install ic-wasm
build-wasm:
	cargo build --release --target wasm32-unknown-unknown --package one_bridge_canister

# cargo install candid-extractor
#
# dfx generate writes to src/declarations, but the web app imports its own copy
# under src/one_bridge_app/src/declarations, so the copy below is what keeps the
# app's interface from silently drifting behind the canister's.
build-did:
	candid-extractor target/wasm32-unknown-unknown/release/one_bridge_canister.wasm > src/one_bridge_canister/one_bridge_canister.did
	dfx generate
	cp src/declarations/one_bridge_canister/one_bridge_canister.did* \
		src/one_bridge_app/src/declarations/one_bridge_canister/
