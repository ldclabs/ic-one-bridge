import { test } from 'node:test'
import assert from 'node:assert/strict'
import { EvmRpc } from '../src/lib/utils/evmrpc.ts'

test('a recent canister quote is used without making an inconsistent browser quote request', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => {
    throw new Error('must not request another quote')
  })
  const rpc = new EvmRpc(['https://a.example'], '0xtoken')
  assert.equal(
    (await rpc.gasFeeEstimation(21_000n, [BigInt(Date.now()), 100n, 20n]))
      .amount,
    4_704_000n
  )
})

test('browser estimates take conservative successful independent quotes and tolerate an unavailable provider', async (t) => {
  t.mock.method(
    globalThis,
    'fetch',
    async (url: string, options: RequestInit) => {
      if (url.includes('offline')) throw Error('offline')
      const { method } = JSON.parse(options.body as string)
      const value = url.includes('high')
        ? method === 'eth_gasPrice'
          ? '0x96'
          : '0x1e'
        : method === 'eth_gasPrice'
          ? '0x64'
          : '0x14'
      return new Response(JSON.stringify({ result: value }), { status: 200 })
    }
  )
  const rpc = new EvmRpc(
    ['https://low.example', 'https://high.example', 'https://offline.example'],
    '0xtoken'
  )
  const quote = await rpc.gasFeeEstimation(84_000n, [0n, 1n, 1n])
  assert.equal(quote.amount, 28_224_000n)
  assert.equal(quote.warning, undefined)
})

test('two URLs from the same provider remain a single-provider estimate and missing quotes fail visibly', async (t) => {
  const mock = t.mock.method(
    globalThis,
    'fetch',
    async () => new Response(JSON.stringify({ result: '0x64' }))
  )
  const rpc = new EvmRpc(
    ['https://a.alchemy.com/key1', 'https://b.alchemy.com/key2'],
    '0xtoken'
  )
  assert.match(
    (await rpc.gasFeeEstimation(84_000n)).warning!,
    /one public provider/
  )
  mock.mock.mockImplementation(
    async () => new Response(JSON.stringify({ result: null }))
  )
  await assert.rejects(rpc.gasFeeEstimation(84_000n), /No public provider/)
})
