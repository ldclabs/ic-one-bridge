import { Principal } from '@icp-sdk/core/principal'
import { authStore } from '$lib/stores/auth.svelte'

const previous = JSON.parse(
  sessionStorage.getItem('bridge-fixture') || 'null',
  (_, value) => (value?.bigint ? BigInt(value.bigint) : value)
)
window.__bridgeFixture = previous
  ? {
      ...previous,
      requests: new Map(previous.requests),
      balances: new Map(previous.balances)
    }
  : {
      mode: 'ready',
      debits: 0,
      approvals: 0,
      calls: 0,
      requests: new Map(),
      balances: new Map()
    }
if (!document.getElementById('fixture-controls')) {
  const controls = document.createElement('aside')
  controls.id = 'fixture-controls'
  controls.style.cssText =
    'position:fixed;bottom:0;left:0;right:0;z-index:999;background:#122b32;color:#fff;padding:8px;font:12px monospace'
  controls.innerHTML =
    '<b>LOCAL FIXTURE — no real payments</b> <select id="fixture-mode" aria-label="Fixture mode"><option>ready</option><option>migration</option><option>ledger</option><option>keys</option><option>quota</option></select> <span id="fixture-status"></span>'
  document.body.appendChild(controls)
  document.getElementById('fixture-mode').onchange = (event) => {
    window.__bridgeFixture.mode = event.target.value
    window.dispatchEvent(new Event('bridge-activity'))
  }
}
const fixture = () => window.__bridgeFixture
const owner = () => authStore.identity.getPrincipal()
const token = Principal.fromText('druyg-tyaaa-aaaaq-aactq-cai')
const bridge = Principal.fromText('dpjyw-raaaa-aaaar-qbxlq-cai')
const feeLimits = {
  max_priority_fee_per_gas: 25_000_000_000n,
  max_hourly_fee: 500_000_000_000_000_000n,
  max_fee_per_gas: 500_000_000_000n,
  max_transaction_fee: 50_000_000_000_000_000n
}
const limits = {
  max_pending: 512,
  requests_per_user_hour: 12,
  max_pending_per_user: 32,
  min_cycles_reserve: 2_000_000_000_000n,
  requests_per_hour: 120,
  max_active_requests: 16
}
const plan = () => ({
  from: { Icp: null },
  to: { Evm: 'BNB' },
  fee: 10_000_000_000n,
  to_addr: [],
  user: owner(),
  ledger: token,
  amount: 1_000_000_000_000n
})
const now = () => BigInt(Date.now())
function status() {
  document.getElementById('fixture-status').textContent =
    `debits: ${fixture().debits}; calls: ${fixture().calls}; approvals: ${fixture().approvals}`
  sessionStorage.setItem(
    'bridge-fixture',
    JSON.stringify(
      {
        ...fixture(),
        requests: [...fixture().requests],
        balances: [...fixture().balances]
      },
      (_, value) =>
        typeof value === 'bigint' ? { bigint: String(value) } : value
    )
  )
}
status()
function state() {
  const mode = fixture().mode
  return {
    total_withdrawn_fees: 0n,
    error_rounds: 0n,
    min_threshold_to_bridge: 1_000_000_000_000n,
    token_symbol: 'PANDA',
    governance_canister: [],
    token_decimals: 8,
    token_bridge_fee: 10_000_000_000n,
    key_name: 'fixture',
    total_bridge_count: 25n,
    evm_address: '0x1111111111111111111111111111111111111111',
    svm_address: '11111111111111111111111111111111',
    evm_token_contracts: [
      ['BNB', ['0x2222222222222222222222222222222222222222', 8, 56n]]
    ],
    evm_providers: [['BNB', [3n, [location.origin + '/rpc']]]],
    evm_latest_gas: [['BNB', [now(), 100_000_000n, 0n]]],
    erc20_gas_limit: 84_000n,
    svm_providers: [],
    svm_token_address: [
      '11111111111111111111111111111111',
      0,
      '11111111111111111111111111111111'
    ],
    icp_address: bridge,
    finalize_bridging_round: [1n, false],
    total_bridged_tokens: 0n,
    total_collected_fees: 0n,
    token_ledger: token,
    token_logo: '/_assets/logo.webp',
    token_name: 'ICPanda',
    icp_collected_fees: 0n,
    sub_bridges: [],
    runtime: [
      {
        resource_limits: limits,
        pending_count: 25n,
        ledger_verified: mode !== 'ledger',
        keys_ready: [mode !== 'keys', false],
        migration_remaining: mode === 'migration' ? 312n : 0n,
        evm_provider_hosts: [['BNB', ['fixture']]],
        svm_provider_hosts: [],
        available_icp_fees: 0n,
        evm_fee_limits: [['BNB', feeLimits]],
        unresolved_operations: 2n,
        svm_mint_verified: false
      }
    ]
  }
}
function log(id, archived = false) {
  return {
    id: archived ? [BigInt(id)] : [],
    from: { Icp: null },
    to: { Evm: 'BNB' },
    fee: 10_000_000_000n,
    payout_started_at: 0n,
    to_tx:
      archived && id !== 0 ? [{ Evm: [true, new Uint8Array(32).fill(1)] }] : [],
    from_meta: [],
    to_addr: [],
    to_meta: [],
    user: owner(),
    from_tx: { Icp: [true, BigInt(id)] },
    created_at: now() - 30_000n,
    error:
      id === 2
        ? ['resolve legacy conflict operation 7 before resuming this task']
        : id === 3
          ? ['BNB: provider temporarily unavailable']
          : [],
    stuck: id === 2,
    icp_amount: 1_000_000_000_000n,
    finalized_at: archived ? now() - 5_000n : 0n,
    runtime: [
      {
        next_poll_at: now() + 10_000n,
        task_id: BigInt(id + 1),
        poll_attempts: 1,
        payout_attempt: [],
        payout_resolution: [],
        ledger: [token],
        payout_mined: false,
        error_chain: []
      }
    ]
  }
}
function operations() {
  return [
    {
      id: 7n,
      phase: { NeedsReview: 'conflicting historical deposit' },
      kind: 'legacy pending conflict',
      error: [
        'The same deposit appears in historical records. Governance reconciliation is required.'
      ],
      deposit: [],
      related_task: [log(2)]
    },
    {
      id: 6n,
      phase: { Submitted: null },
      kind: 'deposit',
      error: ['Awaiting ledger outcome. Continue the original operation.'],
      deposit: [plan()],
      related_task: []
    },
    {
      id: 5n,
      phase: { Prepared: null },
      kind: 'deposit',
      error: [],
      deposit: [plan()],
      related_task: []
    }
  ].map((op) => ({
    owner: owner(),
    created_at: now() - 60_000n,
    revision: 2n,
    request: [],
    signed_tx: [],
    reconciliation: [],
    ...op
  }))
}
const api = {
  async info() {
    return { Ok: state() }
  },
  async evm_address() {
    return { Ok: '0x3333333333333333333333333333333333333333' }
  },
  async svm_address() {
    return { Ok: '11111111111111111111111111111111' }
  },
  async my_pending_logs_page(take, after) {
    return {
      Ok: Array.from({ length: 25 }, (_, i) => log(i))
        .filter((l) => !after.length || l.runtime[0].task_id > after[0])
        .slice(0, take)
    }
  },
  async pending_logs_page(...args) {
    return api.my_pending_logs_page(...args)
  },
  async my_finalized_logs(take, before) {
    return {
      Ok: Array.from({ length: 25 }, (_, i) => log(24 - i, true))
        .filter((l) => !before.length || l.id[0] < before[0])
        .slice(0, take)
    }
  },
  async finalized_logs(...args) {
    return api.my_finalized_logs(...args)
  },
  async my_operations(take, before) {
    return {
      Ok: operations()
        .filter((op) => !before.length || op.id < before[0])
        .slice(0, take)
    }
  },
  async bridge_with_id(from, to, amount, recipient, id) {
    const f = fixture()
    f.calls++
    status()
    if (f.mode === 'quota')
      throw new Error('hourly request budget exhausted; retry in the next hour')
    const key = owner().toText() + ':' + Array.from(id).join(',')
    if (!f.requests.has(key)) {
      f.requests.set(key, { Icp: [true, 100n] })
      f.debits++
      f.balances.set(owner().toText(), 10_000n)
      status()
      throw new Error(
        'Fixture: response lost after debit. Continue the saved request.'
      )
    }
    return { Ok: f.requests.get(key) }
  },
  async my_bridge_log() {
    return { Ok: log(100, true) }
  },
  async resume_operation() {
    return { Ok: { Icp: [true, 100n] } }
  },
  async cancel_operation() {
    return { Ok: null }
  },
  async recheck_task() {
    return { Ok: null }
  },
  async icrc1_metadata() {
    return [['icrc1:fee', { Nat: 10_000n }]]
  },
  async icrc1_balance_of({ owner: account }) {
    return account.toText() === bridge.toText()
      ? 10_000_000_000_000_000n
      : (fixture().balances.get(account.toText()) ?? 1_000_000_020_000n)
  },
  async icrc2_allowance() {
    return { allowance: 10_000_000_000_000_000n, expires_at: [] }
  },
  async icrc2_approve() {
    fixture().approvals++
    status()
    return { Ok: 1n }
  }
}
export function createActor() {
  return api
}
