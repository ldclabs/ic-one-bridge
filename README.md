# 🌉 `One Bridge Canister`

A fully on-chain token bridge running as an Internet Computer canister. It moves a token between the
IC, Solana, and EVM networks (BNB Chain, Ethereum, Base, ...) without any off-chain relayer: the
canister reads the remote chains through HTTPS outcalls and signs remote transactions itself with
threshold ECDSA (secp256k1) and threshold Schnorr (Ed25519).

Each token gets its own canister instance, holding the whole non-IC supply and locking or releasing
against it. ICPanda's PANDA is the reference deployment; see [token_listing.md](./token_listing.md)
to have a token listed.

- Web app: https://1bridge.app/ (`ejwdq-iyaaa-aaaap-an47q-cai`)
- PANDA bridge canister: `dpjyw-raaaa-aaaar-qbxlq-cai`, controlled by the ICPanda SNS DAO
- Bridge info as JSON (or CBOR with `Accept: application/cbor`), served over HTTP without response
  certification: https://dpjyw-raaaa-aaaar-qbxlq-cai.icp0.io/

## How it works

**Every user has a deposit address on every chain.** The canister derives one address per caller
principal — an EVM address from its threshold ECDSA key and a Solana address from its threshold
Ed25519 key — and it can sign for them. Read them with `evm_address` / `svm_address`.

**Bridging in.** `bridge(from_chain, to_chain, amount, opt to)` takes the amount in the IC ledger's
decimals and converts it to the destination chain's decimals.

| `from_chain` | what the canister does |
| --- | --- |
| `"ICP"` | `icrc2_transfer_from` on the token ledger, so the caller must `icrc2_approve` the canister first |
| `"BNB"`, other EVM chains | signs an ERC-20 transfer from the caller's derived EVM address into the bridge's own address, so the caller must fund that address first (with the token, and with enough native gas) |
| `"SOL"` | the same, as an SPL transfer from the caller's derived Solana address |

**Bridging out.** The task goes on a pending queue and a finalization round waits for the incoming
transaction to reach finality, then pays out `amount - token_bridge_fee` on the destination chain:
to `to` when given, otherwise to the caller's own principal or derived address. Payouts are
deduplicated so a retry cannot pay twice. Polling is paced by finality — every 3s for the first
minute, then backing off to 15s, 60s and 5min while nothing advances.

**What a round trusts.** HTTPS outcalls deliberately use non-replicated mode to control cost.
Controllers configure independent official providers, and financial evidence needs two independent
provider identities and agreeing or conservative results. This also trusts the IC replicas serving
those outcalls; two provider URLs do not authenticate a response against a malicious replica.
A Solana blockhash candidate is checked by two providers, and expiry uses that actual hash and its
finalized context, never just a provider's advertised last-valid height. EVM receipts carry their
block hash, are checked against the canonical chain, and must prove the expected token transfer.

**Recovering payments.** Every ledger transfer and bridge signature has a durable operation record
before the external call. An unknown ledger result retains its original request and reservation;
retries use the same timestamp and memo. Use `bridge_with_id` with a stable request ID for retries
across lost replies, `my_operations` to inspect the operation, and `resume_operation` to recover it.
The original `bridge` API can also resume a matching unresolved deposit. Never reset a payout merely
because an RPC cannot currently find it: ordinary retry refuses unresolved attempts, while explicit
governance reconciliation records the evidence and exact operation revision.

**Guards.** `bridge` rejects amounts below `min_threshold_to_bridge` or with more precision than
the source chain carries, refuses the bridge's own addresses, the token contracts and the anonymous
principal as destinations, refuses a second unconfirmed EVM deposit from the same user on the same
chain, and refuses new tasks on a chain whose providers are failing. Before signing for a user's
derived address it checks that the address can pay for the transaction. A task that fails on its
own account — a payout refused on chain, a deposit that delivered less than it claimed — is marked
stuck for an administrator and blocks nothing else. After too many consecutive rounds with
provider errors, new tasks are paused and the rounds slow to an hourly cooldown; a clean round or
`admin_restart_bridging` lifts the pause.

Track a task with `my_bridge_log(from_tx)`, `my_pending_logs()` and `my_finalized_logs(take, prev)`.
Pending results are bounded; use `my_pending_logs_page(take, after_task_id)` or
`pending_logs_page(take, after_task_id)` for later pages. New optional `runtime` fields carry task IDs,
readiness, resource limits and operating-fee accounting without requiring older bridge instances to
return those fields.

Public signing/RPC work is subsidized within configurable quotas: 12 requests per user and 120
requests globally per IC clock hour by default, with a 2T-cycle reserve plus 100B cycles of headroom
for each active subsidized request. Governance can change the quotas and base reserve through
`admin_set_resource_limits`. EVM fees also have per-chain transaction/hour caps through
`admin_set_evm_fee_limits`; two provider quotes are reconciled upward before those caps are applied.


## Repository layout

