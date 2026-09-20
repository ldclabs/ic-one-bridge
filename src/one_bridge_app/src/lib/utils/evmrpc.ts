import type { EvmFeeLimits } from '../../declarations/one_bridge_canister/one_bridge_canister.did.js'
import { evmFee, providerIdentity } from './bridge-fees.ts'

/**
 * Read-only EVM access over public JSON-RPC endpoints.
 *
 * The bridge canister publishes the provider URLs in `info()`; the browser
 * talks to them directly, so nothing here holds a key or signs anything —
 * `sendRawTransaction` only broadcasts a transaction the canister signed.
 */
export class EvmRpc {
  #providers: string[]
  #endpoint: string
  #contract: string

  constructor(providers: string[], contract: string) {
    this.#providers = providers
    this.#endpoint = providers[0] as string
    this.#contract = contract
  }

  // settle on the first provider that answers, so a dead endpoint in the list
  // does not make every later call slow or fail
  async selectProvider() {
    this.#endpoint = await Promise.any(
      this.#providers.map(async (url) => {
        await jsonRPC<string>(url, 'eth_chainId')
        return url
      })
    )
  }

  async #hex(method: string, params: unknown[] = []): Promise<bigint> {
    return BigInt((await jsonRPC<string>(this.#endpoint, method, params)) ?? 0)
  }

  // Prefer the canister's recent two-provider quote. Browser endpoints may
  // differ from the canister's private providers, so this remains an estimate.
  async gasFeeEstimation(
    gas: bigint,
    cached?: [bigint, bigint, bigint],
    limits?: EvmFeeLimits
  ): Promise<{ amount: bigint; warning?: string | undefined }> {
    if (cached && cached[0] + 120_000n >= BigInt(Date.now())) {
      return evmFee(gas, cached[1], cached[2], limits)
    }
    const unique = [
      ...new Map(
        this.#providers.map((url) => [providerIdentity(url), url])
      ).values()
    ]
    const quotes = await Promise.allSettled(
      unique.map(async (url) => {
        const [price, priority] = await Promise.all([
          jsonRPC<string>(url, 'eth_gasPrice'),
          jsonRPC<string>(url, 'eth_maxPriorityFeePerGas')
        ])
        if (price === null || priority === null)
          throw new Error('RPC returned no gas quote')
        return [BigInt(price), BigInt(priority)] as const
      })
    )
    const valid = quotes.flatMap((result) =>
      result.status === 'fulfilled' ? [result.value] : []
    )
    if (!valid.length)
      throw new Error(
        'No public provider could estimate gas. Refresh before submitting.'
      )
    const price = valid.reduce(
      (max, [value]) => (value > max ? value : max),
      0n
    )
    const tip = valid.reduce(
      (max, [, value]) => (value > max ? value : max),
      0n
    )
    const estimate = evmFee(gas, price, tip, limits)
    return {
      ...estimate,
      warning:
        estimate.warning ??
        (valid.length < 2
          ? 'Gas is estimated from one public provider. The bridge checks two providers before signing, so the final requirement may be higher.'
          : undefined)
    }
  }

  async getBalance(address: string): Promise<bigint> {
    return this.#hex('eth_getBalance', [address, 'latest'])
  }

  async getErc20Balance(address: string): Promise<bigint> {
    // balanceOf(address) selector, then the address left-padded to 32 bytes
    const data =
      '0x70a08231000000000000000000000000' +
      address.toLowerCase().replace(/^0x/, '')
    return this.#hex('eth_call', [{ to: this.#contract, data }, 'latest'])
  }

  async getTransactionReceipt(
    txHash: string
  ): Promise<{ status: string } | null> {
    return jsonRPC(this.#endpoint, 'eth_getTransactionReceipt', [txHash])
  }

  async sendRawTransaction(signedTx: string): Promise<string> {
    return (
      (await jsonRPC<string>(this.#endpoint, 'eth_sendRawTransaction', [
        signedTx
      ])) ?? '0x'
    )
  }
}

async function jsonRPC<T>(
  url: string,
  method: string,
  params: unknown[] = []
): Promise<T | null> {
  const resp = await fetch(url, {
    method: 'POST',
    signal: AbortSignal.timeout(15_000),
    mode: 'cors',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json'
    },
    body: JSON.stringify({ id: 1, jsonrpc: '2.0', method, params })
  })

  if (!resp.ok) {
    throw new Error(
      `${method} on ${url} failed: ${resp.status} ${resp.statusText}`
    )
  }

  const res = (await resp.json()) as {
    result?: T
    error?: { code: number; message: string; data?: unknown }
  }

  if (res.error) {
    const { code, message, data } = res.error
    throw new Error(
      `JSON-RPC Error ${code}: ${message}${data ? ` - ${JSON.stringify(data)}` : ''}`
    )
  }

  return res.result ?? null
}
