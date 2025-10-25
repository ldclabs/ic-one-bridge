#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

quill sns make-proposal --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE $PROPOSAL_NEURON_ID --proposal '(
    record {
        title = "Register One Bridge canisters";
        url = "https://github.com/ldclabs/ic-one-bridge";
        summary = "One Bridge App: https://1bridge.app";
        action = opt variant {
            RegisterDappCanisters = record {
                canister_ids = vec {principal "dpjyw-raaaa-aaaar-qbxlq-cai"; principal "ejwdq-iyaaa-aaaap-an47q-cai"};
            }
        };
    }
)' > proposal-message.json

# quill send proposal-message.json