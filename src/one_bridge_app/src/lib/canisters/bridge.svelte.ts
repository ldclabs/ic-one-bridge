import {
  idlFactory,
  type BridgeLog,
  type BridgeTarget,
  type BridgeTx,
  type StateInfo,
  type _SERVICE
} from '$declarations/one_bridge_canister/one_bridge_canister.did.js'
import {
  type BridgeLogInfo,
  type BridgingStatus,
  type Chain
} from '$lib/types/bridge'
import { unwrapResult } from '$lib/types/result'
import { EvmRpc } from '$lib/utils/evmrpc'
import { TokenDisplay, type TokenInfo } from '$lib/utils/token'
import { Principal } from '@dfinity/principal'
import { bytesToHex } from '@ldclabs/cose-ts/utils'
import { tick } from 'svelte'
import { SvelteMap } from 'svelte/reactivity'
import { createActor } from './actors'
import { TokenLedgerAPI } from './tokenledger'

export {
  type BridgeLog,
  type BridgeTarget,
  type BridgeTx,
  type StateInfo
} from '$declarations/one_bridge_canister/one_bridge_canister.did.js'

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

  getTokenUrl(chain: string): [string, string] {
    if (!this.#state) return ['', '']
    if (chain === 'ICP') {
      const token = this.#state.token_ledger.toText()
      return [token, `https://dashboard.internetcomputer.org/canister/${token}`]
    }
    const contract = this.#state.evm_token_contracts.find(
      ([name, _]) => name === chain
    )?.[1][0]
    if (!contract) return ['', '']
    switch (chain) {
      case 'BNB':
        return [contract, `https://bscscan.com/token/${contract}`]
      default:
        return ['', '']
    }
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

  parseNativeAmount(chain: string, amount: string | number): bigint {
    let decimals = 8
    if (chain !== 'ICP') decimals = 18
    const token: TokenInfo = {
      name: chain,
      symbol: chain,
      decimals: decimals,
      fee: 0n,
      one: 10n ** BigInt(decimals),
      logo: '',
      canisterId: ''
    }
    const td = new TokenDisplay(token, 0n)
    return td.parseAmount(amount)
  }

  displayNativeAmount(chain: string, balance: bigint): string {
    let decimals = 8
    if (chain !== 'ICP') decimals = 18
    const token: TokenInfo = {
      name: chain,
      symbol: chain,
      decimals: decimals,
      fee: 0n,
      one: 10n ** BigInt(decimals),
      logo: '',
      canisterId: ''
    }
    const td = new TokenDisplay(token, balance)
    return td.displayValue(balance)
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
    take: number,
    prev?: bigint
  ): Promise<BridgeLogInfo[]> {
    const res = await this.#actor.my_finalized_logs(take, prev ? [prev] : [])
    const logs = unwrapResult(res, 'call my_finalized_logs failed')
    return logs.map((log) => this.toBridgeLogInfo(log))
  }

  async listPendingLogs(): Promise<BridgeLog[]> {
    const res = await this.#actor.pending_logs()
    return unwrapResult(res, 'call pending_logs failed')
  }

  async listFinalizedLogs(
    take: number,
    prev?: bigint
  ): Promise<BridgeLogInfo[]> {
    const res = await this.#actor.finalized_logs(take, prev ? [prev] : [])
    const logs = unwrapResult(res, 'call finalized_logs failed')
    return logs.map((log) => this.toBridgeLogInfo(log))
  }

  async bridge(
    fromChain: string,
    toChain: string,
    icpAmount: bigint,
    toAddr?: string
  ): Promise<BridgingProgress> {
    const res = await this.#actor.bridge(
      fromChain,
      toChain,
      icpAmount,
      toAddr ? [toAddr] : []
    )
    let tx = unwrapResult(res, 'call bridge failed')
    return BridgingProgress.track(this, tx)
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

  toBridgeLogInfo(log: BridgeLog): BridgeLogInfo {
    return {
      id: log.id[0] || 0n,
      user: log.user.toText(),
      token: this.#token?.symbol || '',
      from: getChainName(log.from),
      to: getChainName(log.to),
      amount: this.displayAmount(log.icp_amount),
      fee: this.displayAmount(log.fee),
      fromTx: getTx(log.from_tx),
      fromTxUrl: getTxUrl(this.#token?.canisterId!, log.from, log.from_tx)!,
      toTx: log.to_tx[0] && getTx(log.to_tx[0]),
      toTxUrl: getTxUrl(this.#token?.canisterId!, log.to, log.to_tx[0]),
      toAddr: log.to_addr[0],
      createdAt: Number(log.created_at),
      finalizedAt: Number(log.finalized_at),
      status: getBridgingStatus(log),
      error: log.error[0]
    } as BridgeLogInfo
  }
}

export class BridgingProgress {
  #api: BridgeCanisterAPI
  #tx: BridgeTx
  #log = $state<BridgeLog | null>(null)
  #isComplete = $derived.by(() => isFinalized(this.#log?.to_tx[0]))
  #status = $derived.by(() => getBridgingStatus(this.#log))

  static track(api: BridgeCanisterAPI, tx: BridgeTx): BridgingProgress {
    const progress = new BridgingProgress(api, tx)
    progress.#refreshLog()
    return progress
  }

  private constructor(api: BridgeCanisterAPI, tx: BridgeTx) {
    this.#api = api
    this.#tx = tx
  }

  #refreshLog = async (): Promise<void> => {
    try {
      this.#log = await this.#api.getMyBridgeLog(this.#tx)
      await tick()
      if (!this.#isComplete) {
        setTimeout(() => this.#refreshLog(), 2000)
      }
    } catch (error) {
      console.error(`Error refreshing log ${this.#tx}:`, error)
    }
  }

  get status(): BridgingStatus {
    return this.#status
  }

  get isComplete(): boolean {
    return this.#isComplete
  }

  get info(): BridgeLogInfo | null {
    return this.#log ? this.#api.toBridgeLogInfo(this.#log) : null
  }

  get message(): string {
    if (!this.#log) {
      return 'bridging request accepted.'
    }
    if (isFinalized(this.#log.to_tx[0])) {
      return ''
    }
    if (this.#log.error.length > 0) {
      return `${this.#log.error[0]}`
    }
    if (isFinalized(this.#log.from_tx)) {
      return `waiting for confirmation on ${getChainName(this.#log.to)}`
    }
    return `waiting for confirmation on ${getChainName(this.#log.from)}`
  }
}

export type TransferTxInfo = {
  chain: string
  native: boolean
  isFinalized: boolean
  Icp?: bigint
  Evm?: string
}

export class TransferingProgress {
  #api: BridgeCanisterAPI
  #tx = $state<TransferTxInfo | null>(null)

  static track(
    api: BridgeCanisterAPI,
    tx: TransferTxInfo
  ): TransferingProgress {
    const progress = new TransferingProgress(api, tx)
    progress.#refreshLog()
    return progress
  }

  private constructor(api: BridgeCanisterAPI, tx: TransferTxInfo) {
    this.#api = api
    this.#tx = tx
  }

  #refreshLog = async (): Promise<void> => {
    if (!this.#tx || this.#tx.isFinalized) return

    try {
      const evm = await this.#api.loadEVMTokenAPI(this.#tx.chain)
      if ('Evm' in this.#tx) {
        const receipt = await evm.getTransactionReceipt(this.#tx.Evm)
        if (receipt && receipt.status === '0x1') {
          this.#tx.isFinalized = true
          return
        }
        setTimeout(() => this.#refreshLog(), 2000)
      }
    } catch (error) {
      console.error(`Error refreshing log ${this.#tx}:`, error)
    }
  }

  get status(): BridgingStatus {
    if (this.#tx?.isFinalized) {
      return 'Completed'
    }
    return 'Pending'
  }

  get isComplete(): boolean {
    return this.#tx?.isFinalized || false
  }

  get chain(): string {
    return this.#tx?.chain || ''
  }

  get tx(): string {
    if (!this.#tx) return ''

    if ('Evm' in this.#tx) {
      return this.#tx.Evm
    } else if ('Icp' in this.#tx) {
      return this.#tx.Icp.toString()
    }

    return ''
  }

  get txUrl(): string {
    switch (this.#tx?.chain) {
      case 'ICP':
        // can not link to specific transaction on ICP explorer by id
        return `https://www.icexplorer.io/token/details/${this.#api.token?.canisterId!}`
      case 'BNB':
        if ('Evm' in this.#tx) {
          return `https://bscscan.com/tx/${this.#tx.Evm}`
        }
        return ''
      default:
        return ''
    }
  }

  get message(): string {
    if (this.isComplete) {
      return ''
    }
    return `waiting for confirmation on ${this.#tx!.chain}`
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
        explorerUrl: 'https://www.icexplorer.io',
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
        averageFinalitySeconds: 11
      }
    default:
      throw new Error(`unsupported chain ${chain}`)
  }
}

function getTxUrl(
  tokenledger: string,
  target: BridgeTarget,
  tx?: BridgeTx
): string | undefined {
  if (!tx) return undefined
  switch (getChainName(target)) {
    case 'ICP': {
      // can not link to specific transaction on ICP explorer by id
      return `https://www.icexplorer.io/token/details/${tokenledger}`
    }
    case 'BNB': {
      const hash = getTx(tx)
      if (!hash) return undefined
      return `https://bscscan.com/tx/${hash}`
    }
    default:
      return undefined
  }
}

function getChainName(target: BridgeTarget): string {
  if ('Evm' in target) {
    return target.Evm
  } else if ('Icp' in target) {
    return 'ICP'
  }
  return 'Unknown'
}

function getTx(tx: BridgeTx): string {
  if ('Evm' in tx) {
    const [_isFinalized, rawTx] = tx.Evm
    const bytes = rawTx instanceof Uint8Array ? rawTx : Uint8Array.from(rawTx)
    return '0x' + bytesToHex(bytes)
  }

  return tx.Icp[1].toString()
}

function getBridgingStatus(log?: BridgeLog | null): BridgingStatus {
  if (!log) {
    return 'Accepted' as BridgingStatus
  }
  if (isFinalized(log.to_tx[0])) {
    return 'Completed' as BridgingStatus
  }
  if (log.error.length > 0) {
    return 'Error' as BridgingStatus
  }
  return 'Pending' as BridgingStatus
}

function isFinalized(tx?: BridgeTx): boolean {
  if (!tx) return false
  if ('Evm' in tx) {
    return tx.Evm[0]
  } else if ('Icp' in tx) {
    return tx.Icp[0]
  }
  return false
}
