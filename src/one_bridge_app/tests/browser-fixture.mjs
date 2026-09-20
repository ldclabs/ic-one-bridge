// Local-only UI verification. All canister/ledger calls are replaced; no keys,
// real approvals or real transfers are used. Run from the repository root:
// node src/one_bridge_app/tests/browser-fixture.mjs
import { createServer } from 'vite'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
const root = fileURLToPath(new URL('../', import.meta.url))
process.chdir(root)
const actors = readFileSync(
  new URL('./fixtures/actors.ts', import.meta.url),
  'utf8'
)
const auth = `import { Principal } from '@icp-sdk/core/principal';
const makeIdentity = (id) => ({ isAuthenticated: true, getPrincipal: () => Principal.fromUint8Array(new Uint8Array([id, 1])) });
let identity = $state.raw(makeIdentity(1));
export const authStore = { get identity() { return identity }, async signIn() {}, async logout() { identity = makeIdentity(identity.getPrincipal().toUint8Array()[0] === 1 ? 2 : 1) } };`
const server = await createServer({
  root,
  configFile: root + 'vite.config.js',
  server: { host: '127.0.0.1', port: 4174, strictPort: true },
  plugins: [
    {
      name: 'local-bridge-fixture',
      enforce: 'pre',
      load(id) {
        if (id.endsWith('/lib/canisters/actors.ts')) return actors
        if (id.endsWith('/lib/stores/auth.svelte.ts')) return auth
        if (id.endsWith('/lib/utils/auth.ts'))
          return `import { authStore } from '$lib/stores/auth.svelte'; export const dynAgent = { get id() { return authStore.identity } };`
      },
      configureServer(server) {
        server.middlewares.use('/rpc', (req, res) => {
          let raw = ''
          req.on('data', (chunk) => (raw += chunk))
          req.on('end', () => {
            const body = JSON.parse(raw || '{}')
            const answers = {
              eth_chainId: '0x38',
              eth_getBalance: '0xde0b6b3a7640000',
              eth_call: '0x2386f26fc10000',
              eth_gasPrice: '0x5f5e100',
              eth_maxPriorityFeePerGas: '0x0'
            }
            res.setHeader('Content-Type', 'application/json')
            res.end(
              JSON.stringify({
                jsonrpc: '2.0',
                id: body.id,
                result: answers[body.method] ?? null
              })
            )
          })
        })
      }
    }
  ]
})
await server.listen()
server.printUrls()
