#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

# build and get batch_id, evidence:
# dfx deploy one_bridge_app --ic --by-proposal

export BLOB="$(didc encode --format blob '(record {batch_id=6:nat; evidence=blob "\ee\10\a3\2f\63\ef\45\8d\b9\f5\63\63\0e\15\65\2e\d7\a7\88\81\ff\4d\18\c7\a8\7f\e3\89\7c\7e\d8\37"})')"

quill sns make-proposal --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE $PROPOSAL_NEURON_ID --proposal "(
    record {
        title = \"Execute commit_proposed_batch() to release one_bridge_app v0.5.0\";
        url = \"https://1bridge.app/\";
        summary = \"This proposal executes commit_proposed_batch() on ejwdq-iyaaa-aaaap-an47q-cai to release one_bridge_app v0.5.0.\";
        action = opt variant {
            ExecuteGenericNervousSystemFunction = record {
                function_id = 1_300 : nat64;
                payload = ${BLOB};
            }
        };
    }
)" > proposal-message.json

# quill send proposal-message.json