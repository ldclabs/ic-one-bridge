<script lang="ts">
  import { page } from '$app/state'
  import {
    type BridgeCanisterAPI,
    BridgingProgress
  } from '$lib/canisters/bridge.svelte'
  import ArrowLeftRightLine from '$lib/icons/arrow-left-right-line.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import RefreshLine from '$lib/icons/refresh-line.svelte'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import type { Chain } from '$lib/types/bridge'
  import {
    pruneAddress,
    pruneCanister,
    validateAddress
  } from '$lib/utils/helper'
  import { type TokenInfo } from '$lib/utils/token'
  import { tick } from 'svelte'
  import { innerWidth } from 'svelte/reactivity/window'
  import Spinner from '../ui/Spinner.svelte'
  import TextClipboardButton from '../ui/TextClipboardButton.svelte'
  import NetworkSelector from './ChainSelector.svelte'
  import PrimaryButton from './PrimaryButton.svelte'
  import TokenSelector from './TokenSelector.svelte'

  const {
    isAuthenticated,
    onSignIn,
    mainBridge
  }: {
    isAuthenticated: boolean
    onSignIn: () => Promise<void>
    mainBridge: BridgeCanisterAPI | null
  } = $props()

  const defaultToken =
    page.url.searchParams.get('token') ||
    localStorage.getItem('defaultToken') ||
    'PANDA'
  const defaultFrom =
    page.url.searchParams.get('from') ||
    localStorage.getItem('defaultFrom') ||
    'ICP'
  const defaultTo =
    page.url.searchParams.get('to') ||
    localStorage.getItem('defaultTo') ||
    'BNB'

  let myIcpAddress = $state<string>('')
  let mySolAddress = $state<string>('')
  let myEvmAddress = $state<string>('')
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
  let fromBalanceIcp = $state<bigint>(0n)
  let fromBalanceNative = $state<bigint>(0n)
  let bridgeBalanceIcp = $state<bigint>(0n)
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
    if (!mainBridge) return

    if (isAuthenticated) {
      myIcpAddress = authStore.identity.getPrincipal().toText()
      Promise.all([mainBridge.mySvmAddress(), mainBridge.myEvmAddress()]).then(
        ([svmAddr, evmAddr]) => {
          mySolAddress = svmAddr
          myEvmAddress = evmAddr
          refreshMyTokenInfo()
        }
      )
    } else {
      myIcpAddress = ''
      mySolAddress = ''
      myEvmAddress = ''
    }
  })

  $effect(() => {
    if (!mainBridge) return

    selectedBridge = mainBridge
    mainBridge.loadSubBridges().then((subBridges) => {
      bridges = [mainBridge, ...subBridges]
      supportTokens = bridges.map((b) => b.token!)
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

      if (selectedBridge.state?.error_rounds >= 42n) {
        bridgeError =
          'the bridge is temporarily disabled due to errors, please contact the administrator'
      } else {
        bridgeError = null
      }

      selectedToken = selectedBridge.token!
      supportChains = await selectedBridge.supportChains()
      await tick()
      fromChain =
        supportChains.find((c) => c.name === defaultFrom) ||
        supportChains[0] ||
        null
      if (defaultFrom != defaultTo) {
        toChain =
          supportChains.find((c) => c.name === defaultTo) ||
          supportChains[1] ||
          null
      }

      await refreshMyTokenInfo()
    }).abort
  })

  function resetBridge() {
    isBridging = false
    thirdAddress = ''
    confirmAddress = false
    fromAmount = undefined
    error = null
    bridgingProgress = null
    refreshMyTokenInfo()
  }

  function validateSendAmount(event: Event) {
    const target = event.target as HTMLInputElement
    const [_, err] = _validateSendAmount()
    if (err) {
      target.setCustomValidity(err)
      error = err
    } else {
      target.setCustomValidity('')
      error = null
    }
  }

  function _validateSendAmount(): [bigint, string] {
    if (
      !selectedBridge ||
      !selectedBridge.token ||
      !selectedBridge.tokenDisplay ||
      !fromChain ||
      !toChain
    ) {
      return [0n, '']
    }

    const userBalance =
      fromBalanceIcp - (fromChain.name === 'ICP' ? gasFee : 0n)
    const amount = selectedBridge.parseAmount(fromAmount || 0)
    let err = ''
    if (amount < selectedBridge.tokenDisplay.amount) {
      err = `Minimum bridge amount is ${selectedBridge.tokenDisplay.display()}`
    } else if (amount > userBalance) {
      err = `Insufficient balance, should be less than ${selectedBridge.tokenDisplay.displayValue(
        userBalance
      )}`
    } else if (amount >= bridgeBalanceIcp) {
      err = 'Bridge has insufficient balance'
    } else if (fromChain.name !== 'ICP' && fromBalanceNative < gasFee) {
      err = `Insufficient ${fromChain.name} balance to cover gas fee`
    }

    return [amount, err]
  }

  function validateThirdAddress(event: Event) {
    const target = event.target as HTMLInputElement
    if (!toChain) return

    let err = ''
    const addr = thirdAddress.trim()
    if (addr !== thirdAddress) thirdAddress = addr
    if (addr) {
      if (!validateAddress(toChain.name, addr)) {
        err = `Invalid ${toChain.name} address format`
      }
    }

    if (err) {
      target.setCustomValidity(err)
      error = err
    } else {
      target.setCustomValidity('')
      error = null
    }
  }

  async function refreshMyTokenInfo(all: boolean = false) {
    await tick()

    if (
      !mainBridge ||
      !selectedBridge ||
      !fromChain ||
      !isAuthenticated ||
      !myIcpAddress ||
      !mySolAddress ||
      !myEvmAddress
    ) {
      fromAddress = ''
      fromBalanceIcp = 0n
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

        const subBridges = await mainBridge.loadSubBridges()
        bridges = [mainBridge, ...subBridges]
        supportTokens = bridges.map((b) => b.token!)
      }

      const icp = await selectedBridge.loadICPTokenAPI()
      switch (fromChain.name) {
        case 'ICP':
          fromAddress = myIcpAddress
          fromBalanceIcp = await icp.balance()
          gasFee = selectedBridge.token!.fee
          break
        case 'SOL':
          fromAddress = mySolAddress
          const svm = await selectedBridge.loadSvmTokenAPI()
          const splBalance = await svm!.getSplBalance(mySolAddress)
          fromBalanceIcp = selectedBridge.svmToIcpAmount(splBalance)
          fromBalanceNative = splBalance
          gasFee = 0n
          break
        default:
          const evm = await selectedBridge.loadEVMTokenAPI(fromChain.name)
          fromAddress = myEvmAddress
          const v = await evm.getErc20Balance(myEvmAddress)
          fromBalanceIcp = selectedBridge.evmToIcpAmount(fromChain.name, v)
          fromBalanceNative = await evm.getBalance(myEvmAddress)
          gasFee = await evm.gasFeeEstimation()
      }

      if (toChain) {
        switch (toChain.name) {
          case 'ICP':
            toAddress = myIcpAddress
            bridgeBalanceIcp = await icp.getBalanceOf(selectedBridge.canisterId)
            break
          case 'SOL':
            toAddress = mySolAddress
            const svm = await selectedBridge.loadSvmTokenAPI()
            const splBalance = await svm!.getSplBalance(
              selectedBridge.state?.svm_address!
            )
            bridgeBalanceIcp = selectedBridge.svmToIcpAmount(splBalance)
            break
          default:
            toAddress = myEvmAddress
            const evm = await selectedBridge.loadEVMTokenAPI(toChain.name)
            const v = await evm.getErc20Balance(
              selectedBridge.state?.evm_address!
            )
            bridgeBalanceIcp = selectedBridge.evmToIcpAmount(toChain.name, v)
        }
      }
      isLoading = false
    } catch (err) {
      isLoading = false
      throw err
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
    const temp = fromChain
    fromChain = toChain
    toChain = temp
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
    const [amount, err] = _validateSendAmount()
    if (err) {
      error = err
    } else {
      error = null
    }
    if (isBridging || amount <= 0n) return

    isBridging = true
    toastRun(async () => {
      if (
        !selectedBridge ||
        !selectedBridge.state ||
        !selectedBridge.token ||
        !selectedBridge.tokenDisplay ||
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
            amount +
              selectedBridge.token.fee +
              selectedBridge.state.token_bridge_fee
          )
        }

        bridgingProgress = await selectedBridge.bridge(
          fromChain.name,
          toChain.name,
          amount,
          thirdAddress
        )

        localStorage.setItem('defaultToken', selectedBridge.token.symbol)
        localStorage.setItem('defaultFrom', fromChain.name)
        localStorage.setItem('defaultTo', toChain.name)

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
    <button
      title="Refresh state"
      class="absolute top-2 right-4 rounded-full bg-white/10 p-1 text-white/60 hover:bg-white/20 hover:text-white/80"
      onclick={() => refreshMyTokenInfo(true)}
      disabled={isLoading}
    >
      {#if isLoading}
        <Spinner class="size-5 text-white" />
      {:else}
        <span class=" *:size-5"><RefreshLine /></span>
      {/if}
    </button>
    {#if myIcpAddress && myEvmAddress}
      <div class="mb-3 flex flex-col gap-1 text-sm text-white/80">
        <p class="mb-1 text-white/60">Your address</p>
        <p class="flex items-center gap-1">
          {#key myIcpAddress}
            <span>ICP: {pruneAddress(myIcpAddress, true)}</span>
            <TextClipboardButton
              value={myIcpAddress}
              class="text-white/60 *:size-5 hover:text-white/80"
            />
          {/key}
        </p>
        <p class="flex items-center gap-1">
          {#key mySolAddress}
            <span>SOL: {pruneAddress(mySolAddress, true)}</span>
            <TextClipboardButton
              value={mySolAddress}
              class="text-white/60 *:size-5 hover:text-white/80"
            />
          {/key}
        </p>
        <p class="flex items-center gap-1">
          {#key myEvmAddress}
            <span>EVM: {pruneAddress(myEvmAddress, true)}</span>
            <TextClipboardButton
              value={myEvmAddress}
              class="text-white/60 *:size-5 hover:text-white/80"
            />
          {/key}
        </p>
      </div>
      <hr class="mb-1 border-white/10" />
    {/if}
    <div class="relative">
      <div class="flex w-full items-center gap-4">
        <TokenSelector
          disabled={isLoading || isBridging}
          {selectedToken}
          {onSelectToken}
          tokens={supportTokens}
        />
        {#if selectedBridge}
          <a
            class="flex items-center gap-1 text-sm font-medium text-white/60"
            title="View token ledger canister"
            href="https://dashboard.internetcomputer.org/canister/{selectedBridge.canisterId.toText()}"
            target="_blank"
          >
            <span>Bridge</span>
            <span>{pruneCanister(selectedBridge.canisterId.toText())}</span>
            <span class="*:size-4"><ArrowRightUpLine /></span>
          </a>
        {/if}
      </div>
    </div>

    <div class="relative grid grid-cols-2 items-center gap-0">
      <!-- From Section -->
      <div class="relative">
        <p class="mb-1 flex items-center gap-2 text-sm text-white/60">
          {#if (innerWidth.current || 0) >= 640}
            <span>From</span>
          {/if}
          {#if selectedBridge && fromChain}
            {@const [token, tokenUrl] = selectedBridge.getTokenUrl(
              fromChain.name
            )}
            {#if token && tokenUrl}
              <a
                class="flex items-center gap-1 text-sm font-medium text-white/60"
                title="View {token} info"
                href={tokenUrl}
                target="_blank"
              >
                <span>{pruneCanister(token, false)}</span>
                <span class="*:size-4"><ArrowRightUpLine /></span>
              </a>
            {/if}
          {/if}
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
          {#if selectedBridge && toChain}
            {@const [token, tokenUrl] = selectedBridge.getTokenUrl(
              toChain.name
            )}
            {#if token && tokenUrl}
              <a
                class="flex items-center gap-1 text-sm font-medium text-white/60"
                title="View {token} info"
                href={tokenUrl}
                target="_blank"
              >
                <span>{pruneCanister(token, false)}</span>
                <span class="*:size-4"><ArrowRightUpLine /></span>
              </a>
            {/if}
          {/if}</p
        >
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
      <input
        type="number"
        name="tokenAmount"
        disabled={isLoading || isBridging}
        bind:value={fromAmount}
        oninput={validateSendAmount}
        inputmode="decimal"
        placeholder="0.0"
        step="any"
        data-1p-ignore
        autocomplete="off"
        class="w-full flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left font-mono text-xl leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20 disabled:cursor-not-allowed"
      />
      {#if selectedBridge}
        {@const token_bridge_fee = selectedBridge.state?.token_bridge_fee || 0n}
        <div class="mt-1 flex items-center gap-2 text-sm text-white/60">
          <span
            >Your balance: {selectedBridge.displayAmount(fromBalanceIcp)}</span
          >
          <span class="ml-4"
            >Bridge balance: {selectedBridge.displayAmount(
              bridgeBalanceIcp
            )}</span
          >
          {#if token_bridge_fee >= 0n}
            <span class="ml-4"
              >Bridge fee: {selectedBridge.displayAmount(token_bridge_fee)}
            </span>
          {/if}
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
      <input
        type="text"
        name="thirdAddress"
        disabled={isLoading || isBridging}
        bind:value={thirdAddress}
        oninput={validateThirdAddress}
        placeholder={pruneAddress(toAddress) || '0x...'}
        data-1p-ignore
        autocomplete="off"
        class="mb-1 w-full min-w-0 flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20 disabled:cursor-not-allowed"
      />
      {#if selectedBridge && !error && fromAmount! > 0}
        {@const token_bridge_fee = selectedBridge.state?.token_bridge_fee || 0n}
        {@const amount = selectedBridge.parseAmount(fromAmount!)}
        <div class="mt-1 text-sm text-green-500">
          <span
            >You receive: {selectedBridge.displayAmount(
              amount - token_bridge_fee
            )}</span
          >
        </div>
      {/if}
      {#if thirdAddress}
        <label
          class="flex items-center text-sm font-medium text-white/60 rtl:text-right"
          ><input
            type="checkbox"
            name="confirmAddress"
            disabled={isLoading || isBridging}
            bind:checked={confirmAddress}
            class="text-primary-600 me-2 size-4 shrink-0 rounded-sm border-gray-300 bg-gray-100 ring-0 disabled:cursor-not-allowed"
          />
          I confirmed the address is correct and not an exchange or contract address.
          Any tokens sent to an incorrect address will be unrecoverable.</label
        >
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
      {#if isAuthenticated}
        {#if bridgingProgress}
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
      {:else}
        <PrimaryButton
          onclick={() => {
            isSigningIn = true
            onSignIn()
              .then(() => {
                isSigningIn = false
              })
              .catch(() => {
                isSigningIn = false
              })
          }}
          isLoading={isSigningIn}
          ><span class="text-cyan-500">Sign in with Internet Identity</span
          ></PrimaryButton
        >
      {/if}
    </div>
  {/key}
</div>
