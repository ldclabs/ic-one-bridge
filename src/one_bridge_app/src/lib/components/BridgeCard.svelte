<script lang="ts">
  import { page } from '$app/state'
  import type { BridgeCanisterAPI } from '$lib/canisters/bridge.svelte'
  import ArrowLeftRightLine from '$lib/icons/arrow-left-right-line.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import type { Chain } from '$lib/types/bridge'
  import { shortAddress, validateAddress } from '$lib/utils/helper'
  import { type TokenInfo } from '$lib/utils/token'
  import { tick } from 'svelte'
  import NetworkSelector from './ChainSelector.svelte'
  import PrimaryButton from './PrimaryButton.svelte'
  import TextClipboardButton from './TextClipboardButton.svelte'
  import TokenInput from './TokenInput.svelte'

  const {
    isAuthenticated,
    onSignIn,
    bridge: mainBridge
  }: {
    isAuthenticated: boolean
    onSignIn: () => Promise<void>
    bridge: BridgeCanisterAPI | null
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
  let bridgeBalanceIcp = $state<bigint>(0n)
  let toAddress = $state<string>('')
  let thirdAddress = $state<string>('')
  let confirmAddress = $state<boolean>(false)
  let fromAmount = $state<number>()
  let error = $state<string | null>(null)
  let isLoading = $state<boolean>(false)
  let isSigningIn = $state<boolean>(false)
  let isBridging = $state<boolean>(false)

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
    if (!selectedBridge) return

    return toastRun(async (_signal) => {
      if (!selectedBridge) return

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

  function validateSendAmount(target: HTMLInputElement) {
    if (
      !selectedBridge ||
      !selectedBridge.token ||
      !selectedBridge.tokenDisplay ||
      !fromChain ||
      !toChain
    ) {
      return
    }

    const userBalance =
      fromBalanceIcp -
      (fromChain.name === 'ICP' ? selectedBridge.token.fee : 0n)
    const amount = selectedBridge.parseAmount(target.value)
    let err = ''
    if (amount < selectedBridge.tokenDisplay.amount) {
      err = `Minimum amount is ${selectedBridge.tokenDisplay.display()}`
    } else if (amount > userBalance) {
      err = `Insufficient balance, should be less than ${selectedBridge.tokenDisplay.displayValue(
        userBalance
      )}`
    } else if (amount >= bridgeBalanceIcp) {
      err = 'Bridge has insufficient balance'
    }

    if (err) {
      target.setCustomValidity(err)
      error = err
    } else {
      target.setCustomValidity('')
      error = null
    }
  }

  function validateThirdAddress(event: Event) {
    const target = event.target as HTMLInputElement
    if (!toChain) return

    let err = ''
    const addr = target.value.trim()
    thirdAddress = addr

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

  async function refreshMyTokenInfo() {
    await tick()

    if (!mainBridge || !selectedBridge || !fromChain || !isAuthenticated) {
      fromAddress = ''
      fromBalanceIcp = 0n
      toAddress = ''
      return
    }

    try {
      isLoading = true
      const icp = await selectedBridge.loadICPTokenAPI()
      switch (fromChain.name) {
        case 'ICP':
          fromAddress = authStore.identity.getPrincipal().toText()
          fromBalanceIcp = await icp.balance()
          break
        default:
          const addr = await mainBridge.myEvmAddress()
          const evm = await selectedBridge.loadEVMTokenAPI(fromChain.name)
          fromAddress = addr
          const v = await evm.getErc20Balance(addr)
          fromBalanceIcp = selectedBridge.toIcpAmount(fromChain.name, v)
      }

      if (toChain) {
        switch (toChain.name) {
          case 'ICP':
            toAddress = authStore.identity.getPrincipal().toText()
            bridgeBalanceIcp = await icp.getBalanceOf(selectedBridge.canisterId)
            break
          default:
            toAddress = await mainBridge.myEvmAddress()
            const evm = await selectedBridge.loadEVMTokenAPI(toChain.name)
            const v = await evm.getErc20Balance(
              selectedBridge.state?.evm_address!
            )
            bridgeBalanceIcp = selectedBridge.toIcpAmount(toChain.name, v)
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

  function onBridge() {}
</script>

<div
  class="rounded-xl border border-white/10 bg-[#131721]/80 p-6 pb-10 text-white/90 shadow-2xl backdrop-blur"
>
  {#key bridgeCanister}
    {#if bridgeCanister}
      <a
        class="inline-flex items-center gap-1 self-start text-xs font-medium text-white/70"
        href="https://dashboard.internetcomputer.org/canister/{bridgeCanister}"
        target="_blank"
      >
        <span>Bridge:</span>
        <span>{bridgeCanister}</span>
        <span class="*:size-4"><ArrowRightUpLine /></span>
      </a>
    {/if}

    <div class="mt-6">
      <p class="mb-1 text-sm text-white/60">
        <span
          >You send (>= {selectedBridge?.tokenDisplay?.display() || '0.0'}) from</span
        >
      </p>

      {#if fromAddress}
        {#key fromAddress}
          <p class="mb-1 flex items-center gap-2 text-sm text-white/60">
            <span
              >{fromChain?.name}
              address: {shortAddress(fromAddress, true)}</span
            >
            <TextClipboardButton value={fromAddress} class="*:size-5" />
          </p>
        {/key}
      {/if}

      <TokenInput
        bind:value={fromAmount}
        disabled={isLoading || isBridging}
        validate={validateSendAmount}
        {selectedToken}
        {onSelectToken}
        tokens={supportTokens}
      />
      <div class="mt-1 text-sm text-white/60">
        <span
          >Your balance: {selectedBridge?.displayAmount(fromBalanceIcp)}</span
        >
        <span class="ml-4"
          >Bridge balance: {selectedBridge?.displayAmount(
            bridgeBalanceIcp
          )}</span
        >
      </div>
    </div>

    <div class="mt-6 grid grid-cols-[1fr_32px_1fr] items-center gap-0">
      <!-- From Section -->
      <div class="">
        <p class="mb-1 text-sm text-white/60">From</p>
        <NetworkSelector
          disabled={isLoading || isBridging}
          selectedChain={fromChain}
          disabledChainName={''}
          onSelectChain={onSelectFromChain}
          chains={supportChains}
          containerClass="rounded-xl border border-white/40 shrink-0"
        />
      </div>

      <!-- Swap Button -->
      <div class="">
        <p class="collapse mb-1 text-center text-sm">-</p>
        <button
          onclick={onSwapChains}
          disabled={isLoading || isBridging}
          title="Swap from and to"
          class="flex size-8 items-center justify-center text-white/50 transition-all duration-500 hover:text-white/90"
        >
          <ArrowLeftRightLine />
        </button>
      </div>

      <!-- To Section -->
      <div class="">
        <p class="mb-1 text-sm text-white/60">To</p>
        <NetworkSelector
          disabled={isLoading || isBridging}
          selectedChain={toChain}
          disabledChainName={fromChain?.name ?? ''}
          onSelectChain={(chain) => (toChain = chain)}
          chains={supportChains}
          containerClass="rounded-xl border border-white/40 shrink-0"
        />
      </div>
    </div>

    <div class="mt-6">
      <p class="mb-1 flex items-center gap-2 text-sm text-white/60">
        <span>To {toChain?.name} address:</span>
        {#if toAddress && !thirdAddress}
          <span>{shortAddress(toAddress, true)}</span>
          <TextClipboardButton value={toAddress} class="*:size-5" />
        {/if}
      </p>
      <input
        type="text"
        name="thirdAddress"
        bind:value={thirdAddress}
        oninput={validateThirdAddress}
        placeholder={shortAddress(toAddress, true) || '0x...'}
        class="mb-1 w-full min-w-0 flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20"
      />
      {#if thirdAddress}
        <label
          class="flex items-center text-sm font-medium text-white/60 rtl:text-right"
          ><input
            type="checkbox"
            name="confirmAddress"
            bind:checked={confirmAddress}
            class="text-primary-600 me-2 size-10 rounded-sm border-gray-300 bg-gray-100 ring-0"
          />
          I confirmed the address is correct and not an exchange or contract address.
          Any tokens sent to an incorrect address will be unrecoverable.</label
        >
      {/if}
    </div>

    <div class="mt-6">
      {#if error}
        <p class="mb-1 text-center text-sm text-red-400">{error}</p>
      {/if}
      {#if isAuthenticated}
        <PrimaryButton
          onclick={onBridge}
          disabled={!!error || (!!thirdAddress && !confirmAddress)}
          isLoading={isLoading || isBridging || !selectedBridge}
        >
          {isBridging ? 'Bridging...' : 'Bridge Tokens'}
        </PrimaryButton>
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
          ><span class="text-blue-500">Sign in with Internet Identity</span
          ></PrimaryButton
        >
      {/if}
    </div>
  {/key}
</div>
