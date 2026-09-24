<script lang="ts">
  import {
    TransferingProgress,
    type BridgeCanisterAPI,
    type MyAddresses
  } from '$lib/canisters/bridge.svelte'
  import { getChain, type Chain } from '$lib/chains'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { formDefault, preferredBridge } from '$lib/prefs'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import { pruneAddress } from '$lib/utils/helper'
  import { type TokenInfo } from '$lib/utils/token'
  import { onMount, untrack } from 'svelte'
  import AccountAddresses from './AccountAddresses.svelte'
  import AddressInput from './AddressInput.svelte'
  import AmountInput from './AmountInput.svelte'
  import BridgeHeader from './BridgeHeader.svelte'
  import BridgeStatus from './BridgeStatus.svelte'
  import { errMessage } from '$lib/utils/tryrun'
  import NetworkSelector from './ChainSelector.svelte'
  import ConfirmAddress from './ConfirmAddress.svelte'
  import PrimaryButton from './PrimaryButton.svelte'
  import RefreshButton from './RefreshButton.svelte'
  import TokenLink from './TokenLink.svelte'

  const {
    bridges,
    active
  }: {
    // the main bridge first, already loaded
    bridges: BridgeCanisterAPI[]
    // the card is on screen; a hidden card does not poll balances
    active: boolean
  } = $props()

  const defaultFrom = formDefault('From', 'ICP')

  let alive = true
  let refreshRun = 0
  let refreshTimer: ReturnType<typeof setTimeout> | undefined
  const mainBridge = $derived(bridges[0]!)
  let myAddresses = $state<MyAddresses | null>(null)
  // only the initial value is read here; the token selector changes it
  let selectedBridge = $state<BridgeCanisterAPI>(
    untrack(() => preferredBridge(bridges)!)
  )
  const supportTokens = $derived(
    bridges.map((b) => b.token).filter((t) => t !== null)
  )
  const bridgeCanister = $derived(selectedBridge.canisterId.toText())
  const selectedToken = $derived(selectedBridge.token)
  // The state poll replaces the whole bridge state every 15 s. Effects key on
  // these plain strings instead, so they rerun only when the value changes
  const keysReady = $derived(mainBridge.runtime?.keys_ready.join() ?? '')
  const chainNames = $derived(selectedBridge.chainNames().join())
  const supportChains = $derived(
    chainNames ? chainNames.split(',').map(getChain) : []
  )
  let fromChain = $state<Chain | null>(null)
  let fromAddress = $state<string>('')
  let fromBalance = $state<bigint>(0n)
  let fromBalanceNative = $state<bigint>(0n)
  let gasFee = $state<bigint>(0n)
  let nativeToken = $state<boolean>(false)
  let thirdAddress = $state<string>('')
  let confirmAddress = $state<boolean>(false)
  let fromAmount = $state('')
  let error = $state<string | null>(null)
  let accountError = $state('')
  let feeWarning = $state<string | undefined>()
  const readiness = $derived(
    fromChain?.name === 'ICP'
      ? null
      : selectedBridge.readiness([fromChain?.name ?? ''], false, nativeToken)
  )
  let isLoading = $state<boolean>(false)
  let isTransfering = $state<boolean>(false)
  let transferingProgress = $state<TransferingProgress | null>(null)
  const disabledTransfering = $derived.by(() => {
    return !!(
      isTransfering ||
      error ||
      accountError ||
      readiness ||
      !thirdAddress ||
      !confirmAddress
    )
  })

  // the deposit addresses change only with the signing keys
  $effect(() => {
    const bridge = mainBridge
    void keysReady
    return untrack(() =>
      toastRun(async (signal) => {
        const addresses = await bridge.myAddresses(
          authStore.identity.getPrincipal().toText()
        )
        if (!signal.aborted) myAddresses = addresses
      })
    )
  })

  // keep the chosen chain while the bridge still reaches it
  $effect(() => {
    const chains = supportChains
    untrack(() => {
      const name = fromChain?.name || defaultFrom
      fromChain = chains.find((c) => c.name === name) ?? chains[0] ?? null
    })
  })

  // balances follow the selection; the gas fee and the amount unit also
  // depend on whether the native coin is sent
  $effect(() => {
    void [selectedBridge, fromChain?.name, nativeToken, myAddresses]
    untrack(() => refreshMyTokenInfo())
  })

  // catch up on what moved while the card was hidden
  let wasActive = untrack(() => active)
  $effect(() => {
    if (active && !wasActive) untrack(() => refreshMyTokenInfo())
    wasActive = active
  })

  function resetTransfer() {
    isTransfering = false
    thirdAddress = ''
    confirmAddress = false
    fromAmount = ''
    error = null
    transferingProgress?.stop()
    transferingProgress = null

    refreshMyTokenInfo()
  }

  // the amount and the destination address share the `error` slot, so each
  // field reports through the same helper and editing one must not drop the
  // other one's error
  function reportValidity(event: Event, err: string) {
    ;(event.target as HTMLInputElement).setCustomValidity(err)
    error = err || null
  }

  function validateSendAmount(event: Event) {
    reportValidity(event, validateAmount()[1])
  }

  function validateThirdAddress(event: Event) {
    const addr = thirdAddress.trim()
    if (addr !== thirdAddress) thirdAddress = addr
    reportValidity(event, validateToAddress(addr, false))
  }

  // full check used on submit
  function validateTransfer(): [bigint, string] {
    const [amount, err] = validateAmount()
    return [amount, err || validateToAddress(thirdAddress.trim(), true)]
  }

  function validateToAddress(addr: string, required: boolean): string {
    if (!addr) {
      return required ? 'Destination address is required' : ''
    }
    if (addr === fromAddress) {
      return 'The destination address cannot be the same as the source address'
    }
    if (fromChain && !fromChain.isValidAddress(addr)) {
      return `Invalid ${fromChain.name} address format`
    }
    return ''
  }

  function validateAmount(): [bigint, string] {
    try {
      return validateExactAmount()
    } catch (err) {
      return [0n, errMessage(err)]
    }
  }

  function validateExactAmount(): [bigint, string] {
    if (!selectedBridge.token || !fromChain) {
      return [0n, '']
    }

    const value = fromAmount || '0'
    if (nativeToken) {
      const spendable = fromBalanceNative - gasFee
      const amount = selectedBridge.parseNativeAmount(fromChain.name, value)
      if (amount > spendable) {
        const balance = selectedBridge.displayNativeAmount(
          fromChain.name,
          spendable
        )
        return [
          amount,
          `Insufficient ${fromChain.name} balance, should be less than ${balance}`
        ]
      }
      return [amount, '']
    }

    // ICP charges the ledger fee in the token, so it is not spendable
    const spendable = fromBalance - (fromChain.name === 'ICP' ? gasFee : 0n)
    const amount = selectedBridge.parseAmount(value)
    let err = ''
    if (amount <= 0n) {
      err = 'Enter an amount greater than zero'
    } else if (
      selectedBridge.toTokenAmount(
        fromChain.name,
        selectedBridge.toChainAmount(fromChain.name, amount)
      ) !== amount
    ) {
      err = `The amount has more precision than ${fromChain.name} supports`
    } else if (amount > spendable) {
      err = `Insufficient balance, should be less than ${selectedBridge.displayAmount(
        spendable
      )}`
    } else if (fromChain.name !== 'ICP' && fromBalanceNative < gasFee) {
      err = `Insufficient ${fromChain.name} balance to cover gas fee`
    }

    return [amount, err]
  }

  async function refreshMyTokenInfo(all: boolean = false) {
    // only the latest run may write, so a slow answer for a chain the user
    // already left cannot land on the one now selected
    const run = ++refreshRun
    const bridge = selectedBridge
    const from = fromChain
    const native = nativeToken
    const addresses = myAddresses
    if (!from || !addresses) {
      fromBalance = 0n
      isLoading = false
      return
    }

    try {
      isLoading = true
      if (all) await Promise.all(bridges.map((b) => b.refreshState()))

      const account = await bridge.myAccountOn(from.name, addresses, native)
      if (run !== refreshRun) return
      fromAddress = account.address
      accountError = ''
      feeWarning = account.feeWarning
      fromBalance = account.tokenBalance
      fromBalanceNative = account.nativeBalance
      // a token transfer on ICP pays the ledger fee in the token itself;
      // everything else pays the chain's own fee
      gasFee =
        from.name === 'ICP' && !native ? bridge.token!.fee : account.nativeFee
    } catch (err) {
      if (run === refreshRun) accountError = errMessage(err)
    } finally {
      if (run === refreshRun) isLoading = false
    }
  }

  function onSelectNativeToken() {
    // the amount unit changes with it; the balances refresh on their own
    error = null
  }

  function onSelectToken(token: TokenInfo) {
    const bridge = bridges.find((b) => b.token?.name === token.name)
    nativeToken = false
    if (bridge) {
      selectedBridge = bridge
    }
  }

  function onSelectFromChain(chain: Chain) {
    fromChain = chain
  }

  async function onTransfer() {
    const [amount, err] = validateTransfer()
    error = err || null
    if (disabledTransfering || err || amount <= 0n) return
    if (!selectedBridge.state || !selectedBridge.token || !fromChain) return

    const owner = authStore.identity.getPrincipal().toText()
    const assertOwner = () => {
      if (!alive || owner !== authStore.identity.getPrincipal().toText())
        throw new Error(
          'The signed-in account changed. Please start again with the correct account.'
        )
    }
    const chain = fromChain.name
    isTransfering = true
    toastRun(async () => {
      try {
        assertOwner()
        if (chain === 'ICP') {
          const icp = await selectedBridge.loadICPTokenAPI()
          assertOwner()
          const idx = nativeToken
            ? await icp.transferICP(thirdAddress, amount)
            : await icp.transfer(thirdAddress, amount)
          assertOwner()
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain,
            native: nativeToken,
            // an ICP ledger transfer either returns a block index or throws
            isFinalized: true,
            Icp: idx
          })
        } else if (chain === 'SOL') {
          const svm = await selectedBridge.loadSvmTokenAPI()
          assertOwner()
          const signedTx = nativeToken
            ? await selectedBridge.buildSolTransferTx(thirdAddress, amount)
            : await selectedBridge.buildSplTransferTx(thirdAddress, amount)
          assertOwner()
          const tx = await svm!.sendRawTransaction(signedTx)
          assertOwner()
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain,
            native: nativeToken,
            isFinalized: false,
            Sol: tx
          })
        } else {
          const evm = await selectedBridge.loadEVMTokenAPI(chain)
          assertOwner()
          const signedTx = nativeToken
            ? await selectedBridge.buildEvmTransferTx(
                chain,
                thirdAddress,
                amount
              )
            : await selectedBridge.buildErc20TransferTx(
                chain,
                thirdAddress,
                amount
              )
          assertOwner()
          const tx = await evm.sendRawTransaction(signedTx)
          assertOwner()
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain,
            native: nativeToken,
            isFinalized: false,
            Evm: tx
          })
        }

        refreshMyTokenInfo()
        refreshTimer = setTimeout(() => {
          if (alive) refreshMyTokenInfo()
        }, 5000)
      } catch (err) {
        isTransfering = false
        throw err
      }
    })
  }

  onMount(() => {
    // balances also move outside the app. A run still in flight is left to
    // finish rather than superseded
    const timer = setInterval(() => {
      if (active && !document.hidden && !isLoading) refreshMyTokenInfo()
    }, 15_000)
    return () => {
      alive = false
      clearInterval(timer)
      clearTimeout(refreshTimer)
      transferingProgress?.stop()
    }
  })
