import {
  idlFactory,
  type BridgeLog,
  type BridgeTx,
  type StateInfo,
  type _SERVICE
} from '$declarations/one_bridge_canister/one_bridge_canister.did.js'
import { unwrapResult } from '$lib/types/result'
import { EvmRpc } from '$lib/utils/evmrpc'
import { type TokenInfo } from '$lib/utils/token'
import { Principal } from '@dfinity/principal'
import { bytesToHex } from '@ldclabs/cose-ts/utils'
import { SvelteMap } from 'svelte/reactivity'
import { createActor } from './actors'
import { TokenLedgerAPI } from './tokenledger'

export {
  type BridgeLog,
  type BridgeTarget,
  type BridgeTx,
  type StateInfo
} from '$declarations/one_bridge_canister/one_bridge_canister.did.js'

export function txHash(tx: BridgeTx): string | null {
  if ('Evm' in tx) {
    const [_isFinalized, rawTx] = tx.Evm
    const bytes = rawTx instanceof Uint8Array ? rawTx : Uint8Array.from(rawTx)
    return '0x' + bytesToHex(bytes)
  }

  return null
}

// interface BridgeLog {
//   'id' : [] | [bigint],
//   'to' : BridgeTarget,
//   'fee' : bigint,
//   'to_tx' : [] | [BridgeTx],
//   'to_addr' : [] | [string],
//   'from' : BridgeTarget,
//   'user' : Principal,
//   'from_tx' : BridgeTx,
//   'created_at' : bigint,
//   'error' : [] | [string],
//   'icp_amount' : bigint,
//   'finalized_at' : bigint,
// }

export class BridgeLogInfo {
  readonly log: BridgeLog

  constructor(log: BridgeLog) {
    this.log = log
  }
}

export class BridgeCanisterAPI {
  static #bridges: SvelteMap<string, BridgeCanisterAPI> = new SvelteMap()

  static async loadBridge(canisterId: string): Promise<BridgeCanisterAPI> {
    if (this.#bridges.has(canisterId)) {
      return this.#bridges.get(canisterId) as BridgeCanisterAPI
    }

    const bridge = new BridgeCanisterAPI(canisterId)
    this.#bridges.set(canisterId, bridge)
    return bridge
  }

  readonly canisterId: Principal
  #actor: _SERVICE
  #token: TokenInfo | null = null
  #tokenLedger: TokenLedgerAPI | null = null
  #state = $state<StateInfo | null>(null)

  constructor(canisterId: string) {
    this.canisterId = Principal.fromText(canisterId)
    this.#actor = createActor<_SERVICE>({
      canisterId: this.canisterId,
      idlFactory: idlFactory
    })
  }

  get state(): StateInfo | null {
    return this.#state
  }

  get token(): TokenInfo | null {
    return this.#token
  }

  async loadState(): Promise<StateInfo> {
    if (this.#state == null) {
      await this.refreshState()
    }

    return this.#state as StateInfo
  }

  async refreshState(): Promise<void> {
    const state = await this.#actor.info()
    this.#state = unwrapResult(state, 'call get_state failed')
  }

  async supportChains(): Promise<string[]> {
    const state = await this.loadState()
    return ['ICP', ...state.evm_token_contracts.map(([name, _]) => name)]
  }

  async loadICPTokenAPI(): Promise<TokenLedgerAPI> {
    if (this.#tokenLedger == null) {
      const state = await this.loadState()
      const token: TokenInfo = {
        name: state.token_name,
        symbol: state.token_symbol,
        decimals: state.token_decimals,
        fee: 0n,
        one: 10n ** BigInt(state.token_decimals),
        logo: state.token_logo,
        canisterId: state.token_ledger.toText()
      }

      this.#token = token
      this.#tokenLedger = new TokenLedgerAPI(token)
      let info = await this.#tokenLedger.fetchTokenInfo()
      this.#token.fee = info.fee
    }

    return this.#tokenLedger
  }

  async loadEVMTokenAPI(chain: string): Promise<EvmRpc> {
    const state = await this.loadState()
    const provider = state.evm_providers.find(([name, _]) => name === chain)
    if (!provider) {
      throw new Error(`EVM providers for chain ${chain} not found`)
    }
    const [_maxConfirmations, providerUrls] = provider[1]
    if (providerUrls.length === 0) {
      throw new Error(`EVM provider URLs for chain ${chain} is empty`)
    }

    const api = new EvmRpc(providerUrls)
    await api.selectProvider()
    return api
  }

  async getMyBridgeLog(fromTx: BridgeTx): Promise<BridgeLog> {
    const res = await this.#actor.my_bridge_log(fromTx)
    return unwrapResult(res, 'call my_bridge_log failed')
  }

  async listMyPendingLogs(): Promise<BridgeLog[]> {
    const res = await this.#actor.my_pending_logs()
    return unwrapResult(res, 'call my_pending_logs failed')
  }

  async listMyFinalizedLogs(
    prev?: bigint,
    take?: bigint
  ): Promise<BridgeLog[]> {
    const res = await this.#actor.my_finalized_logs(
      prev ? [prev] : [],
      take ? [take] : []
    )
    return unwrapResult(res, 'call my_finalized_logs failed')
  }

  async listPendingLogs(): Promise<BridgeLog[]> {
    const res = await this.#actor.pending_logs()
    return unwrapResult(res, 'call pending_logs failed')
  }

  async listFinalizedLogs(prev?: bigint, take?: bigint): Promise<BridgeLog[]> {
    const res = await this.#actor.finalized_logs(
      prev ? [prev] : [],
      take ? [take] : []
    )
    return unwrapResult(res, 'call finalized_logs failed')
  }

  async bridge(
    fromChain: string,
    toChain: string,
    icpAmount: bigint,
    toAddr?: string
  ): Promise<BridgeTx> {
    const res = await this.#actor.bridge(
      fromChain,
      toChain,
      icpAmount,
      toAddr ? [toAddr] : []
    )
    return unwrapResult(res, 'call bridge failed')
  }

  // return signed erc20 transfer transaction
  async buildErc20TransferTx(
    chain: string,
    toAddr: string,
    icpAmount: bigint
  ): Promise<string> {
    const tx = await this.#actor.erc20_transfer_tx(chain, toAddr, icpAmount)
    return unwrapResult(tx, 'call erc20_transfer_tx failed')
  }

  // return signed evm transfer transaction
  async buildEvmTransferTx(
    chain: string,
    toAddr: string,
    evmAmount: bigint
  ): Promise<string> {
    const tx = await this.#actor.evm_transfer_tx(chain, toAddr, evmAmount)
    return unwrapResult(tx, 'call evm_transfer_tx failed')
  }
}
