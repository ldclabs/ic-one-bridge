import {
  idlFactory,
  type BridgeLog,
  type BridgeTarget,
  type BridgeTx,
  type StateInfo,
  type OperationInfo,
  type _SERVICE
} from '$declarations/one_bridge_canister/one_bridge_canister.did.js'
import { getChain, type Chain } from '$lib/chains'
import { type BridgeLogInfo, type BridgingStatus } from '$lib/types/bridge'
import { dynAgent } from '$lib/utils/auth'
import {
  prepareRequest,
  submitRequest,
  withRequestLock,
  type BridgeRequest
} from '$lib/utils/bridge-request'
import {
  readinessReason,
  logSettled,
  txFinalized,
  canRecheck,
  reconciliationHeld
} from '$lib/utils/bridge-state'
import { unwrapResult } from '$lib/types/result'
import { EvmRpc } from '$lib/utils/evmrpc'
import { SvmRpc } from '$lib/utils/svmrpc'
import { tokenDisplay, TokenDisplay, type TokenInfo } from '$lib/utils/token'
import { Principal } from '@icp-sdk/core/principal'
import { bytesToHex } from '@ldclabs/cose-ts/utils'
import { getBase58Codec } from '@solana/kit'
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

const base58 = getBase58Codec()

// the Solana system program, which `svm_token_address` carries when the bridge
// has no SPL token configured
const SVM_UNSET = '11111111111111111111111111111111'

// the ICP ledger's own transfer fee, in e8s
const ICP_TX_FEE = 10_000n
// the canister needs 5 000 lamports for a Solana transfer and makes the user's
// derived address the fee payer; the margin keeps a rounded-down "max" from
// landing exactly on that limit.
//
// A transfer to an address that has never held the token also opens its token
// account, and the fee payer puts up ~0.00204 SOL of rent for it. That is not
// added here: a bridge deposit always goes to the bridge's existing account,
// and the canister refuses the wallet case with a message that says so
const SOL_TX_FEE = 10_000n

// the three addresses one identity has across the chains a bridge reaches
export type MyAddresses = {
  icp: string
  svm: string
  evm: string
}

export function addressOn(chain: string, my: MyAddresses): string {
  if (chain === 'ICP') return my.icp
  if (chain === 'SOL') return my.svm
  return my.evm
}

// what the user holds on one chain, as the bridge sees it
export type ChainAccount = {
  chain: string
  // the user's address on this chain
  address: string
  // the bridge's token, in the token's own units
  tokenBalance: bigint
  // the chain's native token, in its own units
  nativeBalance: bigint
  // what one transfer costs on this chain, in native units. On ICP the ledger
  // charges its fee in the token itself, so this is the native ICP fee and
  // only applies to a native ICP transfer
  nativeFee: bigint
  feeWarning?: string | undefined
}

export class BridgeCanisterAPI {
  static #bridges: SvelteMap<string, BridgeCanisterAPI> = new SvelteMap()
  static #loading: Map<string, Promise<BridgeCanisterAPI>> = new Map()

