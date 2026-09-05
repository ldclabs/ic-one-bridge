#!/usr/bin/env bash

# Load the environment variables
source "$(pwd)"/proposals/env.sh

export CANISTERS_PATH="$(pwd)/debug"

# The WASM is the artifact built by the release workflow for the v0.5.2 tag, not
# a local build. Fetch it and check it against the hash published beside it:
#   gh release download v0.5.2 --repo ldclabs/ic-one-bridge \
#     --pattern 'one_bridge_canister.wasm.gz' --dir debug --clobber
#   shasum -a 256 "$CANISTERS_PATH/one_bridge_canister.wasm.gz"
#   # expect ad5f0814fb28d3cea0253450347ac20e888ae183929e916716bd86a1ef6c5dac
#
# And the hash the canister reports today, which should still be v0.5.1:
#   curl -s https://ic-api.internetcomputer.org/api/v3/canisters/dpjyw-raaaa-aaaar-qbxlq-cai | jq -r .module_hash
#
# No upgrade argument on purpose: every field of UpgradeArgs is optional and an
# absent one leaves the stored value alone, so token_bridge_fee (100 PANDA) and
# min_threshold_to_bridge (10,000 PANDA) survive the upgrade unchanged.
#
# erc20_gas_limit is new in v0.5.2 and has no stored value to leave alone, but
# absent it takes its default of 84,000 -- the literal v0.5.1 hard-codes -- so
# omitting it changes nothing either. Pass it only to move off that figure.

quill sns make-upgrade-canister-proposal $PROPOSAL_NEURON_ID --canister-ids-file ./sns_canister_ids.json --pem-file $PROPOSAL_PEM_FILE --target-canister-id "dpjyw-raaaa-aaaar-qbxlq-cai" --wasm-path "$CANISTERS_PATH/one_bridge_canister.wasm.gz" --mode upgrade --title "Upgrade one_bridge_canister canister to v0.5.2" --summary "This proposal upgrades dpjyw-raaaa-aaaar-qbxlq-cai from v0.5.1 to v0.5.2. The canister runs module de2b54e82a431b330cffa5fc92bdf3050651aaa06014655762b16423c606ed69 today; this installs ad5f0814fb28d3cea0253450347ac20e888ae183929e916716bd86a1ef6c5dac, the artifact the release workflow built from the v0.5.2 tag and published with its SHA-256.

No upgrade argument is passed, so the bridge fee and the minimum bridge amount keep their current values, and the ERC-20 gas limit that v0.5.2 turns into a setting takes its default of 84,000 -- the figure v0.5.1 hard-codes, against a measured transfer cost of about 53,700. The Candid interface only gains record fields and trailing optional arguments, so existing callers are unaffected.

The release is the outcome of a security review of the bridging path.

Trust in the RPC providers: every answer a payout depends on -- receipts and their Transfer events, block heights, nonces, signature statuses, balances -- is now asked of two providers and acted on only when they agree, or on the more conservative of the two readings. Broadcasts and gas prices still take the first answer. A chain therefore needs at least two providers; BNB Chain's three already qualify.

Funds: a deposit is credited only once the receipt's Transfer events show the expected amount reached the bridge, and an ICP payout carries a ledger dedup key so a retry comes back as a duplicate instead of paying twice. A payout is refused to the bridge's own addresses, the token contracts, the ledger, the zero address and the anonymous principal, EVM addresses are checked against their EIP-55 checksum, and an amount finer than a chain's decimals can carry is rejected rather than silently floored.

Liveness: a task records the deadline and the signed bytes of its transactions, so one no provider has seen is broadcast again and one that can no longer land is abandoned or rebuilt. A task that fails on its own account is marked stuck for an administrator instead of blocking its chain, and the error circuit breaker now cools down hourly and lifts itself after a clean round instead of pausing the bridge until a proposal passes.

Signing: every user-triggered signing path checks the derived address's balances before spending a threshold signature, and they share a per-user lock so two requests cannot race for the same nonce.

Administration: admin_retry_bridging_task can redirect a task to another target, including back to the deposit chain as a refund; admin_close_bridging_task needs force while a payout is still in flight; admin_init_public_keys fetches a missing master key; and the validate_ methods are controller-only." --url "https://github.com/ldclabs/ic-one-bridge/releases/tag/v0.5.2" > proposal-message.json

# quill send proposal-message.json
