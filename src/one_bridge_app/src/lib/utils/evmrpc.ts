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

  async gasFeeEstimation(gas: bigint = 54000n): Promise<bigint> {
    const [gasPrice, maxPriorityFeePerGas] = await Promise.all([
      this.#hex('eth_gasPrice'),
      this.#hex('eth_maxPriorityFeePerGas')
    ])
    return gas * (gasPrice + maxPriorityFeePerGas)
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