  static async loadBridge(canisterId: string): Promise<BridgeCanisterAPI> {
    const loaded = this.#bridges.get(canisterId)
    if (loaded) {
      return loaded
    }

    // share one in-flight load, and only cache the bridge once its state is
    // loaded, so concurrent callers never get a stateless bridge and a failed
    // load is not cached forever
    let loading = this.#loading.get(canisterId)
    if (!loading) {
      loading = (async () => {
        const bridge = new BridgeCanisterAPI(canisterId)
        await bridge.loadState()
        this.#bridges.set(canisterId, bridge)
        return bridge
      })()
      this.#loading.set(canisterId, loading)
      loading.finally(() => this.#loading.delete(canisterId)).catch(() => {})
    }

    return loading
  }

  readonly canisterId: Principal
  #actor: _SERVICE
  #token = $state<TokenInfo | null>(null)
  #display: TokenDisplay | null = null
  #tokenLedger: TokenLedgerAPI | null = null
  #tokenLedgerLoading: { id: string; promise: Promise<TokenLedgerAPI> } | null =
    null
  #svmRpc: SvmRpc | null = null
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

  // the smallest amount this bridge accepts, in the token's units
  get minAmount(): bigint {
    return this.#state?.min_threshold_to_bridge ?? 0n
  }

  get bridgeFee(): bigint {
    return this.#state?.token_bridge_fee ?? 0n
  }

  get runtime() {
    return this.#state?.runtime[0] ?? null
  }

  readiness(
    chains: string[],
    newBridge = true,
    nativeTransfer = false
  ): string | null {
    return readinessReason(this.#state, chains, newBridge, nativeTransfer)
  }

  get supportsRecovery(): boolean {
    return !!this.runtime
  }

  //#region amounts

  parseAmount(amount: string | number): bigint {
    return this.#display?.parseAmount(amount) ?? 0n
  }

  displayAmount(amount: bigint): string {
    return this.#display?.displayValue(amount) ?? ''
  }

  parseNativeAmount(chain: string, amount: string | number): bigint {
    return tokenDisplay(getChain(chain).nativeDecimals).parseAmount(amount)
  }

  displayNativeAmount(chain: string, amount: bigint): string {
    return tokenDisplay(getChain(chain).nativeDecimals).displayValue(amount)
  }

  // decimals the token uses on the given chain, or undefined when the chain is
  // not configured on this bridge
  #chainDecimals(chain: string): number | undefined {
    if (!this.#state) return undefined
    if (chain === 'ICP') return this.#state.token_decimals
    if (chain === 'SOL') return this.#state.svm_token_address[1]
    return this.#state.evm_token_contracts.find(
      ([name]) => name === chain
    )?.[1][1]
  }

  // an amount held on `chain`, expressed in the token's own units
  toTokenAmount(chain: string, chainAmount: bigint): bigint {
    const decimals = this.#chainDecimals(chain)
    if (!this.#state || decimals == undefined) return chainAmount
    const tokenDecimals = this.#state.token_decimals
    if (tokenDecimals >= decimals) {
      return chainAmount * 10n ** BigInt(tokenDecimals - decimals)
    }
    return chainAmount / 10n ** BigInt(decimals - tokenDecimals)
  }

  // the conversion the canister applies before it signs the transfer
  toChainAmount(chain: string, tokenAmount: bigint): bigint {
    const decimals = this.#chainDecimals(chain)
    if (!this.#state || decimals == undefined) return tokenAmount
    const tokenDecimals = this.#state.token_decimals
    if (decimals >= tokenDecimals) {
      return tokenAmount * 10n ** BigInt(decimals - tokenDecimals)
    }
    return tokenAmount / 10n ** BigInt(tokenDecimals - decimals)
  }

  validateBridgePrecision(
    from: string,
    to: string,
    amount: bigint
  ): string | null {
    if (amount > (1n << 128n) - 1n)
      return 'The amount exceeds the bridge’s supported range.'
    for (const [chain, value] of [
      [from, amount],
      [to, amount - this.bridgeFee]
    ] as const) {
      if (value <= 0n) return 'The amount must exceed the bridge fee.'
      if (
        this.toTokenAmount(chain, this.toChainAmount(chain, value)) !== value
      ) {
        return `The amount ${chain === to ? 'after the bridge fee ' : ''}has more precision than ${chain} supports.`
      }
    }
    return null
  }

  //#endregion

  //#region state

  async loadState(): Promise<StateInfo> {
    return this.#state ?? (await this.refreshState())
  }

  async refreshState(): Promise<StateInfo> {
    const state = unwrapResult(await this.#actor.info(), 'call info failed')
    const previous = this.#state
    if (
      !previous ||
      configKey([previous.evm_providers, previous.evm_token_contracts]) !==
        configKey([state.evm_providers, state.evm_token_contracts])
    )
      this.#evmRPC.clear()
    if (
      !previous ||
      configKey([previous.svm_providers, previous.svm_token_address]) !==
        configKey([state.svm_providers, state.svm_token_address])
    )
      this.#svmRpc = null
    const sameLedger = this.#token?.canisterId === state.token_ledger.toText()
    if (!sameLedger) this.#tokenLedger = null
    this.#token = {
      name: state.token_name,
      symbol: state.token_symbol,
      decimals: state.token_decimals,
      fee: sameLedger ? (this.#token?.fee ?? 0n) : 0n,
      logo: state.token_logo,
      canisterId: state.token_ledger.toText()
    }
    this.#display = new TokenDisplay(state.token_decimals)
    this.#state = state
    return state
  }

  /**
   * The other bridge canisters this one fronts, one per extra token.
   *
   * A sub-bridge keeps its own token, ledger and logs, but not its own keys:
   * the main bridge derives the user addresses and signs the transfers, which
   * is why the whole app reads addresses from the main bridge alone.
   *
   * Sub-bridge development is paused, so `sub_bridges` is empty in production
   * and every path below currently runs against the main bridge only. The
   * support is kept because the canister still publishes the field.
   */
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

  async supportChains(): Promise<Chain[]> {
    const state = await this.loadState()
    const names = ['ICP']
    if (state.svm_token_address[0] !== SVM_UNSET) {
      names.push('SOL')
    }
    names.push(...state.evm_token_contracts.map(([name]) => name))
    return names.map(getChain)
  }

  // the token's identifier on `chain` and its explorer page, or two empty
  // strings when the chain carries no token on this bridge
  tokenOn(chain: string): [string, string] {
    if (!this.#state) return ['', '']

    let token = ''
    if (chain === 'ICP') {
      token = this.#state.token_ledger.toText()
    } else if (chain === 'SOL') {
      const addr = this.#state.svm_token_address[0]
      token = addr === SVM_UNSET ? '' : addr
    } else {
      token =
        this.#state.evm_token_contracts.find(
          ([name]) => name === chain
        )?.[1][0] ?? ''
    }

    return token ? [token, getChain(chain).tokenUrl(token)] : ['', '']
  }

  //#endregion

  //#region chain clients

  async loadICPTokenAPI(): Promise<TokenLedgerAPI> {
    await this.loadState()
    const token = this.#token!
    const id = token.canisterId
    if (this.#tokenLedger?.canisterId.toText() === id) return this.#tokenLedger
    if (this.#tokenLedgerLoading?.id === id)
      return this.#tokenLedgerLoading.promise
    const promise = (async () => {
      const ledger = new TokenLedgerAPI(token)
      const info = await ledger.fetchTokenInfo()
      if (this.#token?.canisterId !== id)
        throw new Error('The token ledger changed. Refresh before continuing.')
      this.#token.fee = info.fee
      this.#tokenLedger = ledger
      return ledger
    })()
    this.#tokenLedgerLoading = { id, promise }
    try {
      return await promise
    } finally {
      if (this.#tokenLedgerLoading?.promise === promise)
        this.#tokenLedgerLoading = null
    }
  }

  async loadSvmTokenAPI(): Promise<SvmRpc | null> {
    if (!this.#svmRpc) {
      const state = await this.loadState()
      if (state.svm_token_address[0] !== SVM_UNSET) {
        if (!state.svm_providers.length)
          throw new Error(
            'Public browser RPC endpoints for SOL are not configured; governance must publish anonymous endpoints.'
          )
        // cache only once a provider answered: an unawaited selection leaves
        // the client on providers[0] and turns a total outage into an
        // unhandled rejection
        const rpc = new SvmRpc(
          state.svm_providers,
          state.svm_token_address[0],
          state.svm_token_address[2]
        )
        await rpc.selectProvider()
        this.#svmRpc = rpc
      }
    }

    return this.#svmRpc
  }

  async loadEVMTokenAPI(chain: string): Promise<EvmRpc> {
    if (this.#evmRPC.has(chain)) {
      return this.#evmRPC.get(chain)!
    }

    const state = await this.loadState()
    const contract = state.evm_token_contracts.find(([name]) => name === chain)
    if (!contract) {
      throw new Error(`EVM token contract for chain ${chain} not found`)
    }
    const provider = state.evm_providers.find(([name]) => name === chain)
    if (!provider) {
      throw new Error(`EVM providers for chain ${chain} not found`)
    }
    const [_maxConfirmations, providerUrls] = provider[1]
    if (providerUrls.length === 0) {
      throw new Error(
        `Public browser RPC endpoints for ${chain} are not configured; the bridge administrator must publish anonymous endpoints`
      )
    }

    const api = new EvmRpc(providerUrls, contract[1][0])
    // same as above: a client whose provider selection threw must not be
    // cached, or every later call silently uses providers[0]
    await api.selectProvider()
    this.#evmRPC.set(chain, api)
    return api
  }

  //#endregion

  //#region accounts

  async myEvmAddress(): Promise<string> {
    const res = await this.#actor.evm_address([])
    return unwrapResult(res, 'call evm_address failed')
  }

  async mySvmAddress(): Promise<string> {
    const res = await this.#actor.svm_address([])
    return unwrapResult(res, 'call svm_address failed')
  }

  /**
   * The user's three addresses, as derived by THIS bridge canister.
   *
   * Call this on the main bridge only. A canister derives its addresses from
   * the root key it holds, and each canister's root key is its own, so a
   * sub-bridge asked directly would answer with addresses nobody deposits to:
   * by design the main bridge owns the key, publishes the one set of addresses
   * every token uses, and signs the transfers on a sub-bridge's behalf.
   *
   * Pass the result down to {@link myAccountOn} rather than re-deriving per
   * bridge. See also the note on {@link BridgeCanisterAPI.loadSubBridges}.
   */
  async myAddresses(icp: string): Promise<MyAddresses> {
    const keys = this.runtime?.keys_ready
    const [svm, evm] = await Promise.all([
      keys && !keys[1] ? Promise.resolve('') : this.mySvmAddress(),
      keys && !keys[0] ? Promise.resolve('') : this.myEvmAddress()
    ])
    return { icp, svm, evm }
  }

  /**
   * The user's position on one chain: where they hold the token, how much of it
   * and of the chain's native coin they have, and what a transfer costs there.
   *
   * `my` must come from the main bridge's {@link myAddresses}, for every
   * bridge: the main canister publishes the one set of addresses, and a
   * sub-bridge's token is deposited to those same addresses.
   */
  async myAccountOn(
    chain: string,
    my: MyAddresses,
    native = false
  ): Promise<ChainAccount> {
    switch (chain) {
      case 'ICP': {
        const icp = await this.loadICPTokenAPI()
        const [tokenBalance, nativeBalance] = await Promise.all([
          icp.balance(),
          icp.getICPBalanceOf(Principal.fromText(my.icp))
        ])
        return {
          chain,
          address: my.icp,
          tokenBalance,
          nativeBalance,
          nativeFee: ICP_TX_FEE
        }
      }
      case 'SOL': {
        const svm = await this.loadSvmTokenAPI()
        if (!svm) throw new Error('SOL is not supported by this bridge')
        const [splBalance, nativeBalance] = await Promise.all([
          native ? Promise.resolve(0n) : svm.getSplBalance(my.svm),
          svm.getBalance(my.svm)
        ])
        return {
          chain,
          address: my.svm,
          tokenBalance: this.toTokenAmount(chain, splBalance),
          nativeBalance,
          nativeFee: SOL_TX_FEE
        }
      }
      default: {
        const state = await this.loadState()
        const evm = await this.loadEVMTokenAPI(chain)
        const [erc20Balance, nativeBalance, nativeFee] = await Promise.all([
          native ? Promise.resolve(0n) : evm.getErc20Balance(my.evm),
          evm.getBalance(my.evm),
          // the gas limit the canister will sign with, so the form and the
          // canister agree on what the address has to hold
          evm.gasFeeEstimation(
            native ? 21_000n : state.erc20_gas_limit,
            state.evm_latest_gas.find(([name]) => name === chain)?.[1],
            this.runtime?.evm_fee_limits.find(([name]) => name === chain)?.[1]
          )
        ])
        return {
          chain,
          address: my.evm,
          tokenBalance: this.toTokenAmount(chain, erc20Balance),
          nativeBalance,
          nativeFee: nativeFee.amount,
          feeWarning: nativeFee.warning
        }
      }
    }
  }

  // what the bridge itself holds on `chain`, in the token's units — the pool a
  // transfer out of that chain is paid from
  async reserveOn(chain: string): Promise<bigint> {
    const state = await this.loadState()
    switch (chain) {
      case 'ICP': {
        const icp = await this.loadICPTokenAPI()
        return icp.getBalanceOf(this.canisterId)
      }
      case 'SOL': {
        const svm = await this.loadSvmTokenAPI()
        if (!svm) throw new Error('SOL is not supported by this bridge')
        const balance = await svm.getSplBalance(state.svm_address)
        return this.toTokenAmount(chain, balance)
      }
      default: {
        const evm = await this.loadEVMTokenAPI(chain)
        const balance = await evm.getErc20Balance(state.evm_address)
        return this.toTokenAmount(chain, balance)
      }
    }
  }

  //#endregion

  //#region logs

  async getMyBridgeLog(fromTx: BridgeTx): Promise<BridgeLog> {
    const res = await this.#actor.my_bridge_log(fromTx)
    return unwrapResult(res, 'call my_bridge_log failed')
  }

  async listMyFinalizedLogs(
    take: number,
    prev?: bigint
  ): Promise<BridgeLogInfo[]> {
    const res = await this.#actor.my_finalized_logs(
      take,
      prev === undefined ? [] : [prev]
    )
    const logs = unwrapResult(res, 'call my_finalized_logs failed')
    return logs.map((log) => this.toBridgeLogInfo(log))
  }

  async listFinalizedLogs(
    take: number,
    prev?: bigint
  ): Promise<BridgeLogInfo[]> {
    const res = await this.#actor.finalized_logs(
      take,
      prev === undefined ? [] : [prev]
    )
    const logs = unwrapResult(res, 'call finalized_logs failed')
    return logs.map((log) => this.toBridgeLogInfo(log))
  }

  toBridgeLogInfo(log: BridgeLog): BridgeLogInfo {
    const from = getChainName(log.from)
    const to = getChainName(log.to)
    return {
      id: log.id[0] ?? 0n,
      taskId: log.runtime[0]?.task_id,
      fromTransaction: log.from_tx,
      canRecheck: canRecheck(log),
      settled: logSettled(log),
      stuck: log.stuck,
      needsReview: !log.id.length && reconciliationHeld(log),
      nextPollAt: Number(log.runtime[0]?.next_poll_at ?? 0n),
      user: log.user.toText(),
      token: this.#token?.symbol || '',
      from,
      to,
      amount: this.displayAmount(log.icp_amount),
      fee: this.displayAmount(log.fee),
      fromTx: getTx(log.from_tx),
      fromTxUrl: this.#txUrl(from, log.from_tx)!,
      toTx: log.to_tx[0] && getTx(log.to_tx[0]),
      toTxUrl: this.#txUrl(to, log.to_tx[0]),
      toAddr: log.to_addr[0],
      createdAt: Number(log.created_at),
      finalizedAt: Number(log.finalized_at),
      status: getBridgingStatus(log),
      error: log.error[0]
    } as BridgeLogInfo
  }

  #txUrl(chain: string, tx?: BridgeTx): string | undefined {
    if (!tx) return undefined
    const hash = getTx(tx)
    if (!hash) return undefined
    return getChain(chain).txUrl(hash, this.tokenOn(chain)[0]) || undefined
  }

  //#endregion

  //#region transfers

  // New deposits only. Recovery must never require the already-debited funds
  // to still be present in the user's source account.
  async #assertSufficientSourceBalance(
    fromChain: string,
    amount: bigint
  ): Promise<void> {
    let required = this.toChainAmount(fromChain, amount)
    if (fromChain === 'ICP') {
      // ICP deposits go through icrc2_transfer_from, so the ledger takes its fee
      // from the user's account on top of the amount
      required += this.#token?.fee ?? 0n
    }

    let balance: bigint
    try {
      balance = await this.#sourceChainBalance(fromChain)
    } catch (err) {
      console.error(`Failed to read ${fromChain} balance before bridging:`, err)
      return
    }

    if (balance < required) {
      const symbol = this.#token?.symbol ?? 'token'
      throw new Error(
        `Insufficient ${symbol} balance on ${fromChain}: ` +
          `need ${this.displayAmount(amount)}, ` +
          `have ${this.displayAmount(this.toTokenAmount(fromChain, balance))}`
      )
    }
  }

  /**
   * Balance the bridge would draw from on the source chain, in that chain's
   * units.
   *
   * This asks `this` canister for the address, which is right only while `this`
   * is the main bridge — the one that holds the key and publishes the addresses.
   * That holds today because sub-bridge development is paused; when it resumes,
   * read the address from the main bridge here too, the way
   * {@link myAccountOn} already does, or this guard will compare an amount
   * against an address nobody funded.
   */
  async #sourceChainBalance(fromChain: string): Promise<bigint> {
    switch (fromChain) {
      case 'ICP': {
        const icp = await this.loadICPTokenAPI()
        return await icp.balance()
      }
      case 'SOL': {
        const svm = await this.loadSvmTokenAPI()
        if (!svm) throw new Error('SOL is not supported by this bridge')
        return await svm.getSplBalance(await this.mySvmAddress())
      }
      default: {
        const evm = await this.loadEVMTokenAPI(fromChain)
        return await evm.getErc20Balance(await this.myEvmAddress())
      }
    }
  }

  async prepareBridgeRequest(
    from: string,
    to: string,
    amount: bigint,
    recipient = ''
  ): Promise<BridgeRequest> {
    const owner = this.#owner()
    const canister = this.canisterId.toText()
    const prepare = () =>
      prepareRequest(localStorage, canister, owner, {
        from,
        to,
        amount: String(amount),
        recipient: recipient.trim()
      })
    // Serialise creation across tabs; retries share the persisted ID.
    return withRequestLock(canister, owner, prepare)
  }

  async submitBridgeRequest(request: BridgeRequest): Promise<BridgingProgress> {
    if (request.canister !== this.canisterId.toText())
      throw new Error('This request belongs to another bridge')
    const assertOwner = () => {
      if (this.#owner() !== request.owner)
        throw new Error(
          'The signed-in account changed. Sign in with the original account to continue this request.'
        )
    }
    try {
      const tx = await withRequestLock(request.canister, request.owner, () =>
        submitRequest(localStorage, request, {
          assertOwner,
          prepare: async () => {
            await this.refreshState()
            assertOwner()
            const reason = this.readiness([request.from, request.to])
            if (reason) throw new Error(reason)
            await this.#assertSufficientSourceBalance(
              request.from,
              BigInt(request.amount)
            )
            assertOwner()
            if (request.from === 'ICP') {
              const ledger = await this.loadICPTokenAPI()
              assertOwner()
              await ledger.ensureAllowance(
                this.canisterId,
                BigInt(request.amount) + (this.token?.fee ?? 0n),
                request.owner
              )
            }
          },
          send: async (saved, id) =>
            unwrapResult(
              await this.#actor.bridge_with_id(
                saved.from,
                saved.to,
                BigInt(saved.amount),
                saved.recipient ? [saved.recipient] : [],
                id
              ),
              'Bridge request could not complete'
            )
        })
      )
      return BridgingProgress.track(this, tx)
    } finally {
      this.#activityChanged()
    }
  }

  #owner(): string {
    const principal = dynAgent.id.getPrincipal()
    if (principal.isAnonymous()) throw new Error('Sign in to continue')
    return principal.toText()
  }

  #activityChanged() {
    if (typeof window !== 'undefined')
      window.dispatchEvent(new Event('bridge-activity'))
  }

