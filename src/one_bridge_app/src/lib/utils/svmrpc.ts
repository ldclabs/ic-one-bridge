import {
  address,
  createSolanaRpc,
  getAddressEncoder,
  getProgramDerivedAddress,
  isSolanaError,
  mainnet,
  SOLANA_ERROR__JSON_RPC__INVALID_PARAMS,
  type Address,
  type Base64EncodedWireTransaction,
  type Signature
} from '@solana/kit'

const addressEncoder = getAddressEncoder()

export class SvmRpc {
  #providers: string[]
  #rpc
  #mintAddress: string
  #programId: string

  constructor(providers: string[], mintAddress: string, programId: string) {
    this.#providers = providers
    this.#rpc = createSolanaRpc(mainnet(providers[0] as string))
    this.#mintAddress = mintAddress
    this.#programId = programId
  }

  async selectProvider() {
    let selected = await Promise.any(
      this.#providers.map(async (url) => {
        const rpc = createSolanaRpc(mainnet(url))
        await rpc.getLatestBlockhash().send()
        return rpc
      })
    )
    this.#rpc = selected
  }

  // an address that never held SOL reads 0, so any error is a real failure
  async getBalance(addr: string): Promise<bigint> {
    const { value } = await this.#rpc.getBalance(address(addr)).send()
    return BigInt(value)
  }

  async #associatedTokenAddress(addr: string): Promise<Address> {
    const [pda, _] = await getProgramDerivedAddress({
      programAddress: 'ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL' as Address,
      seeds: [
        // Owner
        addressEncoder.encode(addr as Address),
        // Token program
        addressEncoder.encode(this.#programId as Address),
        // Mint
        addressEncoder.encode(this.#mintAddress as Address)
      ]
    })
    return pda
  }

  async getSplBalance(addr: string): Promise<bigint> {
    const address = await this.#associatedTokenAddress(addr)
    try {
      const { value } = await this.#rpc.getTokenAccountBalance(address).send()
      return BigInt(value.amount)
    } catch (e) {
      // the token account does not exist until it first receives the token;
      // every other failure must surface rather than read as an empty balance
      if (isSolanaError(e, SOLANA_ERROR__JSON_RPC__INVALID_PARAMS)) return 0n
      throw e
    }
  }

  /**
   * 'processed', 'confirmed', 'finalized', 'failed', or '' when no provider
   * has the signature yet.
   *
   * A transaction that failed still reaches the `finalized` commitment level
   * while having moved nothing, so `err` decides before the commitment does.
   */
  async getTransactionStatus(sig: string): Promise<string> {
    const { value } = await this.#rpc
      .getSignatureStatuses([sig as Signature])
      .send()
    const status = value?.[0]
    if (!status) return ''
    return status.err ? 'failed' : status.confirmationStatus || ''
  }

  async sendRawTransaction(signedTx: string): Promise<string> {
    const rt = await this.#rpc
      .sendTransaction(signedTx as Base64EncodedWireTransaction, {
        encoding: 'base64'
      })
      .send()
    return rt
  }
}