| Path | What |
| --- | --- |
| [src/one_bridge_canister](./src/one_bridge_canister) | the bridge canister (Rust) |
| [src/one_bridge_app](./src/one_bridge_app) | the web app (SvelteKit), deployed as an asset canister |
| [evm_contracts](./evm_contracts) | the flattened ERC-20 source deployed on the EVM side |
| [proposals](./proposals) | `quill` scripts for the SNS proposals that operate the canister |
| [sns_functions.md](./sns_functions.md) | SNS generic function ids for the admin methods |

## Development

```bash
make lint   # cargo fmt + clippy
make test   # cargo test --workspace
make integration-test  # local PocketIC ledger/RPC, callback traps, watchdog and upgrades

make build-wasm  # cargo build --release --target wasm32-unknown-unknown
make build-did   # regenerate the .did from the wasm, then dfx generate
```

Releases are built by [.github/workflows/release.yml](./.github/workflows/release.yml) on a `v*`
tag, in a digest-pinned Rust build container, with a pinned Rust toolchain, `ic-wasm` and `wasm-opt`,
and gzip timestamps disabled for reproducible artifacts. Each release
publishes `one_bridge_canister.wasm.gz` next to a file naming its SHA-256, which is what upgrade
proposals reference.

## Quick Start

### Local Deployment

```bash
dfx canister create --specified-id dpjyw-raaaa-aaaar-qbxlq-cai one_bridge_canister
# deploy with default settings (key_name = "dfx_test_key")
dfx deploy one_bridge_canister
```

### Mainnet Deployment

#### 1. Deploy the canister to subnet `pzp6e`:
```bash
dfx deploy one_bridge_canister --argument "(opt variant {Init =
  record {
    key_name = \"key_1\";
    token_name = \"ICPanda\";
    token_symbol = \"PANDA\";
    token_decimals = 8;
    token_logo = \"https://532er-faaaa-aaaaj-qncpa-cai.icp0.io/f/374?inline&filename=1734188626561.webp\";
    token_ledger = principal \"druyg-tyaaa-aaaaq-aactq-cai\";
    token_bridge_fee = 10_000_000_000;
    min_threshold_to_bridge = 1_000_000_000_000;
    governance_canister = opt principal \"dwv6s-6aaaa-aaaaq-aacta-cai\";
  }
})" --ic --subnet pzp6e-ekpqk-3c5x7-2h6so-njoeq-mt45d-h3h6c-q3mxf-vpeq5-fk5o7-yae
```

`token_bridge_fee` and `min_threshold_to_bridge` are in the ledger's decimals — the values above are
100 PANDA and 10,000 PANDA. Neither has a setter: changing them later means an upgrade that passes
new `UpgradeArgs`, and an omitted field keeps its stored value.

`erc20_gas_limit` is the gas limit of the token's ERC-20 `transfer` on every EVM chain. It is
optional and defaults to 84,000, which fits a plain OpenZeppelin token; a token with more logic in
its transfer needs a higher one, set the same way, at init or through `UpgradeArgs`.

#### 2. Check info:
```bash
dfx canister call one_bridge_canister info '()' --ic
dfx canister call one_bridge_canister evm_address '(null)' --ic
dfx canister call one_bridge_canister svm_address '(null)' --ic
```

#### 3. Set EVM providers (e.g. BNB Chain Mainnet):
```bash
# chain_name = "BNB", max_confirmations = 3 (must be >= 2)
dfx canister call one_bridge_canister admin_set_evm_providers '("BNB", 3, vec { "https://bsc-dataseed.bnbchain.org"; "https://bsc.nodereal.io"; "https://bsc-dataseed.nariox.org" })' --ic
```

The reads a payout depends on need two agreeing providers, so at least two are required and
three keep the bridge working while one is down; only `https` URLs are accepted. `max_confirmations`
is how many blocks a transaction has to be behind the tip, or `0` to rely on the chain's own
`finalized` block tag, which is the safer choice on every chain that supports it.

#### 4. Add the EVM contract (e.g. BNB Chain PANDA token):
```bash
# chain_name = "BNB" (uppercase, at most 8 chars), chain_id = 56
# contract_address must be EIP-55 checksummed
dfx canister call one_bridge_canister admin_add_evm_contract '("BNB", 56, "0xe74583edAFF618D88463554b84Bc675196b36990")' --ic
```

The canister verifies the chain id against the providers and reads the contract's `decimals()`
itself. **Other EVM chains (Ethereum, Base, Avalanche...) are added the same way.**

#### 5. Add the Solana side (optional):
```bash
dfx canister call one_bridge_canister admin_set_svm_providers '(vec { "https://api.mainnet-beta.solana.com"; "https://solana-rpc.publicnode.com" })' --ic
# the SPL mint address; the canister reads its decimals and token program
dfx canister call one_bridge_canister admin_add_svm_contract '("<SPL mint address>")' --ic
```

#### Operating fees and browser RPCs

`info().evm_providers` and `info().svm_providers` expose only public browser endpoints. Anonymous
origin URLs continue to work automatically. If internal URLs contain paths or credentials, publish
separate anonymous endpoints with `admin_set_public_providers(chain, urls)`; do not publish API keys.
Core canister RPC configuration remains separate and still requires two independent providers.