  async listOperations(take = 20, before?: bigint): Promise<OperationInfo[]> {
    return unwrapResult(
      await this.#actor.my_operations(
        take,
        before === undefined ? [] : [before]
      ),
      'Could not load operations'
    )
  }

  async listPendingLogs(
    mine: boolean,
    take = 20,
    after?: bigint
  ): Promise<BridgeLogInfo[]> {
    const cursor: [] | [bigint] = after === undefined ? [] : [after]
    const result = !this.supportsRecovery
      ? mine
        ? await this.#actor.my_pending_logs()
        : await this.#actor.pending_logs()
      : mine
        ? await this.#actor.my_pending_logs_page(take, cursor)
        : await this.#actor.pending_logs_page(take, cursor)
    return unwrapResult(result, 'Could not load pending tasks').map((log) =>
      this.toBridgeLogInfo(log)
    )
  }

  async resumeOperation(
    operation: OperationInfo
  ): Promise<BridgingProgress | null> {
    const owner = this.#owner()
    if (owner !== operation.owner.toText())
      throw new Error('This operation belongs to another account')
    try {
      const tx = unwrapResult(
        await this.#actor.resume_operation(operation.id),
        'Could not resume operation'
      )
      if (owner !== this.#owner())
        throw new Error('The signed-in account changed')
      return operation.kind === 'deposit'
        ? BridgingProgress.track(this, tx)
        : null
    } finally {
      this.#activityChanged()
    }
  }

  async cancelOperation(operation: OperationInfo): Promise<void> {
    if (this.#owner() !== operation.owner.toText())
      throw new Error('This operation belongs to another account')
    try {
      unwrapResult(
        await this.#actor.cancel_operation(operation.id),
        'Could not cancel operation'
      )
    } finally {
      this.#activityChanged()
    }
  }

  async recheckTask(tx: BridgeTx): Promise<void> {
    this.#owner()
    try {
      unwrapResult(await this.#actor.recheck_task(tx), 'Could not recheck task')
    } finally {
      this.#activityChanged()
    }
  }

  // the transfer transactions below are signed by the canister; the browser
  // only broadcasts them through the chain's own RPC
  async buildErc20TransferTx(
    chain: string,
    toAddr: string,
    amount: bigint
  ): Promise<string> {
    const tx = await this.#actor.erc20_transfer_tx(chain, toAddr, amount)
    return unwrapResult(tx, 'call erc20_transfer_tx failed')
  }

  async buildEvmTransferTx(
    chain: string,
    toAddr: string,
    evmAmount: bigint
  ): Promise<string> {
    const tx = await this.#actor.evm_transfer_tx(chain, toAddr, evmAmount)
    return unwrapResult(tx, 'call evm_transfer_tx failed')
  }

  async buildSplTransferTx(toAddr: string, amount: bigint): Promise<string> {
    const tx = await this.#actor.spl_transfer_tx(toAddr, amount)
    return unwrapResult(tx, 'call spl_transfer_tx failed')
  }

  async buildSolTransferTx(toAddr: string, solAmount: bigint): Promise<string> {
    const tx = await this.#actor.sol_transfer_tx(toAddr, solAmount)
    return unwrapResult(tx, 'call sol_transfer_tx failed')
  }

  //#endregion
}

export class BridgingProgress {
  #api: BridgeCanisterAPI
  #tx: BridgeTx
  #log = $state<BridgeLog | null>(null)
  #isComplete = $derived.by(() => isFinalized(this.#log?.to_tx[0]))
  #owner = dynAgent.id.getPrincipal().toText()
  #timer: ReturnType<typeof setTimeout> | undefined
  #stopped = false
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

  stop() {
    this.#stopped = true
    clearTimeout(this.#timer)
  }

  get isSettled(): boolean {
    return this.#log ? logSettled(this.#log) || this.#log.stuck : false
  }

  #refreshLog = async (): Promise<void> => {
    if (this.#stopped || dynAgent.id.getPrincipal().toText() !== this.#owner)
      return
    try {
      const log = await this.#api.getMyBridgeLog(this.#tx)
      if (this.#stopped || dynAgent.id.getPrincipal().toText() !== this.#owner)
        return
      this.#log = log
      await tick()
      if (!this.isSettled) {
        this.#timer = setTimeout(() => this.#refreshLog(), 5000)
      }
    } catch (error) {
      console.error(`Error refreshing log ${this.#tx}:`, error)
      // keep polling: a transient failure must not strand the UI in "Bridging..."
      if (!this.#stopped)
        this.#timer = setTimeout(() => this.#refreshLog(), 5000)
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
    if (logSettled(this.#log))
      return (
        this.#log.error[0] ??
        'This transfer was closed without a completed payout.'
      )
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
  Sol?: string
}

export class TransferingProgress {
  #api: BridgeCanisterAPI
  #tx = $state<TransferTxInfo | null>(null)
  #error = $state<string | null>(null)
  #owner = dynAgent.id.getPrincipal().toText()
  #timer: ReturnType<typeof setTimeout> | undefined
  #stopped = false

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

  stop() {
    this.#stopped = true
    clearTimeout(this.#timer)
  }

  #refreshLog = async (): Promise<void> => {
    if (this.#stopped || dynAgent.id.getPrincipal().toText() !== this.#owner)
      return
    if (!this.#tx || this.#tx.isFinalized || this.#error) return

    try {
      if ('Evm' in this.#tx) {
        const evm = await this.#api.loadEVMTokenAPI(this.#tx.chain)
        const receipt = await evm.getTransactionReceipt(this.#tx.Evm)
        if (receipt) {
          if (receipt.status === '0x1') {
            this.#tx.isFinalized = true
          } else {
            // the transaction was mined but reverted, it will never finalize
            this.#error = `transaction reverted on ${this.#tx.chain}`
          }
          return
        }
        if (!this.#stopped)
          this.#timer = setTimeout(() => this.#refreshLog(), 5000)
      } else if ('Sol' in this.#tx) {
        const sol = await this.#api.loadSvmTokenAPI()
        const status = sol ? await sol.getTransactionStatus(this.#tx.Sol) : ''
        if (status === 'finalized') {
          this.#tx.isFinalized = true
          return
        }
        if (status === 'failed') {
          // it landed and failed, so it moved nothing and will never finalize
          this.#error = `transaction failed on ${this.#tx.chain}`
          return
        }

        if (!this.#stopped)
          this.#timer = setTimeout(() => this.#refreshLog(), 5000)
      }
    } catch (error) {
      console.error(`Error refreshing log ${this.#tx}:`, error)
      // keep polling: a transient failure must not strand the UI in "Transfering..."
      if (!this.#stopped)
        this.#timer = setTimeout(() => this.#refreshLog(), 5000)
    }
  }

  get status(): BridgingStatus {
    if (this.#error) {
      return 'Error'
    }
    if (this.#tx?.isFinalized) {
      return 'Completed'
    }
    return 'Pending'
  }

  get isComplete(): boolean {
    return this.#tx?.isFinalized || false
  }

  // the progress is settled once the transaction finalized or failed, only then
  // the UI may be reset for a new transfer
  get isSettled(): boolean {
    return this.isComplete || !!this.#error
  }

  get chain(): string {
    return this.#tx?.chain || ''
  }

  get tx(): string {
    if (!this.#tx) return ''

    if ('Evm' in this.#tx) {
      return this.#tx.Evm
    } else if ('Sol' in this.#tx) {
      return this.#tx.Sol
    } else if ('Icp' in this.#tx) {
      return this.#tx.Icp.toString()
    }

    return ''
  }

  get txUrl(): string {
    const chain = this.#tx?.chain
    if (!chain) return ''
    return getChain(chain).txUrl(this.tx, this.#api.tokenOn(chain)[0])
  }

  get message(): string {
    if (this.#error) {
      return this.#error
    }
    if (this.isComplete) {
      return ''
    }
    return `waiting for confirmation on ${this.#tx!.chain}`
  }
}

function getChainName(target: BridgeTarget): string {
  if ('Evm' in target) {
    return target.Evm
  } else if ('Sol' in target) {
    return 'SOL'
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
  } else if ('Sol' in tx) {
    const [_isFinalized, rawTx] = tx.Sol
    const bytes = rawTx instanceof Uint8Array ? rawTx : Uint8Array.from(rawTx)
    return base58.decode(bytes)
  }

  return tx.Icp[1].toString()
}

function getBridgingStatus(log?: BridgeLog | null): BridgingStatus {
  if (!log) {
    return 'Accepted'
  }
  if (isFinalized(log.to_tx[0])) {
    return 'Completed'
  }
  if (logSettled(log)) return 'Closed'
  if (log.stuck)
    return reconciliationHeld(log) ? 'Needs review' : 'Needs attention'
  if (log.error.length > 0) {
    return 'Retrying'
  }
  return 'Pending'
}

function isFinalized(tx?: BridgeTx): boolean {
  return txFinalized(tx)
}

function configKey(value: unknown): string {
  return JSON.stringify(value, (_, item) =>
    typeof item === 'bigint' ? String(item) : item
  )
}
