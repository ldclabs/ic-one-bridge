#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

# check the state before and after executing this proposal:
# dfx canister --network ic call --query dpjyw-raaaa-aaaar-qbxlq-cai info '()'
# dfx canister --network ic call --query dpjyw-raaaa-aaaar-qbxlq-cai pending_logs '()'

# admin_restart_bridging takes no arguments, so the payload is the empty candid tuple
export BLOB="$(didc encode --format blob '()')"

quill sns make-proposal --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE $PROPOSAL_NEURON_ID --proposal "(
    record {
        title = \"Execute admin_restart_bridging() to re-enable bridging on the one_bridge_canister canister\";
        url = \"https://1bridge.app/\";
        summary = \"One Bridge is refusing every bridge request with 'the bridge is temporarily disabled due to errors, please contact the administrator'. This proposal calls admin_restart_bridging() on dpjyw-raaaa-aaaar-qbxlq-cai to clear that state and resume bridging. It moves no funds.

Background: a BNB bridging task got stuck when a user's incoming transfer reverted on chain, having asked to bridge more tokens than their deposit address held. The version deployed at the time could not tell a reverted transaction from one that was merely unconfirmed, so it kept polling for a receipt that would never come, spent the canister's cycles on RPC calls, and once those calls started failing the error counter climbed to its limit of 42, which is what disables bridging.

v0.5.0, released in proposal 485, ends such a task instead of retrying it forever, and did so on the first finalization round after the upgrade: pending_logs() is now empty and the task is archived with its error, its amount and fee left out of the totals because nothing was bridged. Only error_rounds is still at 42, since it is not cleared when the queue drains, and that is what this call resets.\";
        action = opt variant {
            ExecuteGenericNervousSystemFunction = record {
                function_id = 1_308 : nat64;
                payload = ${BLOB};
            }
        };
    }
)" > proposal-message.json

# quill send proposal-message.json
