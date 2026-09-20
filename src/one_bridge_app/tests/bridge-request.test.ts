import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  prepareRequest,
  readRequest,
  saveRequest,
  submitRequest,
  requestBytes,
  forgetRequest,
  type RequestStorage
} from '../src/lib/utils/bridge-request.ts'

function storage(): RequestStorage {
  const data = new Map<string, string>()
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => {
      data.set(key, value)
    },
    removeItem: (key) => {
      data.delete(key)
    }
  }
}
const plan = {
  from: 'ICP',
  to: 'BNB',
  amount: '10000000000000000001',
  recipient: ''
}

test('a lost reply retries the persisted ID without checking the spent balance or approving again', async () => {
  const db = storage()
  const request = prepareRequest(db, 'bridge', 'alice', plan)
  let preparations = 0
  let payments = 0
  const ledger = new Map<string, bigint>()
  const send = async (_request: unknown, id: Uint8Array) => {
    const key = Buffer.from(id).toString('hex')
    if (!ledger.has(key)) {
      ledger.set(key, 15n)
      payments++
      throw new Error('reply lost after debit')
    }
    return ledger.get(key)!
  }
  await assert.rejects(
    submitRequest(db, request, {
      assertOwner() {},
      prepare: async () => {
        preparations++
      },
      send
    })
  )
  const afterReload = readRequest(db, 'bridge', 'alice')!
  assert.equal(afterReload.stage, 'submitted')
  const result = await submitRequest(db, afterReload, {
    assertOwner() {},
    prepare: async () => {
      throw new Error('balance is now zero')
    },
    send
  })
  assert.equal(result, 15n)
  assert.equal(preparations, 1)
  assert.equal(payments, 1)
  assert.equal(readRequest(db, 'bridge', 'alice')!.stage, 'accepted')
  // Following an accepted receipt is still the original request.
  assert.equal(
    await submitRequest(db, afterReload, {
      assertOwner() {},
      prepare: async () => {
        throw Error('must not prepare')
      },
      send
    }),
    15n
  )
  assert.equal(payments, 1)
})

test('requests are partitioned by account and canister, retain exact amounts, and require explicit dismissal', () => {
  const db = storage()
  const first = prepareRequest(db, 'bridge', 'alice', plan)
  assert.equal(first.amount, plan.amount)
  assert.equal(requestBytes(first).length, 32)
  assert.equal(readRequest(db, 'bridge', 'bob'), null)
  assert.equal(readRequest(db, 'other', 'alice'), null)
  assert.throws(
    () => prepareRequest(db, 'bridge', 'alice', plan),
    /Review the saved/
  )
  forgetRequest(db, first)
  const second = prepareRequest(db, 'bridge', 'alice', plan)
  assert.notEqual(first.id, second.id)
  forgetRequest(db, first)
  assert.equal(readRequest(db, 'bridge', 'alice')!.id, second.id)
})

test('storage failure and account changes stop submission before an external payment', async () => {
  assert.throws(
    () =>
      prepareRequest(
        {
          ...storage(),
          setItem() {
            throw Error('storage blocked')
          }
        },
        'bridge',
        'alice',
        plan
      ),
    /storage blocked/
  )
  const db = storage()
  const request = prepareRequest(db, 'bridge', 'alice', plan)
  let owner = 'alice'
  let sent = false
  await assert.rejects(
    submitRequest(db, request, {
      assertOwner() {
        if (owner !== 'alice') throw Error('account changed')
      },
      prepare: async () => {
        owner = 'bob'
      },
      send: async () => {
        sent = true
      }
    }),
    /account changed/
  )
  assert.equal(sent, false)
  assert.equal(readRequest(db, 'bridge', 'alice')!.stage, 'prepared')
})

test('failed preparations keep a retryable draft; canister rejection never silently drops a submitted request', async () => {
  const db = storage()
  const request = prepareRequest(db, 'bridge', 'alice', plan)
  await assert.rejects(
    submitRequest(db, request, {
      assertOwner() {},
      prepare: async () => {
        throw Error('approval rejected')
      },
      send: async () => 1
    })
  )
  assert.equal(readRequest(db, 'bridge', 'alice')!.stage, 'prepared')
  await assert.rejects(
    submitRequest(db, request, {
      assertOwner() {},
      prepare: async () => {},
      send: async () => {
        throw Error('operation needs review')
      }
    })
  )
  assert.equal(readRequest(db, 'bridge', 'alice')!.stage, 'submitted')
})

test('request changes in another tab stop stale submission', async () => {
  const db = storage()
  const first = prepareRequest(db, 'bridge', 'alice', plan)
  forgetRequest(db, first)
  const next = prepareRequest(db, 'bridge', 'alice', { ...plan, to: 'SOL' })
  await assert.rejects(
    submitRequest(db, first, {
      assertOwner() {},
      prepare: async () => {},
      send: async () => {
        throw Error('must not send')
      }
    }),
    /changed in another tab/
  )
  assert.equal(readRequest(db, 'bridge', 'alice')!.id, next.id)
  saveRequest(db, { ...next, id: 'invalid' })
  assert.throws(() => readRequest(db, 'bridge', 'alice'), /cannot be read/)
})
