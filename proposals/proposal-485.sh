#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

export CANISTERS_PATH="$(pwd)/debug"

quill sns make-upgrade-canister-proposal $PROPOSAL_NEURON_ID --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE --target-canister-id "dpjyw-raaaa-aaaar-qbxlq-cai" --wasm-path "$CANISTERS_PATH/one_bridge_canister.wasm.gz" --mode upgrade --title "Upgrade one_bridge_canister canister to v0.5.0" --summary "fix: fix some issues" --url "https://github.com/ldclabs/ic-one-bridge/releases/tag/v0.5.0" > proposal-message.json

# quill send proposal-message.json