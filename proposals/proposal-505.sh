#!/usr/bin/env bash
set -euo pipefail

# Load the environment variables
source "$(pwd)"/proposals/env.sh

CANISTERS_PATH="$(pwd)/debug"
export CANISTERS_PATH

# The WASM is the artifact built by the release workflow for the v0.6.1 tag, not
# a local build. Fetch it and check it against the hash published beside it:
#   gh release download v0.6.1 --repo ldclabs/ic-one-bridge \
#     --pattern 'one_bridge_canister.wasm.gz' --dir debug --clobber
#   shasum -a 256 "$CANISTERS_PATH/one_bridge_canister.wasm.gz"
#   # expect e1dd9e33b76630db219d50a514931eefd4bd1eef6772f40ca7e07c32a72d3923
#
# On 2026-09-20 the live module hash still matched v0.5.2. Recheck before sending:
#   curl -s https://ic-api.internetcomputer.org/api/v3/canisters/dpjyw-raaaa-aaaar-qbxlq-cai | jq -r .module_hash
#   # expect ad5f0814fb28d3cea0253450347ac20e888ae183929e916716bd86a1ef6c5dac
#
# No upgrade argument on purpose: the stored bridge fee, minimum bridge amount,
# ERC-20 gas limit, provider settings and existing keys are retained. The new
# resource_limits setting takes its defaults when absent from the old state.
# After execution, info().runtime reports migration_remaining and ledger/key/
# Solana mint readiness. Admission and payouts wait for financial migration;
# the relevant metadata checks must also pass before a bridge can use them.

# Refuse a stale or locally rebuilt artifact before generating the proposal.
printf '%s  %s\n' \
  'e1dd9e33b76630db219d50a514931eefd4bd1eef6772f40ca7e07c32a72d3923' \
  "$CANISTERS_PATH/one_bridge_canister.wasm.gz" | shasum -a 256 --check

quill sns make-upgrade-canister-proposal "$PROPOSAL_NEURON_ID" --canister-ids-file ./sns_canister_ids.json --pem-file "$PROPOSAL_PEM_FILE" --target-canister-id "dpjyw-raaaa-aaaar-qbxlq-cai" --wasm-path "$CANISTERS_PATH/one_bridge_canister.wasm.gz" --mode upgrade --title "Upgrade one_bridge_canister canister to v0.6.1" --summary "This proposal upgrades dpjyw-raaaa-aaaar-qbxlq-cai from v0.5.2 to v0.6.1. The live module hash checked on 2026-09-20 is ad5f0814fb28d3cea0253450347ac20e888ae183929e916716bd86a1ef6c5dac; this installs e1dd9e33b76630db219d50a514931eefd4bd1eef6772f40ca7e07c32a72d3923, the artifact the release workflow built from the v0.6.1 tag and published with its SHA-256.

No upgrade argument is passed, so the stored bridge fee, minimum bridge amount, ERC-20 gas limit, provider configuration and existing signing keys keep their current values. The Candid changes add methods and optional fields while retaining the existing method signatures.

Payment recovery and duplicate protection:

- Ledger transfers and signatures for bridge deposits and payouts are recorded in a durable operation journal before the external call. An unknown ledger outcome retains its original request, timestamp and memo; a retry does not create a fresh payment, and unresolved fee withdrawals retain their reservations. Stale callbacks cannot overwrite a newer operation revision.
- bridge_with_id lets a caller retry with a stable request ID after a lost reply. Operation queries and resume methods expose and recover unfinished work; the existing bridge method can also resume a matching unresolved deposit.
- Legacy tasks and archives are indexed in bounded batches. Duplicate source transactions remain under a durable reconciliation hold, including sources already archived, and ordinary retry or recheck cannot clear that hold. Explicit governance reconciliation requires the exact operation revision and evidence.

Remote-chain confirmation:

- Financial decisions require independent provider identities. EVM receipts are checked against canonical block hashes and the expected token movement; replacement nonces use finalized evidence. Solana checks the actual blockhash and finalized context before treating a transaction as expired, and verifies the expected token movement.
- EVM canonical and finalized block reads prefer compact headers. A provider that explicitly reports the header method unsupported may fall back to a full block read with a bounded response budget; oversized or disagreeing responses do not bypass confirmation.

Resource controls and settlement:

- New default limits allow 12 subsidized requests per user and 120 globally per IC clock hour, at most 16 active requests, 512 pending tasks and 32 pending tasks per user, with a 2T-cycle base reserve and additional headroom per active request. Governance can adjust these limits and per-chain EVM transaction/hour fee caps.
- Pending work uses stable indexes, bounded batches and paginated queries. Finalization recovery keeps settlement progressing after interrupted rounds while preserving unresolved payment records and reconciliation holds.
- Token ledger transfer fees are paid directly from the bridge account. Payouts do not require historical ICP-origin bridge fee income or a separate ledger fee budget. Fee withdrawals retain the ICP-origin income ceiling, payment deduplication and withdrawal reservations.

Upgrade readiness and governance:

The upgrade preserves the existing stable-memory IDs and migrates legacy financial history before admitting or paying new bridge tasks. It rechecks ledger and Solana mint metadata, retains existing keys and retries missing-key initialization. The optional runtime information exposes migration progress and readiness.

The recovery and configuration methods have governance validators. Their SNS function registrations are separate proposals in scripts 506-512; this proposal only upgrades the canister code. Public browser RPC endpoints are configured separately from private core provider URLs." --url "https://github.com/ldclabs/ic-one-bridge/releases/tag/v0.6.1" > proposal-message.json

# quill send proposal-message.json
