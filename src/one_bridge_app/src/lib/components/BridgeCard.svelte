<script lang="ts">
  import {
    addressOn,
    BridgingProgress,
    type BridgeCanisterAPI,
    type MyAddresses
  } from '$lib/canisters/bridge.svelte'
  import { getChain, type Chain } from '$lib/chains'
  import ArrowLeftRightLine from '$lib/icons/arrow-left-right-line.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { formDefault, preferredBridge, rememberForm } from '$lib/prefs'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import { pruneAddress } from '$lib/utils/helper'
  import { type TokenInfo } from '$lib/utils/token'
  import { onMount, untrack } from 'svelte'
  import { innerWidth } from 'svelte/reactivity/window'
  import AccountAddresses from './AccountAddresses.svelte'
  import AddressInput from './AddressInput.svelte'
  import AmountInput from './AmountInput.svelte'
  import BridgeHeader from './BridgeHeader.svelte'
  import BridgeStatus from './BridgeStatus.svelte'
  import {
    readRequest,
    forgetRequest,
    withRequestLock,
    type BridgeRequest
  } from '$lib/utils/bridge-request'
  import { errMessage } from '$lib/utils/tryrun'
  import NetworkSelector from './ChainSelector.svelte'
  import ConfirmAddress from './ConfirmAddress.svelte'
  import PrimaryButton from './PrimaryButton.svelte'
  import RefreshButton from './RefreshButton.svelte'
  import TokenLink from './TokenLink.svelte'

  const {
    isAuthenticated,
    onSignIn,
    bridges,
    active
  }: {
    isAuthenticated: boolean
    onSignIn: () => Promise<void>
    // the main bridge first; empty until it has loaded
    bridges: BridgeCanisterAPI[]
    // the card is on screen; a hidden card does not poll balances
    active: boolean
  } = $props()

  const defaultFrom = formDefault('From', 'ICP')
  const defaultTo = formDefault('To', 'BNB')

  const mainBridge = $derived(bridges[0] ?? null)
  let myAddresses = $state<MyAddresses | null>(null)
  let selectedBridge = $state<BridgeCanisterAPI | null>(null)
  const supportTokens = $derived(
    bridges.map((b) => b.token).filter((t) => t !== null)
  )
  const bridgeCanister = $derived(
    selectedBridge ? selectedBridge.canisterId.toText() : ''
  )
  const selectedToken = $derived(selectedBridge?.token ?? null)
  // The state poll replaces the whole bridge state every 15 s. Effects key on
  // these plain strings instead, so they rerun only when the value changes
  const keysReady = $derived(mainBridge?.runtime?.keys_ready.join() ?? '')
  const chainNames = $derived(selectedBridge?.chainNames().join() ?? '')
  const supportChains = $derived(
    chainNames ? chainNames.split(',').map(getChain) : []
  )
  let fromChain = $state<Chain | null>(null)
  let toChain = $state<Chain | null>(null)
  let fromAddress = $state<string>('')
  let fromBalance = $state<bigint>(0n)
  let fromBalanceNative = $state<bigint>(0n)
  let bridgeReserve = $state<bigint>(0n)
  let gasFee = $state<bigint>(0n)
  let toAddress = $state<string>('')
  let thirdAddress = $state<string>('')
  let confirmAddress = $state<boolean>(false)
  let fromAmount = $state('')
  const receivedAmount = $derived.by(() => {
    if (!selectedBridge || !fromAmount) return null
    try {
      const value =
        selectedBridge.parseAmount(fromAmount) - selectedBridge.bridgeFee
      return value > 0n ? value : null
    } catch {
      return null
    }
  })
  let error = $state<string | null>(null)
  const bridgeError = $derived(
    selectedBridge?.readiness(
      [fromChain?.name ?? '', toChain?.name ?? ''].filter(Boolean)
    ) ?? null
  )
  let accountError = $state('')
  let feeWarning = $state<string | undefined>()
  let savedRequest = $state<BridgeRequest | null>(null)
  let savedError = $state('')
  let actionError = $state('')
  let alive = true
  let refreshRun = 0
  let isLoading = $state<boolean>(false)
  let isSigningIn = $state<boolean>(false)
  let isBridging = $state<boolean>(false)
  let bridgingProgress = $state<BridgingProgress | null>(null)
  const disabledBridging = $derived.by(() => {
    return !!(
      isBridging ||
      savedRequest ||
      savedError ||
      accountError ||
      bridgeError ||
      error ||
      (thirdAddress && !confirmAddress)
    )
  })

  // pick the remembered token once the bridges have loaded
  $effect(() => {
    const loaded = bridges
    if (!selectedBridge || !loaded.includes(selectedBridge))
      selectedBridge = untrack(() => preferredBridge(loaded))
  })

  // keep the chosen chains while the bridge still reaches them
  $effect(() => {
    const chains = supportChains
    untrack(() => {
      const fromName = fromChain?.name || defaultFrom
      const toName = toChain?.name || defaultTo
      fromChain = chains.find((c) => c.name === fromName) ?? chains[0] ?? null
      toChain =
        chains.find((c) => c.name === toName && c.name !== fromChain?.name) ??
        chains.find((c) => c.name !== fromChain?.name) ??
        null
    })
  })

  // the deposit addresses change only with the account and the signing keys
  $effect(() => {
    const bridge = mainBridge
    void keysReady
    if (!bridge || !isAuthenticated) {
      myAddresses = null
      return
    }

    return untrack(() =>
      toastRun(async (signal) => {
        const addresses = await bridge.myAddresses(
          authStore.identity.getPrincipal().toText()
        )
        if (!signal.aborted) myAddresses = addresses
      })
    )
  })

  // balances follow the selection
  $effect(() => {
    void [selectedBridge, fromChain?.name, toChain?.name, myAddresses]
    untrack(() => refreshMyTokenInfo())
  })

  $effect(() => {
    void selectedBridge
    untrack(loadSavedRequest)
  })

  // catch up on what moved while the card was hidden
  let wasActive = untrack(() => active)
  $effect(() => {
    if (active && !wasActive) untrack(() => refreshMyTokenInfo())
    wasActive = active
  })

  function resetBridge() {
    isBridging = false
    thirdAddress = ''
    confirmAddress = false
    fromAmount = ''
    error = null
    bridgingProgress?.stop()
    bridgingProgress = null
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
    reportValidity(event, validateToAddress(addr))
  }

  // full check used on submit
  function validateBridge(): [bigint, string] {
    const [amount, err] = validateAmount()
    return [amount, err || validateToAddress(thirdAddress.trim())]
  }

  function validateToAddress(addr: string): string {
    if (addr && toChain && !toChain.isValidAddress(addr)) {
      return `Invalid ${toChain.name} address format`
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
    if (!selectedBridge?.token || !fromChain || !toChain) {
      return [0n, '']
    }

    // an ICP deposit pays the ledger fee twice, for the approval and for the
    // bridge's transfer, and neither is spendable
    const spendable =
      fromBalance - (fromChain.name === 'ICP' ? 2n * gasFee : 0n)
    const amount = selectedBridge.parseAmount(fromAmount || '0')
    let err =
      selectedBridge.validateBridgePrecision(
        fromChain.name,
        toChain.name,
        amount
      ) ?? ''
    if (err) return [amount, err]
    if (amount < selectedBridge.minAmount) {
      err = `Minimum bridge amount is ${selectedBridge.displayAmount(
        selectedBridge.minAmount
      )}`
    } else if (amount > spendable) {
      err = `Insufficient balance, should be less than ${selectedBridge.displayAmount(
        spendable
      )}`
    } else if (amount - selectedBridge.bridgeFee > bridgeReserve) {
      err = 'Bridge has insufficient balance'
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
    const to = toChain
    const addresses = myAddresses
    if (!bridge || !from || !isAuthenticated || !addresses) {
      fromAddress = ''
      fromBalance = 0n
      toAddress = ''
      isLoading = false
      return
    }

    try {
      isLoading = true
      if (all) await Promise.all(bridges.map((b) => b.refreshState()))

      const [account, reserve] = await Promise.all([
        bridge.myAccountOn(from.name, addresses),
        to ? bridge.reserveOn(to.name) : 0n
      ])
      if (run !== refreshRun) return
      fromAddress = account.address
      feeWarning = account.feeWarning
      accountError = ''
      fromBalance = account.tokenBalance
      fromBalanceNative = account.nativeBalance
      // on ICP the ledger takes its fee in the token itself; every other chain
      // pays gas out of its native coin
      gasFee = from.name === 'ICP' ? bridge.token!.fee : account.nativeFee
      if (to) {
        toAddress = addressOn(to.name, addresses)
        bridgeReserve = reserve
      }
    } catch (err) {
      if (run === refreshRun) accountError = errMessage(err)
    } finally {
      if (run === refreshRun) isLoading = false
    }
  }

  function onSelectToken(token: TokenInfo) {
    const bridge = bridges.find((b) => b.token?.name === token.name)
    if (bridge) {
      selectedBridge = bridge
    }
  }

  function onSwapChains() {
    ;[fromChain, toChain] = [toChain, fromChain]
  }

  function onSelectFromChain(chain: Chain) {
    fromChain = chain
    if (toChain?.name === chain.name) {
      toChain = supportChains.find((c) => c.name !== chain.name) || null
    }
  }

  function onSelectToChain(chain: Chain) {
    toChain = chain
    if (fromChain?.name === chain.name) {
      fromChain = supportChains.find((c) => c.name !== chain.name) || null
    }
  }

  function loadSavedRequest() {
    if (!selectedBridge || !isAuthenticated) return
    try {
      savedRequest = readRequest(
        localStorage,
        selectedBridge.canisterId.toText(),
        authStore.identity.getPrincipal().toText()
      )
      savedError = ''
    } catch (err) {
      savedError = errMessage(err)
    }
  }

  async function continueSaved() {
    const bridge = selectedBridge
    const request = savedRequest
    if (!bridge || !request || isBridging) return
    isBridging = true
    actionError = ''
    try {
      bridgingProgress?.stop()
      const next = await bridge.submitBridgeRequest(request)
      if (!alive) {
        next.stop()
        return
      }
      bridgingProgress = next
      rememberForm(bridge.token?.symbol ?? '', request.from, request.to)
      refreshMyTokenInfo()
    } catch (err) {
      if (alive) actionError = errMessage(err)
    } finally {
      if (alive) {
        isBridging = false
        loadSavedRequest()
      }
    }
  }

  async function dismissSaved() {
    const previous = savedRequest
    if (!previous || isBridging) return
    isBridging = true
    try {
      await withRequestLock(previous.canister, previous.owner, () => {
        const current = readRequest(
          localStorage,
          previous.canister,
          previous.owner
        )
        if (!alive || !current || current.id !== previous.id) return
        if (
          current.stage === 'submitted' &&
          !confirm(
            'This request may already have moved funds. Check Operations & recovery first. Clearing it only removes this browser’s shortcut; it does not cancel the payment. Have you verified the outcome and want to clear it?'
          )
        )
          return
        forgetRequest(localStorage, current)
        actionError = ''
        resetBridge()
      })
    } catch (err) {
      savedError = errMessage(err)
    } finally {
      if (alive) {
        isBridging = false
        loadSavedRequest()
      }
    }
  }

  async function onBridge() {
    if (!selectedBridge || !fromChain || !toChain || disabledBridging) return
    const [amount, err] = validateBridge()
    error = err || null
    if (err || amount <= 0n) return
    isBridging = true
    actionError = ''
    try {
      savedRequest = await selectedBridge.prepareBridgeRequest(
        fromChain.name,
        toChain.name,
        amount,
        thirdAddress
      )
      rememberForm(
        selectedBridge.token?.symbol ?? '',
        fromChain.name,
        toChain.name
      )
    } catch (err) {
      actionError = errMessage(err)
    } finally {
      isBridging = false
      loadSavedRequest()
    }
    if (savedRequest && !actionError) await continueSaved()
  }

  onMount(() => {
    const refresh = () => loadSavedRequest()
    // balances also move outside the app, e.g. a deposit from an exchange. A
    // run still in flight is left to finish rather than superseded
    const timer = setInterval(() => {
      if (active && !document.hidden && !isLoading) refreshMyTokenInfo()
    }, 15_000)
    window.addEventListener('storage', refresh)
    window.addEventListener('bridge-activity', refresh)
    return () => {
      alive = false
      clearInterval(timer)
      bridgingProgress?.stop()
      window.removeEventListener('storage', refresh)
      window.removeEventListener('bridge-activity', refresh)
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
      chains={[fromChain?.name ?? '', toChain?.name ?? ''].filter(Boolean)}
    />
    {#if savedError}<p role="alert" class="text-sm break-words text-red-300"
        >{savedError}</p
      >{/if}
    {#if savedRequest}
      <section
        aria-label="Saved bridge request"
        class="space-y-3 rounded-lg border border-cyan-300/25 bg-cyan-300/5 p-4"
      >
        <p class="font-medium text-cyan-100"
          >{savedRequest.stage === 'accepted'
            ? 'Your bridge request was accepted'
            : 'You have a saved bridge request'}</p
        >
        <p class="text-sm text-white/70"
          >{savedRequest.from} → {savedRequest.to} · {selectedBridge?.displayAmount(
            BigInt(savedRequest.amount)
          )}
          {selectedBridge?.token?.symbol}</p
        >
        {#if savedRequest.recipient}<p class="text-xs break-all text-white/50"
            >To {savedRequest.recipient}</p
          >{/if}
        <p class="text-xs leading-relaxed text-white/60"
          >{savedRequest.stage === 'prepared'
            ? 'Continue this draft or discard it before starting another transfer.'
            : 'Continue with the original request to recover its result. This does not start another transfer or ask you to fund the deposit again.'}</p
        >
        <div class="flex flex-wrap gap-2">
          <button
            class="rounded-md bg-cyan-300/15 px-3 py-2 text-sm text-cyan-100 disabled:opacity-40"
            disabled={isBridging}
            onclick={continueSaved}
            >{isBridging
              ? 'Checking…'
              : savedRequest.stage === 'accepted'
                ? 'Follow original transfer'
                : 'Continue saved request'}</button
          >
          <button
            class="rounded-md bg-white/5 px-3 py-2 text-sm text-white/60 disabled:opacity-40"
            disabled={isBridging}
            onclick={dismissSaved}
            >{savedRequest.stage === 'accepted'
              ? 'Start another bridge'
              : savedRequest.stage === 'prepared'
                ? 'Discard draft'
                : 'Clear after reviewing outcome'}</button
          >
        </div>
      </section>
    {/if}

    <div class="relative">
      <BridgeHeader
        bridge={selectedBridge}
        tokens={supportTokens}
        {selectedToken}
        {onSelectToken}
        disabled={isLoading || isBridging || !!savedRequest}
      />
    </div>

    <div class="relative grid grid-cols-2 items-center gap-0">
      <!-- From Section -->
      <div class="relative">
        <p class="mb-1 flex items-center gap-2 text-sm text-white/60">
          {#if (innerWidth.current || 0) >= 640}
            <span>From</span>
          {/if}
          <TokenLink bridge={selectedBridge} chain={fromChain?.name} />
        </p>
        <NetworkSelector
          disabled={isLoading || isBridging || !!savedRequest}
          selectedChain={fromChain}
          disabledChainName={''}
          onSelectChain={onSelectFromChain}
          chains={supportChains}
          containerClass="rounded-xl border border-white/40 shrink-0 mr-2 pr-1"
        />
      </div>

      <!-- To Section -->
      <div class="relative">
        <p class="mb-1 ml-2 flex items-center gap-2 text-sm text-white/60">
          {#if (innerWidth.current || 0) >= 640}
            <span>To</span>
          {/if}
          <TokenLink bridge={selectedBridge} chain={toChain?.name} />
        </p>
        <NetworkSelector
          disabled={isLoading || isBridging || !!savedRequest}
          selectedChain={toChain}
          disabledChainName={fromChain?.name ?? ''}
          onSelectChain={onSelectToChain}
          chains={supportChains}
          containerClass="rounded-xl border border-white/40 shrink-0 ml-2 pl-1"
        />
      </div>

      <!-- Swap Button -->
      <div class="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2">
        <p class="collapse mb-1 text-center text-sm">-</p>
        <button
          onclick={onSwapChains}
          disabled={isLoading || isBridging || !!savedRequest}
          title="Swap from and to"
          class="hover:bg-gray flex size-8 items-center justify-center rounded-full border border-white/40 bg-black/90 text-white/50 shadow transition-all duration-500 hover:border-white/60 hover:text-white/90"
        >
          <span class="*:size-5"><ArrowLeftRightLine /></span>
        </button>
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
        disabled={isLoading || isBridging || !!savedRequest}
        oninput={validateSendAmount}
      />
      {#if selectedBridge}
        <div class="mt-1 flex items-center gap-2 text-sm text-white/60">
          <span>Your balance: {selectedBridge.displayAmount(fromBalance)}</span>
          <span class="ml-4"
            >Bridge balance: {selectedBridge.displayAmount(bridgeReserve)}</span
          >
          <span class="ml-4"
            >Bridge fee: {selectedBridge.displayAmount(
              selectedBridge.bridgeFee
            )}
          </span>
        </div>
      {/if}
    </div>

    <div class="relative">
      <p class="mb-1 flex items-center gap-1 text-sm text-white/60">
        <span>To {toChain?.name} address:</span>
        {#if toAddress && !thirdAddress}
          <span>{pruneAddress(toAddress)}</span>
        {/if}
      </p>
      <AddressInput
        bind:value={thirdAddress}
        disabled={isLoading || isBridging || !!savedRequest}
        placeholder={pruneAddress(toAddress) || '0x...'}
        oninput={validateThirdAddress}
      />
      {#if selectedBridge && !error && receivedAmount !== null}
        <div class="mt-1 text-sm text-green-500">
          <span
            >You receive: {selectedBridge.displayAmount(receivedAmount)}</span
          >
        </div>
      {/if}
      {#if thirdAddress}
        <ConfirmAddress
          bind:checked={confirmAddress}
          disabled={isLoading || isBridging || !!savedRequest}
        />
      {/if}
    </div>

    <div class="relative">
      {#if accountError || actionError || error}
        <p role="alert" class="mb-1 text-sm break-words text-red-400"
          >{actionError || accountError || error}</p
        >
      {/if}
      {#if feeWarning}<p class="mb-2 text-xs text-amber-200">{feeWarning}</p
        >{/if}
      {#if fromChain && fromChain.name !== 'ICP'}<p
          class="mb-2 text-xs text-white/40"
          >Gas is an estimate. The bridge checks current provider quotes and fee
          limits before signing.</p
        >{/if}
      {#if bridgingProgress}
        {@const message = bridgingProgress.message}
        {@const info = bridgingProgress.info}
        {#if message}
          <p class="mb-1 text-sm text-green-500"
            >{bridgingProgress.status}: {message}</p
          >
        {/if}
        {#if info && info.fromTxUrl}
          <a
            class="mb-1 flex items-center gap-1 text-sm font-medium text-green-500"
            href={info.fromTxUrl}
            target="_blank"
          >
            <span
              >{'From ' + info.from + ' Tx: ' + pruneAddress(info.fromTx)}</span
            >
            <span class="*:size-4"><ArrowRightUpLine /></span>
          </a>
        {/if}
        {#if info && info.toTx && info.toTxUrl}
          <a
            class="mb-1 flex items-center gap-1 text-sm font-medium text-green-500"
            href={info.toTxUrl}
            target="_blank"
          >
            <span>{'To ' + info.to + ' Tx: ' + pruneAddress(info.toTx)}</span>
            <span class="*:size-4"><ArrowRightUpLine /></span>
          </a>
        {/if}
      {/if}
      {#if !isAuthenticated}
        <PrimaryButton
          onclick={() => {
            isSigningIn = true
            // closing the Internet Identity popup rejects; that is the user's
            // choice, not an error worth reporting
            onSignIn()
              .catch(() => {})
              .finally(() => {
                isSigningIn = false
              })
          }}
          isLoading={isSigningIn}
          ><span class="text-cyan-500">Sign in with Internet Identity</span
          ></PrimaryButton
        >
      {:else if savedRequest}
        <p class="text-center text-sm text-white/50"
          >{bridgingProgress?.isSettled
            ? 'See Activity for the final status or any required review.'
            : 'Use the saved request above to continue. Your activity is also available below.'}</p
        >
      {:else}
        <PrimaryButton
          onclick={onBridge}
          disabled={disabledBridging}
          isLoading={isLoading || isBridging || !selectedBridge}
        >
          {isBridging ? 'Bridging...' : 'Bridge tokens'}
        </PrimaryButton>
      {/if}
    </div>
  {/key}
</div>