</script>

<div
  class="space-y-6 rounded-xl border border-white/10 bg-[#131721]/80 p-6 pb-10 text-white/90 shadow-2xl backdrop-blur"
>
  {#key bridgeCanister}
    <RefreshButton {isLoading} onclick={() => refreshMyTokenInfo(true)} />
    <AccountAddresses addresses={myAddresses} />
    <BridgeStatus
      bridge={selectedBridge}
      chains={fromChain ? [fromChain.name] : []}
      wallet
      native={nativeToken}
    />
    {#if accountError}<p role="alert" class="text-sm break-words text-red-300"
        >{accountError}</p
      >{/if}
    {#if feeWarning}<p class="text-xs text-amber-200">{feeWarning}</p>{/if}

    <div class="relative">
      <BridgeHeader
        bridge={selectedBridge}
        tokens={supportTokens}
        {selectedToken}
        {onSelectToken}
        disabled={isLoading || isTransfering}
        dimmed={nativeToken}
      />
    </div>

    <div class="grid grid-cols-[1fr_1fr] items-center justify-center gap-4">
      <!-- From Section -->
      <div class="">
        <p class="mb-1 flex items-center gap-2 text-sm text-white/60">
          <span>Chain</span>
          {#if !nativeToken}
            <TokenLink
              bridge={selectedBridge}
              chain={fromChain?.name}
              label="Token"
            />
          {/if}
        </p>
        <NetworkSelector
          disabled={isLoading || isTransfering}
          selectedChain={fromChain}
          disabledChainName={''}
          onSelectChain={onSelectFromChain}
          chains={supportChains}
          containerClass="rounded-xl border border-white/40 shrink-0"
        />
      </div>
      <div class="">
        <p class="collapse mb-1 text-sm text-white/60">-</p>
        <label
          class="flex items-center text-sm font-medium text-white/90 rtl:text-right"
          ><input
            type="checkbox"
            name="nativeToken"
            disabled={isLoading || isTransfering}
            bind:checked={nativeToken}
            onchange={onSelectNativeToken}
            class="text-primary-600 me-2 size-4 shrink-0 rounded-sm border-gray-300 bg-gray-100 ring-0 disabled:cursor-not-allowed"
          />Native Token</label
        >
      </div>
    </div>

    <div class="relative">
      <p class="mb-1 flex items-center gap-1 text-sm text-white/60">
        <span>From {fromChain?.name} address:</span>
        {#if fromAddress}
          <span>{pruneAddress(fromAddress)}</span>
        {/if}
      </p>
      <AmountInput
        bind:value={fromAmount}
        disabled={isLoading || isTransfering}
        oninput={validateSendAmount}
      />
      <div class="mt-1 flex items-center gap-2 text-sm text-white/60">
        <span>Your balance: {selectedBridge.displayAmount(fromBalance)}</span>
        <span class:text-white={nativeToken}
          >Native {fromChain?.name} balance: {selectedBridge.displayNativeAmount(
            fromChain?.name!,
            fromBalanceNative
          )}</span
        >
        <span
          >Gas fee: ~{selectedBridge.displayNativeAmount(
            fromChain?.name!,
            gasFee
          )}</span
        >
      </div>
    </div>

    <div class="relative">
      <p class="mb-1 flex items-center gap-1 text-sm text-white/60">
        <span>To {fromChain?.name} address:</span>
      </p>
      <AddressInput
        bind:value={thirdAddress}
        disabled={isLoading || isTransfering}
        oninput={validateThirdAddress}
      />
      <ConfirmAddress
        bind:checked={confirmAddress}
        disabled={isLoading || isTransfering}
      />
    </div>

    <div class="relative">
      {#if error}
        <p class="mb-1 text-sm text-red-400">{error}</p>
      {/if}
      {#if transferingProgress}
        {@const message = transferingProgress.message}
        {@const tx = transferingProgress.tx}
        {@const txUrl = transferingProgress.txUrl}
        {#if message}
          <p class="mb-1 text-sm text-green-500"
            >{transferingProgress.status}: {message}</p
          >
        {/if}
        {#if tx && txUrl}
          <a
            class="mb-1 flex items-center gap-1 text-sm font-medium text-green-500"
            href={txUrl}
            target="_blank"
          >
            <span>{transferingProgress.chain + ' Tx: ' + pruneAddress(tx)}</span
            >
            <span class="*:size-4"><ArrowRightUpLine /></span>
          </a>
        {/if}

        <PrimaryButton
          onclick={resetTransfer}
          disabled={!transferingProgress.isSettled}
          isLoading={!transferingProgress.isSettled}
        >
          {#if transferingProgress.isComplete}
            <span class="text-green-500">Transfer completed, start again</span>
          {:else if transferingProgress.isSettled}
            <span class="text-red-400">Transfer failed, start again</span>
          {:else}
            <span>Transfering...</span>
          {/if}
        </PrimaryButton>
      {:else}
        <PrimaryButton
          onclick={onTransfer}
          disabled={disabledTransfering}
          isLoading={isLoading || isTransfering}
        >
          {isTransfering ? 'Transfering...' : 'Transfer tokens'}
        </PrimaryButton>
      {/if}
    </div>
  {/key}
</div>
