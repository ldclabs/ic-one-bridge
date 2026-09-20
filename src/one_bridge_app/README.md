# 🌉 `One Bridge Web App`

The web front end for [One Bridge](../../README.md): a SvelteKit app built to a static SPA and
served from an asset canister on the Internet Computer.

- Live: https://1bridge.app/ (`ejwdq-iyaaa-aaaap-an47q-cai`)
- Talks to the PANDA bridge canister `dpjyw-raaaa-aaaar-qbxlq-cai`, plus every canister listed in
  its `sub_bridges`, so one page serves all the listed tokens

## What it does

Users sign in with Internet Identity (`https://id.ai` in production, the local II canister in
development). The app then reads the bridge canister's state to show the user's derived EVM and
Solana deposit addresses, submits `bridge_with_id` calls, and follows pending tasks, durable operations
and finalized logs. The app targets canister v0.6.1; older canisters can still be browsed, but creating
new bridges waits for the recoverable-request interface to become available.

Each bridge request is saved to local storage before submission, scoped to the signed-in principal
and canister. The browser must support Web Locks on HTTPS (or localhost) so tabs cannot race
when creating or clearing a request. Retries reuse the original 32-byte request ID and parameters. A request already submitted
to the canister bypasses the new-deposit balance and allowance checks. An accepted request remains
available until the user explicitly starts another bridge. Do not clear an unknown request before
checking its outcome: removing the browser shortcut does not cancel a payment.

**My activity** pages pending tasks, operations and finalized history independently. Users can resume
an operation, cancel a safely unexecuted operation, or schedule a confirmation recheck. The canister
is authoritative for all actions; reconciliation holds require governance and cannot be cleared here.
A completed deposit operation means the deposit was received, not that its destination payout has
completed. The original transfer can be followed to its final task status.

The app refreshes migration, ledger/mint verification, key readiness and queue status. New-deposit
controls respect those states without blocking recovery. Configured request and gas limits are shown;
remaining hourly usage is not available from the API. Gas estimates prefer a recent canister quote,
otherwise use conservative public-provider quotes, and remain estimates because the canister may use
different private providers. Monetary input stays decimal text and is converted directly to bigint.

A sub-bridge holds its own token, ledger and logs, but not its own keys: the main canister derives
the deposit addresses and signs the transfers for all of them, so the app reads the user's addresses
from the main bridge and reuses them for every token. Sub-bridge development is paused, so
`sub_bridges` is empty in production today and the app effectively runs against the main bridge
alone.

Chain balances are read in the browser straight from the RPC providers the canister publishes in
`info()` — the app holds no keys and no backend of its own.

## Develop

Requires Node >= 22.6 and pnpm. The bridge and Internet Identity canister ids are compiled in from
[`src/lib/constants.ts`](src/lib/constants.ts); point them at a local deployment to develop against
`dfx`.

```bash
pnpm install            # from the repository root
pnpm --filter one_bridge_app dev     # vite dev server, /api proxied to 127.0.0.1:4943
pnpm --filter one_bridge_app check   # svelte-check
pnpm --filter one_bridge_app test    # request recovery, readiness and fee regressions
pnpm --filter one_bridge_app build   # static output in ./build
```

For local UI verification with simulated canister and ledger calls, run:

```bash
node src/one_bridge_app/tests/browser-fixture.mjs
```

Open `http://127.0.0.1:4174/`. The fixture deliberately loses the first bridge response after a simulated
debit. Reload and continue the saved request: the debit counter must stay at one. It also provides
migration/readiness/limit states, governance holds and multiple pages of activity. The fixture is a
separate local server; its actor replacements are never part of the production build.

## Deploy

`dfx.json` does not build this app, so run `pnpm build` first. On mainnet the asset canister is
controlled by the SNS DAO, which means uploading and committing are two separate steps:

```bash
# uploads the batch and prints its batch_id and evidence
dfx deploy one_bridge_app --ic --by-proposal
```

The batch is then committed by an SNS proposal calling `commit_proposed_batch` with those two
values — see [../../proposals/proposal-486.sh](../../proposals/proposal-486.sh) for the script.

`static/.well-known/ic-domains` carries the custom domain and `static/.ic-assets.json` the response
headers and CSP; both are part of the uploaded assets.
