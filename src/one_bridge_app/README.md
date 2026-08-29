# 🌉 `One Bridge Web App`

The web front end for [One Bridge](../../README.md): a SvelteKit app built to a static SPA and
served from an asset canister on the Internet Computer.

- Live: https://1bridge.app/ (`ejwdq-iyaaa-aaaap-an47q-cai`)
- Talks to the PANDA bridge canister `dpjyw-raaaa-aaaar-qbxlq-cai`, plus every canister listed in
  its `sub_bridges`, so one page serves all the listed tokens

## What it does

Users sign in with Internet Identity (`https://id.ai` in production, the local II canister in
development). The app then reads the bridge canister's state to show the user's derived EVM and
Solana deposit addresses, submits `bridge` calls, and follows the pending and finalized logs.

Chain balances are read in the browser straight from the RPC providers the canister publishes in
`info()` — the app holds no keys and no backend of its own.

## Develop

Requires Node >= 22 and pnpm. `dfx deploy` writes the canister ids this app reads into `.env` at the
repository root, so deploy the canisters first.

```bash
pnpm install            # from the repository root
pnpm --filter one_bridge_app dev     # vite dev server, /api proxied to 127.0.0.1:4943
pnpm --filter one_bridge_app check   # svelte-check
pnpm --filter one_bridge_app build   # static output in ./build
```

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
