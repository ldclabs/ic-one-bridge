#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

# build and get batch_id, evidence:
# dfx deploy one_bridge_app --ic --by-proposal

export BLOB="$(didc encode --format blob '(record {batch_id=7:nat; evidence=blob "\43\bb\5f\99\b6\a7\46\49\f0\5a\7b\4e\5b\73\ec\4e\b0\ed\a9\51\50\3d\c4\a5\df\d7\15\2b\b6\c3\27\70"})')"

quill sns make-proposal --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE $PROPOSAL_NEURON_ID --proposal "(
    record {
        title = \"Execute commit_proposed_batch() to release one_bridge_app v0.5.2\";
        url = \"https://1bridge.app/\";
        summary = \"This proposal executes commit_proposed_batch() on ejwdq-iyaaa-aaaap-an47q-cai to release one_bridge_app v0.5.2, the front end for one_bridge_canister v0.5.2. The form now sizes its EVM gas estimate from the canister's own gas limit and fee arithmetic, so it no longer clears a balance the canister then refuses to sign for; bridging out of Solana accounts for the fee the user's derived address pays; a failed Solana transfer is reported as failed instead of completed; and a paused bridge now says it retries by itself, matching the canister's circuit breaker. A chain the bridge lists but this build has never seen degrades to a plain EVM entry instead of taking the chain selector down. Roughly 2,400 lines of unreachable code and 20 unused dependencies are gone.\";
        action = opt variant {
            ExecuteGenericNervousSystemFunction = record {
                function_id = 1_300 : nat64;
                payload = ${BLOB};
            }
        };
    }
)" > proposal-message.json

# quill send proposal-message.json