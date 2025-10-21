import {
  idlFactory,
  type BridgeLog,
  type BridgeTx,
  type StateInfo,
  type _SERVICE
} from '$declarations/one_bridge_canister/one_bridge_canister.did.js'
import { type Chain } from '$lib/types/bridge'
import { unwrapResult } from '$lib/types/result'
import { EvmRpc } from '$lib/utils/evmrpc'
import { TokenDisplay, type TokenInfo } from '$lib/utils/token'
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

export class BridgeCanisterAPI {
  static #bridges: SvelteMap<string, BridgeCanisterAPI> = new SvelteMap()

  static async loadBridge(canisterId: string): Promise<BridgeCanisterAPI> {
    if (this.#bridges.has(canisterId)) {
      return this.#bridges.get(canisterId) as BridgeCanisterAPI
    }

    const bridge = new BridgeCanisterAPI(canisterId)
    this.#bridges.set(canisterId, bridge)
    await bridge.loadState()
    return bridge
  }

  readonly canisterId: Principal
  #actor: _SERVICE
  #token: TokenInfo | null = null
  #tokenDisplay: TokenDisplay | null = null
  #tokenLedger: TokenLedgerAPI | null = null
  #evmRPC: Map<string, EvmRpc> = new Map()
  #state = $state<StateInfo | null>(null)

  private constructor(canisterId: string) {
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

  get tokenDisplay(): TokenDisplay | null {
    return this.#tokenDisplay
  }

  toIcpAmount(chain: string, evmBalance: bigint): bigint {
    if (!this.#state) return evmBalance
    const evmDecimals = this.#state.evm_token_contracts.find(
      ([name, _]) => name === chain
    )?.[1][1]
    if (!evmDecimals) return evmBalance
    if (this.#state.token_decimals > evmDecimals) {
      const diff = this.#state.token_decimals - evmDecimals
      return evmBalance * 10n ** BigInt(diff)
    }
    const diff = evmDecimals - this.#state.token_decimals
    return evmBalance / 10n ** BigInt(diff)
  }

  parseAmount(amount: string | number): bigint {
    if (!this.#tokenDisplay) return 0n
    return this.#tokenDisplay.parseAmount(amount)
  }

  displayAmount(icpBalance: bigint): string {
    if (!this.#tokenDisplay) return ''
    return this.#tokenDisplay.displayValue(icpBalance)
  }

  async loadState(): Promise<StateInfo> {
    if (this.#state == null) {
      const state = await this.refreshState()
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
      const td = new TokenDisplay(token, state.min_threshold_to_bridge)
      td.fee = state.token_bridge_fee
      this.#tokenDisplay = td
    }

    return this.#state as StateInfo
  }

  async loadSubBridges(): Promise<BridgeCanisterAPI[]> {
    const state = await this.loadState()
    const subBridges = await Promise.all(
      state.sub_bridges.map(async (canisterId) => {
        try {
          return await BridgeCanisterAPI.loadBridge(canisterId.toText())
        } catch (error) {
          console.error(
            `Failed to load sub-bridge ${canisterId.toText()}:`,
            error
          )

          return null
        }
      })
    )

    return subBridges.filter((b) => b !== null)
  }

  async refreshState(): Promise<StateInfo> {
    const state = await this.#actor.info()
    this.#state = unwrapResult(state, 'call get_state failed')
    return this.#state as StateInfo
  }

  async supportChains(): Promise<Chain[]> {
    const state = await this.loadState()
    return ['ICP', ...state.evm_token_contracts.map(([name, _]) => name)].map(
      getChain
    )
  }

  async loadICPTokenAPI(): Promise<TokenLedgerAPI> {
    if (this.#tokenLedger == null) {
      await this.loadState()

      this.#tokenLedger = new TokenLedgerAPI(this.#token!)
      try {
        let info = await this.#tokenLedger.fetchTokenInfo()
        this.#token!.fee = info.fee
      } catch (error) {
        console.error('Failed to load ICP token API:', error)
      }
    }

    return this.#tokenLedger
  }

  async loadEVMTokenAPI(chain: string): Promise<EvmRpc> {
    if (this.#evmRPC.has(chain)) {
      return this.#evmRPC.get(chain)!
    }

    const state = await this.loadState()
    const contract = state.evm_token_contracts.find(
      ([name, _]) => name === chain
    )
    if (!contract) {
      throw new Error(`EVM token contract for chain ${chain} not found`)
    }
    const provider = state.evm_providers.find(([name, _]) => name === chain)
    if (!provider) {
      throw new Error(`EVM providers for chain ${chain} not found`)
    }
    const [_maxConfirmations, providerUrls] = provider[1]
    if (providerUrls.length === 0) {
      throw new Error(`EVM provider URLs for chain ${chain} is empty`)
    }

    const api = new EvmRpc(providerUrls, contract[1][0])
    this.#evmRPC.set(chain, api)
    await api.selectProvider()
    return api
  }

  async myEvmAddress(): Promise<string> {
    const res = await this.#actor.evm_address([])
    return unwrapResult(res, 'call evm_address failed')
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

function getChain(chain: string): Chain {
  switch (chain) {
    case 'ICP':
      return {
        id: 0,
        name: 'ICP',
        fullName: 'Internet Computer',
        nativeToken: 'ICP',
        explorerUrl: 'https://icexplorer.io',
        logo: '/_assets/icp.webp',
        averageFinalitySeconds: 2
      }
    case 'BNB':
      return {
        id: 56,
        name: 'BNB',
        fullName: 'BNB Chain',
        nativeToken: 'BNB',
        explorerUrl: 'https://bscscan.com',
        logo: '/_assets/bnb.png',
        averageFinalitySeconds: 15
      }
    default:
      throw new Error(`unsupported chain ${chain}`)
  }
}
