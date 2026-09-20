#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

# build and get batch_id, evidence:
# dfx deploy one_bridge_app --ic --by-proposal

export BLOB="$(didc encode --format blob '(record {batch_id=8:nat; evidence=blob "\c2\9b\b3\b2\ed\a1\53\4b\14\0f\79\10\36\33\a2\ae\9a\08\8b\9c\1d\a8\2c\ce\e1\be\24\c0\7c\e4\d2\c4"})')"

quill sns make-proposal --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE $PROPOSAL_NEURON_ID --proposal "(
    record {
        title = \"Execute commit_proposed_batch() to release one_bridge_app v0.6.1\";
        url = \"https://1bridge.app/\";
        summary = \"This proposal executes commit_proposed_batch() on ejwdq-iyaaa-aaaap-an47q-cai to release one_bridge_app v0.6.1, the front end for one_bridge_canister v0.6.1.

Recoverable bridge requests:

- Every new bridge request receives a random stable request ID and is saved in the browser before submission. If a reply is lost or the page is reloaded, continuing the saved request asks the canister for the same operation instead of creating another deposit. A submitted retry does not repeat the new-deposit balance and ledger-allowance preparation.
- Saved requests are scoped to the signed-in principal and bridge canister, coordinated across browser tabs, and keep their selected token and chains across reloads. Account changes stop in-flight UI work from being attached to the wrong identity.

Activity and recovery:

- My activity now pages pending bridge tasks, durable operations and finalized history. It exposes the canister's safe owner actions to resume an operation, cancel one that has not executed, or schedule another confirmation check.
- Operations that need external accounting or governance reconciliation are identified as such and cannot be presented as ordinary retries or cancellations. Pending, retrying, completed, closed, stuck and reconciliation states are shown separately.
- Public pending and finalized activity is paginated as well, so the page no longer truncates the bridge to one recent combined list.

Readiness and resource limits:

- The form reads the v0.6.1 runtime status and pauses new deposits while financial-history migration, ledger or Solana mint verification, signing-key initialization, the error circuit breaker, or pending-capacity limits require it. Existing recovery remains available during those states.
- The configured per-user and global request limits, pending limits, key and metadata readiness, migration progress, unresolved-operation count and per-chain EVM fee ceilings are visible in the interface.
- Browser RPC clients use only the public endpoints exposed by the canister, refresh when provider configuration changes, and prefer the canister's recent two-provider gas quote. When that quote is stale, estimates reconcile independent public-provider readings conservatively and warn when the configured transaction fee ceiling would be exceeded.

Amount and wallet correctness:

- Token amounts stay as decimal text until they are converted directly to bigint, avoiding JavaScript floating-point rounding. The form checks the source and post-fee destination precision before submission and formats large balances without converting them to Number.
- Wallet transfers use the native-transfer gas limit when sending a chain's native coin, avoid irrelevant token-balance reads, and stop their confirmation polling after settlement or an account change.

The app can still display older bridge canisters, but creating new bridge requests requires the v0.6.1 recoverable-request interface.\";
        action = opt variant {
            ExecuteGenericNervousSystemFunction = record {
                function_id = 1_300 : nat64;
                payload = ${BLOB};
            }
        };
    }
)" > proposal-message.json

# quill send proposal-message.json
