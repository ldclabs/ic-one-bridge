#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

export CANISTERS_PATH="$(pwd)/debug"

# The WASM is the artifact built by the release workflow for the v0.5.1 tag, not
# a local build. Fetch it and check it against the hash published beside it:
#   gh release download v0.5.1 --repo ldclabs/ic-one-bridge \
#     --pattern 'one_bridge_canister.wasm.gz' --dir debug --clobber
#   shasum -a 256 "$CANISTERS_PATH/one_bridge_canister.wasm.gz"
#   # expect de2b54e82a431b330cffa5fc92bdf3050651aaa06014655762b16423c606ed69
#
# And the hash the canister reports today, which should still be v0.5.0:
#   curl -s https://ic-api.internetcomputer.org/api/v3/canisters/dpjyw-raaaa-aaaar-qbxlq-cai | jq -r .module_hash
#
# No upgrade argument on purpose: every field of UpgradeArgs is optional and an
# absent one leaves the stored value alone, so token_bridge_fee (100 PANDA) and
# min_threshold_to_bridge (10,000 PANDA) survive the upgrade unchanged.

quill sns make-upgrade-canister-proposal $PROPOSAL_NEURON_ID --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE --target-canister-id "dpjyw-raaaa-aaaar-qbxlq-cai" --wasm-path "$CANISTERS_PATH/one_bridge_canister.wasm.gz" --mode upgrade --title "Upgrade one_bridge_canister canister to v0.5.1" --summary "This proposal upgrades dpjyw-raaaa-aaaar-qbxlq-cai from v0.5.0 to v0.5.1. The canister runs module b56c1fef6e0225539520183f006c28055256011a8fec79979041b879753ad655 today; this installs de2b54e82a431b330cffa5fc92bdf3050651aaa06014655762b16423c606ed69, the artifact the release workflow built from the v0.5.1 tag and published with its SHA-256.

The Candid interface is unchanged, and no upgrade argument is passed, so the bridge fee and the minimum bridge amount set by the earlier proposal keep their current values.

Correctness fixes:

- eth_sendRawTransaction's result is no longer decoded as a string. A provider that answered with null or an unexpected shape turned a transaction already on its way to the mempool into a failed broadcast; on the deposit path that moved a user's tokens to the bridge with no task recording it. No caller reads the value, since they all know the hash of the transaction they signed.
- EVM receipt quantities are decoded as alloy U64 and accept both the hex-string and the bare-number form. A decode failure after a 2xx response does not fail over to another provider, so the stricter hand-rolled parse could wedge a chain permanently.
- ICP payouts are deduplicated at the ledger: the transfer now carries created_at_time and a memo derived from the incoming transaction, so a stale-lock takeover racing a still-in-flight transfer gets Duplicate back instead of paying the recipient twice, and Duplicate is treated as success. Ledger transfers also move from bounded_wait to unbounded_wait, because a bounded call that times out reports an unknown outcome that the next round would rebuild into a second payout.
- A finalization round reserves a task's outgoing transaction slot before it broadcasts, and a round that has been superseded by a stale-lock takeover refuses to merge its results, release the lock, or touch the replacement round's timer. Progress is measured against the state each task was in when the round started.
- The finalization timer reads the current round when it fires rather than capturing one at scheduling time, so a surviving timer can no longer leave a non-empty queue with no timer armed.
- Transport and address-derivation errors on the ICP path are prefixed with the chain name, so the stuck-task gate in bridge() recognises them.

Cycle and latency work:

- An HTTPS outcall is billed on max_response_bytes, the bytes it reserves rather than the bytes that come back, and neither RPC client set it: every call reserved the 2 MB default, about 20.8 billion cycles apiece on a 13-node subnet, and an EVM confirmation poll spent two of those. Every method now names its own budget.
- That poll no longer fetches the block height until a receipt exists, since the height cannot change the answer before the transaction is mined, and a failed block-height sweep is cached for the round instead of being retried by every task parked on it.
- The finalization poll is paced by finality instead of firing every second: the first tier now polls every 3 seconds for about a minute, past the slowest finality the bridge waits on, then backs off to 15 seconds, 60 seconds and 5 minutes.
- my_bridge_log no longer decodes a user's entire archive out of stable memory to answer; the scan is capped and reports an honest miss beyond it, with my_finalized_logs paging older history.

Alongside these, duplicated RPC, transaction-building and key-initialisation code is written once and unused RPC methods are removed, which shrinks the WASM." --url "https://github.com/ldclabs/ic-one-bridge/releases/tag/v0.5.1" > proposal-message.json

# quill send proposal-message.json
