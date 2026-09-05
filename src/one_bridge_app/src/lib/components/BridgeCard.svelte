<script lang="ts">
  import {
    addressOn,
    BridgingProgress,
    type BridgeCanisterAPI,
    type MyAddresses
  } from '$lib/canisters/bridge.svelte'
  import { type Chain } from '$lib/chains'
  import ArrowLeftRightLine from '$lib/icons/arrow-left-right-line.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { formDefault, rememberForm } from '$lib/prefs'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import { pruneAddress } from '$lib/utils/helper'
  import { type TokenInfo } from '$lib/utils/token'
  import { tick } from 'svelte'
  import { innerWidth } from 'svelte/reactivity/window'
  import AccountAddresses from './AccountAddresses.svelte'
  import AddressInput from './AddressInput.svelte'
  import AmountInput from './AmountInput.svelte'
  import BridgeHeader from './BridgeHeader.svelte'
  import NetworkSelector from './ChainSelector.svelte'
  import ConfirmAddress from './ConfirmAddress.svelte'
  import PrimaryButton from './PrimaryButton.svelte'
  import RefreshButton from './RefreshButton.svelte'
  import TokenLink from './TokenLink.svelte'

  const {
    isAuthenticated,
    onSignIn,
    mainBridge
  }: {
    isAuthenticated: boolean
    onSignIn: () => Promise<void>
    mainBridge: BridgeCanisterAPI | null
  } = $props()

  const defaultToken = formDefault('Token', 'PANDA')
  const defaultFrom = formDefault('From', 'ICP')
  const defaultTo = formDefault('To', 'BNB')

  let myAddresses = $state<MyAddresses | null>(null)
  let bridges = $state<BridgeCanisterAPI[]>([])
  let selectedBridge = $state<BridgeCanisterAPI | null>(null)
  let supportChains = $state<Chain[]>([])
  let supportTokens = $state<TokenInfo[]>([])
  let bridgeCanister = $derived(
    selectedBridge ? selectedBridge.canisterId.toText() : ''
  )
  let selectedToken = $state<TokenInfo | null>(null)
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
  let fromAmount = $state<number>()
  let error = $state<string | null>(null)
  let bridgeError = $state<string | null>(null)
  let isLoading = $state<boolean>(false)
  let isSigningIn = $state<boolean>(false)
  let isBridging = $state<boolean>(false)
  let bridgingProgress = $state<BridgingProgress | null>(null)
  const disabledBridging = $derived.by(() => {
    return !!(
      isBridging ||
      bridgeError ||
      error ||
      (thirdAddress && !confirmAddress)
    )
  })

  $effect(() => {
    const bridge = mainBridge
    if (!bridge || !isAuthenticated) {
      myAddresses = null
      return
    }

    return toastRun(async (_signal) => {
      myAddresses = await bridge.myAddresses(
        authStore.identity.getPrincipal().toText()
      )
      await refreshMyTokenInfo()
    }).abort
  })

  $effect(() => {
    if (!mainBridge) return

    selectedBridge = mainBridge
    loadBridges(mainBridge).then(() => {
      if (selectedBridge?.token?.symbol != defaultToken) {
        selectedBridge =
          bridges.find((b) => b.token?.symbol === defaultToken) || mainBridge
      }
    })
  })

  $effect(() => {
    if (!selectedBridge || !selectedBridge.state) return

    return toastRun(async (_signal) => {
      if (!selectedBridge || !selectedBridge.state) return

      bridgeError = selectedBridge.pausedReason

      selectedToken = selectedBridge.token!
      supportChains = await selectedBridge.supportChains()
      await tick()
      // keep the current selection when it is still supported, this effect also
      // reruns whenever the bridge state is refreshed
      const fromName = fromChain?.name || defaultFrom
      const toName = toChain?.name || defaultTo
      fromChain =
        supportChains.find((c) => c.name === fromName) ||
        supportChains[0] ||
        null
      if (fromChain?.name !== toName) {
        toChain =
          supportChains.find((c) => c.name === toName) ||
          supportChains.find((c) => c.name !== fromChain?.name) ||
          null
      }

      await refreshMyTokenInfo()
    }).abort
  })

  async function loadBridges(main: BridgeCanisterAPI) {
    bridges = [main, ...(await main.loadSubBridges())]
    supportTokens = bridges.map((b) => b.token).filter((t) => t !== null)
  }

  function resetBridge() {
    isBridging = false
    thirdAddress = ''
    confirmAddress = false
    fromAmount = undefined
    error = null
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
    if (!selectedBridge?.token || !fromChain || !toChain) {
      return [0n, '']
    }

    // ICP charges the ledger fee in the token, so it is not spendable
    const spendable = fromBalance - (fromChain.name === 'ICP' ? gasFee : 0n)
    const amount = selectedBridge.parseAmount(Math.max(fromAmount || 0, 0))
    let err = ''
    if (amount < selectedBridge.minAmount) {
      err = `Minimum bridge amount is ${selectedBridge.displayAmount(
        selectedBridge.minAmount
      )}`
    } else if (amount > spendable) {
      err = `Insufficient balance, should be less than ${selectedBridge.displayAmount(
        spendable
      )}`
    } else if (amount >= bridgeReserve) {
      err = 'Bridge has insufficient balance'
    } else if (fromChain.name !== 'ICP' && fromBalanceNative < gasFee) {
      err = `Insufficient ${fromChain.name} balance to cover gas fee`
    }

    return [amount, err]
  }

  async function refreshMyTokenInfo(all: boolean = false) {
    await tick()

    if (
      !mainBridge ||
      !selectedBridge ||
      !fromChain ||
      !isAuthenticated ||
      !myAddresses
    ) {
      fromAddress = ''
      fromBalance = 0n
      toAddress = ''
      return
    }

    try {
      isLoading = true

      if (all) {
        await mainBridge.refreshState()
        if (selectedBridge !== mainBridge) {
          await selectedBridge.refreshState()
        }
        await loadBridges(mainBridge)
      }

      const account = await selectedBridge.myAccountOn(
        fromChain.name,
        myAddresses
      )
      fromAddress = account.address
      fromBalance = account.tokenBalance
      fromBalanceNative = account.nativeBalance
      // on ICP the ledger takes its fee in the token itself; every other chain
      // pays gas out of its native coin
      gasFee =
        fromChain.name === 'ICP' ? selectedBridge.token!.fee : account.nativeFee

      if (toChain) {
        toAddress = addressOn(toChain.name, myAddresses)
        bridgeReserve = await selectedBridge.reserveOn(toChain.name)
      }
    } finally {
      isLoading = false
    }
  }

  function onSelectToken(token: TokenInfo) {
    const bridge = bridges.find((b) => b.token?.name === token.name)
    if (bridge) {
      selectedBridge = bridge
    }
    refreshMyTokenInfo()
  }

  async function onSwapChains() {
    ;[fromChain, toChain] = [toChain, fromChain]
    await refreshMyTokenInfo()
  }

  async function onSelectFromChain(chain: Chain) {
    fromChain = chain
    if (toChain?.name === chain.name) {
      toChain = supportChains.find((c) => c.name !== chain.name) || null
    }
    await refreshMyTokenInfo()
  }

  async function onSelectToChain(chain: Chain) {
    toChain = chain
    if (fromChain?.name === chain.name) {
      fromChain = supportChains.find((c) => c.name !== chain.name) || null
    }
    await refreshMyTokenInfo()
  }

  async function onBridge() {
    const [amount, err] = validateBridge()
    error = err || null
    if (isBridging || err || amount <= 0n) return

    isBridging = true
    toastRun(async () => {
      if (
        !selectedBridge?.state ||
        !selectedBridge.token ||
        !fromChain ||
        !toChain
      ) {
        return
      }

      try {
        if (fromChain.name === 'ICP') {
          const icp = await selectedBridge.loadICPTokenAPI()
          await icp.ensureAllowance(
            selectedBridge.canisterId,
            amount + selectedBridge.token.fee + selectedBridge.bridgeFee
          )
        }

        bridgingProgress = await selectedBridge.bridge(
          fromChain.name,
          toChain.name,
          amount,
          thirdAddress
        )

        rememberForm(selectedBridge.token.symbol, fromChain.name, toChain.name)

        refreshMyTokenInfo()
        setTimeout(() => {
          refreshMyTokenInfo()
        }, 5000)
      } catch (err) {
        isBridging = false
        throw err
      }
    })
  }
