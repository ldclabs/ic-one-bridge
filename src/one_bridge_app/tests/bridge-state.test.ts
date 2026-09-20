import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  canRecheck,
  logSettled,
  operationActions,
  operationStatus,
  readinessReason
} from '../src/lib/utils/bridge-state.ts'
import { evmFee, providerIdentity } from '../src/lib/utils/bridge-fees.ts'
import { TokenDisplay } from '../src/lib/utils/token.ts'
import type {
  StateInfo,
  BridgeLog,
  OperationInfo
} from '../src/declarations/one_bridge_canister/one_bridge_canister.did.js'

function state(overrides = {}): StateInfo {
  return {
    error_rounds: 0n,
    runtime: [
      {
        migration_remaining: 0n,
        ledger_verified: true,
        keys_ready: [true, false],
        svm_mint_verified: false,
        pending_count: 2n,
        unresolved_operations: 1n,
        resource_limits: { max_pending: 10 },
        ...overrides
      }
    ]
  } as unknown as StateInfo
}
function log(overrides = {}): BridgeLog {
  return {
    id: [],
    finalized_at: 0n,
    stuck: false,
    error: [],
    to_tx: [],
    ...overrides
  } as unknown as BridgeLog
}
function operation(
  phase: OperationInfo['phase'],
  kind = 'deposit'
): OperationInfo {
  return {
    kind,
    phase,
    error: [],
    reconciliation: []
  } as unknown as OperationInfo
}

test('new deposits respect migration/readiness/capacity, while unrelated Solana readiness does not block BNB', () => {
  assert.equal(readinessReason(state(), ['ICP', 'BNB']), null)
  assert.match(readinessReason(state(), ['SOL', 'ICP'])!, /Solana signing key/)
  assert.match(
    readinessReason(state({ migration_remaining: 1n }), ['ICP', 'BNB'])!,
    /upgraded/
  )
  assert.match(
    readinessReason(state({ ledger_verified: false }), ['ICP', 'BNB'])!,
    /ledger/
  )
  assert.match(
    readinessReason(state({ pending_count: 9n }), ['ICP', 'BNB'])!,
    /queue is full/
  )
  assert.equal(
    readinessReason(state({ migration_remaining: 10n }), ['BNB'], false),
    null
  )
  assert.match(
    readinessReason({ runtime: [] } as unknown as StateInfo, ['BNB'])!,
    /upgrade/
  )
})

test('archived index zero and closed payments are terminal; held tasks cannot be rechecked', () => {
  assert.equal(logSettled(log({ id: [0n] })), true)
  assert.equal(logSettled(log({ finalized_at: 42n })), true)
  assert.equal(logSettled(log()), false)
  assert.equal(
    canRecheck(log({ stuck: true, error: ['BNB: temporary failure'] })),
    true
  )
  assert.equal(
    canRecheck(
      log({
        stuck: true,
        error: ['resolve legacy conflict operation 7 before resuming this task']
      })
    ),
    false
  )
  assert.equal(canRecheck(log({ id: [0n] })), false)
})

test('uncertain payments and governance holds never offer cancellation or unsafe payout resume', () => {
  assert.equal(operationActions(operation({ Submitted: null })).cancel, false)
  assert.equal(
    operationActions(operation({ NeedsReview: 'unknown result' })).resume,
    false
  )
  assert.equal(
    operationActions(operation({ NeedsReview: 'unknown result' })).cancel,
    false
  )
  assert.equal(operationActions(operation({ Prepared: null })).cancel, true)
  assert.equal(
    operationActions(operation({ Prepared: null }, 'payout')).resume,
    false
  )
  assert.equal(
    operationActions(operation({ Rejected: 'failed' }, 'payout')).cancel,
    false
  )
  assert.equal(
    operationStatus(operation({ Completed: { Icp: [true, 0n] } })),
    'Deposit received'
  )
})

test('fee arithmetic matches canister ceilings, native transfer gas and provider identity grouping', () => {
  const limits = {
    max_fee_per_gas: 300n,
    max_priority_fee_per_gas: 30n,
    max_transaction_fee: 20_000_000n,
    max_hourly_fee: 100_000_000n
  }
  const quote = evmFee(84_000n, 100n, 20n, limits)
  assert.equal(quote.amount, 18_816_000n)
  assert.equal(quote.warning, undefined)
  assert.match(evmFee(84_000n, 150n, 20n, limits).warning!, /exceeds/)
  assert.equal(evmFee(21_000n, 100n, 20n).amount, 4_704_000n)
  assert.equal(
    providerIdentity('https://bsc-dataseed1.bnbchain.org'),
    providerIdentity('https://bsc-dataseed2.bnbchain.org')
  )
  assert.equal(
    providerIdentity('https://eth.alchemyapi.io/v2/key'),
    providerIdentity('https://base.alchemy.com/v2/key')
  )
})

test('token amounts never round through floating point and reject unsupported precision', () => {
  const token = new TokenDisplay(8)
  assert.equal(
    token.parseAmount('9007199254740993.12345678'),
    900719925474099312345678n
  )
  assert.match(token.displayValue(900719925474099312345678n), /12345678$/)
  assert.throws(() => token.parseAmount('1.000000001'), /more than 8 decimals/)
  assert.throws(() => token.parseAmount('-1'), /Invalid amount/)
})

test('mint verification pauses SPL transfers without blocking a native SOL withdrawal', () => {
  const waitingMint = state({
    keys_ready: [true, true],
    svm_mint_verified: false
  })
  assert.match(
    readinessReason(waitingMint, ['SOL'], false)!,
    /token is being verified/
  )
  assert.equal(readinessReason(waitingMint, ['SOL'], false, true), null)
})