Solana bridging supports the classic Token program and Token-2022 mints without mint extensions.
Transfer-fee/hook and other extension semantics must not enter nominal-amount bridge accounting.
Previously configured mints are revalidated during upgrade. Users can still withdraw a legacy
extension mint to an already-created recipient ATA where the existing transfer instruction supports it.

The ICP ledger fee is paid from an explicitly tracked operating budget. A sponsor can approve the
canister on the token ledger and call `fund_ledger_fees(amount)` to fund it. Earned, verified ICP-side
bridge fees can also pay those costs. New accounting excludes unknown historical expenses: after
upgrading from older code, governance must reconcile backing before using
`admin_recognize_legacy_fees(total_verified_legacy_fees, evidence)`. The argument is a cumulative
verified total, so repeating the same proposal cannot credit it twice.

`/config` (JSON) and `/config.cbor` provide cached, certified configuration and funds addresses.
The legacy HTTP `/` endpoint remains an uncertified operational view. Check initialization flags
before using an address, and use exact integer/CBOR handling for token quantities.

#### 6. Bridge 10,000 PANDA from ICP to BNB Chain:
- 6.1. The whole PANDA supply on BNB Chain should be held by the bridge canister's EVM address at
  initialization.
- 6.2. That address needs enough native gas (BNB) to pay for the payout transactions.
- 6.3. The user must approve the canister to spend PANDA on their behalf (`icrc2_approve`).

```bash
# from_chain = "ICP", to_chain = "BNB"
# amount = 1_000_000_000_000 (10,000 PANDA with 8 decimals)
# to = null pays out to the caller's own derived EVM address
dfx canister call one_bridge_canister bridge '("ICP", "BNB", 1_000_000_000_000, null)' --ic

# check pending transfers
dfx canister call one_bridge_canister my_pending_logs '()' --ic

# after some time, check finalized transfers (take, prev)
dfx canister call one_bridge_canister my_finalized_logs '(10, null)' --ic
```

Bridging back is the mirror image: send the tokens to the address `evm_address '(null)'` returns,
leave enough BNB there for one transfer, then call
`bridge '("BNB", "ICP", 1_000_000_000_000, null)'`.

## Operating the bridge

The admin methods are guarded by `is_controller`, so on mainnet the SNS DAO calls them through
proposals. Each one has a `validate_*` twin that renders the arguments for voters, and the generic
function ids are listed in [sns_functions.md](./sns_functions.md). The scripts under
[proposals/](./proposals) are the working examples, including canister upgrades.

| Method | When |
| --- | --- |
| `admin_restart_bridging` | bridging is paused after too many failing rounds; clears the counter and re-arms the timer right away instead of waiting for the cooldown |
| `admin_retry_bridging_task` | a task is stuck: its payout demonstrably moved no funds and must be rebuilt, optionally to another target — a corrected address, or the chain the deposit came from as a refund |
| `admin_close_bridging_task` | a task cannot be retried and is archived as not bridged; refused while its payout may still land unless forced |
| `admin_init_public_keys` | a master key could not be fetched at install or upgrade; bridging that needs it is refused until it is there |
| `admin_collect_fees` | withdraw the fees held on the ICP ledger to a principal |
| `admin_add_bridges` / `admin_remove_bridges` | manage the canisters allowed to call `evm_sign` |

## API Reference

The generated Candid file is the authoritative interface:
[one_bridge_canister.did](./src/one_bridge_canister/one_bridge_canister.did).

- Read state and addresses: `info`, `evm_address`, `svm_address`.
- Bridge and recover: `bridge`, `bridge_with_id`, `my_operations`, `resume_deposit`,
  `resume_operation`, `cancel_operation`, `recheck_task`.
- Read history: `my_bridge_log`, `my_pending_logs`, `my_pending_logs_page`, `pending_logs`,
  `pending_logs_page`, `my_finalized_logs`, `finalized_logs`.
- Withdraw from a user's derived wallet: `erc20_transfer`, `erc20_transfer_tx`,
  `evm_transfer_tx`, `spl_transfer_tx`, `sol_transfer_tx`.
- Fund ledger operating costs: `fund_ledger_fees`.
- Governance configuration and recovery: the existing admin methods plus
  `admin_set_resource_limits`, `admin_set_evm_fee_limits`, `admin_set_public_providers`,
  `admin_recheck_task`, `admin_resolve_operation`, `admin_resolve_legacy_payout`,
  `admin_recognize_legacy_fees`, and their `validate_*` counterparts.

See [the migration and recovery guide](./docs/reviews/remediation-2026-09-06.md) for defaults,
manual reconciliation preconditions and downgrade restrictions.

Full Candid API definition: [one_bridge_canister.did](./src/one_bridge_canister/one_bridge_canister.did)

## License
Copyright © 2024-2026 [LDC Labs](https://github.com/ldclabs).

`ldclabs/ic-one-bridge` is licensed under the MIT License. See [LICENSE](./LICENSE) for the full
license text.