</script>

<div
  class="space-y-6 rounded-xl border border-white/10 bg-[#131721]/80 p-6 pb-10 text-white/90 shadow-2xl backdrop-blur"
>
  {#key bridgeCanister}
    <RefreshButton {isLoading} onclick={() => refreshMyTokenInfo(true)} />
    <AccountAddresses addresses={myAddresses} />

    <div class="relative">
      <BridgeHeader
        bridge={selectedBridge}
        tokens={supportTokens}
        {selectedToken}
        {onSelectToken}
        disabled={isLoading || isBridging}
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
          disabled={isLoading || isBridging}
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
          disabled={isLoading || isBridging}
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
          disabled={isLoading || isBridging}
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
        disabled={isLoading || isBridging}
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
        disabled={isLoading || isBridging}
        placeholder={pruneAddress(toAddress) || '0x...'}
        oninput={validateThirdAddress}
      />
      {#if selectedBridge && !error && fromAmount! > 0}
        {@const received =
          selectedBridge.parseAmount(fromAmount!) - selectedBridge.bridgeFee}
        <div class="mt-1 text-sm text-green-500">
          <span>You receive: {selectedBridge.displayAmount(received)}</span>
        </div>
      {/if}
      {#if thirdAddress}
        <ConfirmAddress
          bind:checked={confirmAddress}
          disabled={isLoading || isBridging}
        />
      {/if}
    </div>

    <div class="relative">
      {#if bridgeError || error}
        <p class="mb-1 text-sm text-red-400">{bridgeError || error}</p>
      {/if}
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
      {:else if bridgingProgress}
        {@const isComplete = bridgingProgress.isComplete}
        <PrimaryButton
          onclick={resetBridge}
          disabled={!isComplete}
          isLoading={!isComplete}
        >
          {#if isComplete}
            <span class="text-green-500">Bridge completed, start again</span>
          {:else}
            <span>Bridging...</span>
          {/if}
        </PrimaryButton>
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
