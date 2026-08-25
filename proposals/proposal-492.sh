#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

export CANISTERS_PATH="$(pwd)/debug"

# token_bridge_fee and min_threshold_to_bridge are only read from the post-upgrade
# arguments, so changing them means reinstalling the WASM that is already running.
# Before submitting, check that it really is the same one:
#   shasum -a 256 "$CANISTERS_PATH/one_bridge_canister.wasm.gz"
#   curl -s https://ic-api.internetcomputer.org/api/v3/canisters/dpjyw-raaaa-aaaar-qbxlq-cai | jq -r .module_hash
export UPGRADE_ARG='(opt variant { Upgrade = record {
  token_name = null;
  token_symbol = null;
  token_logo = null;
  token_ledger = null;
  token_bridge_fee = opt (10_000_000_000 : nat);
  min_threshold_to_bridge = opt (1_000_000_000_000 : nat);
  governance_canister = null;
} })'

quill sns make-upgrade-canister-proposal $PROPOSAL_NEURON_ID --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE --target-canister-id "dpjyw-raaaa-aaaar-qbxlq-cai" --wasm-path "$CANISTERS_PATH/one_bridge_canister.wasm.gz" --canister-upgrade-arg "$UPGRADE_ARG" --mode upgrade --title "Upgrade one_bridge_canister canister to set the bridge fee to 100 PANDA and the minimum to 10,000 PANDA" --summary "This proposal raises two bridging parameters on dpjyw-raaaa-aaaar-qbxlq-cai:

- token_bridge_fee: 1 PANDA -> 100 PANDA (100_000_000 -> 10_000_000_000)
- min_threshold_to_bridge: 1,000 PANDA -> 10,000 PANDA (100_000_000_000 -> 1_000_000_000_000)

Neither can be set through a canister method; both are only read from the post-upgrade arguments, which is why this takes the form of an upgrade proposal. The WASM it installs is the one already running, from the v0.5.0 release: its SHA-256 is b56c1fef6e0225539520183f006c28055256011a8fec79979041b879753ad655, which is the module hash the canister reports today. There are no code changes.

Once this takes effect, a bridge request below 10,000 PANDA is rejected and each completed bridge collects 100 PANDA. Tasks already in flight keep the fee recorded when they were created." --url "https://github.com/ldclabs/ic-one-bridge/releases/tag/v0.5.0" > proposal-message.json

# quill send proposal-message.json
