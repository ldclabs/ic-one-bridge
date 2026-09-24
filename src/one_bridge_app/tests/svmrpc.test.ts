import { test } from 'node:test'
import assert from 'node:assert/strict'
import { SvmRpc } from '../src/lib/utils/svmrpc.ts'

const OWNER = '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM'

function rpc() {
  return new SvmRpc(
    ['https://sol.example'],
    'So11111111111111111111111111111111111111112',
    'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA'
  )
}

function reply(options: RequestInit, body: object) {
  const { id } = JSON.parse(options.body as string)
  return new Response(JSON.stringify({ jsonrpc: '2.0', id, ...body }))
}

test('a token account that was never opened reads as an empty balance', async (t) => {
  t.mock.method(
    globalThis,
    'fetch',
    async (_url: string, options: RequestInit) =>
      reply(options, {
        error: {
          code: -32602,
          message: 'Invalid param: could not find account'
        }
      })
  )
  assert.equal(await rpc().getSplBalance(OWNER), 0n)
})

test('a failing provider surfaces instead of reading as a zero balance', async (t) => {
  t.mock.method(
    globalThis,
    'fetch',
    async () => new Response('unavailable', { status: 503 })
  )
  await assert.rejects(rpc().getSplBalance(OWNER))
  await assert.rejects(rpc().getBalance(OWNER))
})
