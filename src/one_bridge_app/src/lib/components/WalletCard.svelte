<script lang="ts">
  import { page } from '$app/state'
  import {
    type BridgeCanisterAPI,
    TransferingProgress
  } from '$lib/canisters/bridge.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
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
  import TextClipboardButton from '../ui/TextClipboardButton.svelte'
  import NetworkSelector from './ChainSelector.svelte'
  import PrimaryButton from './PrimaryButton.svelte'
  import TokenSelector from './TokenSelector.svelte'

  const {
    mainBridge
  }: {
    mainBridge: BridgeCanisterAPI
  } = $props()

  const defaultToken =
    page.url.searchParams.get('token') ||
    localStorage.getItem('defaultToken') ||
    'PANDA'
  const defaultFrom =
    page.url.searchParams.get('from') ||
    localStorage.getItem('defaultFrom') ||
    'ICP'

  let myIcpAddress = $state<string>('')
  let myEvmAddress = $state<string>('')
  let bridges = $state<BridgeCanisterAPI[]>([])
  let selectedBridge = $state<BridgeCanisterAPI>(mainBridge)
  let supportChains = $state<Chain[]>([])
  let supportTokens = $state<TokenInfo[]>([])
  let bridgeCanister = $derived(selectedBridge.canisterId.toText())
  let selectedToken = $state<TokenInfo | null>(null)
  let fromChain = $state<Chain | null>(null)
  let fromAddress = $state<string>('')
  let fromBalanceIcp = $state<bigint>(0n)
  let fromBalanceNative = $state<bigint>(0n)
  let nativeToken = $state<boolean>(false)
  let thirdAddress = $state<string>('')
  let confirmAddress = $state<boolean>(false)
  let fromAmount = $state<number>()
  let error = $state<string | null>(null)
  let isLoading = $state<boolean>(false)
  let isTransfering = $state<boolean>(false)
  let transferingProgress = $state<TransferingProgress | null>(null)
  const disabledTransfering = $derived.by(() => {
    return !!(isTransfering || error || !thirdAddress || !confirmAddress)
  })

  $effect(() => {
    myIcpAddress = authStore.identity.getPrincipal().toText()
    mainBridge.myEvmAddress().then((addr) => {
      myEvmAddress = addr
      refreshMyTokenInfo()
    })
  })

  $effect(() => {
    selectedBridge = mainBridge
    mainBridge.loadSubBridges().then((subBridges) => {
      bridges = [mainBridge, ...subBridges]
      supportTokens = bridges.map((b) => b.token!)
      if (selectedBridge.token?.symbol != defaultToken) {
        selectedBridge =
          bridges.find((b) => b.token?.symbol === defaultToken) || mainBridge
      }
    })
  })

  $effect(() => {
    if (!selectedBridge.state) return

    return toastRun(async (_signal) => {
      if (!selectedBridge.state) return

      selectedToken = selectedBridge.token!
      supportChains = await selectedBridge.supportChains()
      await tick()
      fromChain =
        supportChains.find((c) => c.name === defaultFrom) ||
        supportChains[0] ||
        null

      await refreshMyTokenInfo()
    }).abort
  })

  function resetTransfer() {
    isTransfering = false
    thirdAddress = ''
    confirmAddress = false
    fromAmount = undefined
    error = null
    transferingProgress = null

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
      !fromChain
    ) {
      return [0n, '']
    }

    if (nativeToken) {
      const userBalance =
        fromBalanceNative - (fromChain.name === 'ICP' ? 10000n : 0n)
      const amount = selectedBridge.parseNativeAmount(
        fromChain.name,
        fromAmount || 0
      )
      let err = ''
      if (amount > userBalance) {
        const balance = selectedBridge.displayNativeAmount(
          fromChain.name,
          userBalance
        )
        err = `Insufficient ${fromChain.name} balance, should be less than ${balance}`
      }
      return [amount, err]
    }

    const userBalance =
      fromBalanceIcp -
      (fromChain.name === 'ICP' ? selectedBridge.token.fee : 0n)
    const amount = selectedBridge.parseAmount(fromAmount || 0)
    let err = ''
    if (amount < selectedBridge.tokenDisplay.amount) {
      err = `Minimum transfer amount is ${selectedBridge.tokenDisplay.display()}`
    } else if (amount > userBalance) {
      err = `Insufficient balance, should be less than ${selectedBridge.tokenDisplay.displayValue(
        userBalance
      )}`
    }

    return [amount, err]
  }

  function validateThirdAddress(event: Event) {
    const target = event.target as HTMLInputElement
    if (!fromChain) return

    let err = ''
    const addr = thirdAddress.trim()
    if (addr !== thirdAddress) thirdAddress = addr
    if (thirdAddress === fromAddress) {
      err = 'The destination address cannot be the same as the source address'
    } else if (!validateAddress(fromChain.name, addr)) {
      err = `Invalid ${fromChain.name} address format`
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

    if (
      !mainBridge ||
      !selectedBridge ||
      !fromChain ||
      !myIcpAddress ||
      !myEvmAddress
    ) {
      fromBalanceIcp = 0n
      return
    }

    try {
      isLoading = true
      const icp = await selectedBridge.loadICPTokenAPI()
      switch (fromChain.name) {
        case 'ICP':
          fromAddress = myIcpAddress
          fromBalanceIcp = await icp.balance()
          fromBalanceNative = await icp.getICPBalanceOf(
            authStore.identity.getPrincipal()
          )
          break
        default:
          const evm = await selectedBridge.loadEVMTokenAPI(fromChain.name)
          fromAddress = myEvmAddress
          const v = await evm.getErc20Balance(myEvmAddress)
          fromBalanceIcp = selectedBridge.toIcpAmount(fromChain.name, v)
          fromBalanceNative = await evm.getBalance(myEvmAddress)
      }

      isLoading = false
    } catch (err) {
      isLoading = false
      throw err
    }
  }

  function onSelectToken(token: TokenInfo) {
    const bridge = bridges.find((b) => b.token?.name === token.name)
    nativeToken = false
    if (bridge) {
      selectedBridge = bridge
    }
  }

  async function onSelectFromChain(chain: Chain) {
    fromChain = chain

    await refreshMyTokenInfo()
  }

  async function onTransfer() {
    const [amount, err] = _validateSendAmount()
    if (err) {
      error = err
    } else {
      error = null
    }
    if (isTransfering || amount <= 0n) return

    isTransfering = true
    toastRun(async () => {
      if (
        !selectedBridge ||
        !selectedBridge.state ||
        !selectedBridge.token ||
        !selectedBridge.tokenDisplay ||
        !fromChain
      ) {
        return
      }

      try {
        if (fromChain.name === 'ICP') {
          const icp = await selectedBridge.loadICPTokenAPI()
          const idx = nativeToken
            ? await icp.transferICP(thirdAddress, amount)
            : await icp.transfer(thirdAddress, amount)
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain: 'ICP',
            native: nativeToken,
            isFinalized: true,
            Icp: idx
          })
        } else {
          const evm = await selectedBridge.loadEVMTokenAPI(fromChain.name)
          const signedTx = nativeToken
            ? await selectedBridge.buildEvmTransferTx(
                fromChain.name,
                thirdAddress,
                amount
              )
            : await selectedBridge.buildErc20TransferTx(
                fromChain.name,
                thirdAddress,
                amount
              )
          const tx = await evm.sendRawTransaction(signedTx)
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain: fromChain.name,
            native: nativeToken,
            isFinalized: false,
            Evm: tx
          })
        }

        refreshMyTokenInfo()
      } catch (err) {
        isTransfering = false
        throw err
      }
    })
  }
</script>

<div
  class="space-y-6 rounded-xl border border-white/10 bg-[#131721]/80 p-6 pb-10 text-white/90 shadow-2xl backdrop-blur"
>
  {#key bridgeCanister}
    {#if myIcpAddress && myEvmAddress}
      <div class="mb-3 flex flex-col gap-1 text-sm text-white/90">
        <p class="mb-1 text-white/60">Your address</p>
        <p class="flex items-center gap-1">
          {#key myIcpAddress}
            <span>ICP: {pruneAddress(myIcpAddress, true)}</span>
            <TextClipboardButton value={myIcpAddress} class="*:size-5" />
          {/key}
        </p>
        <p class="flex items-center gap-1">
          {#key myEvmAddress}
            <span>EVM: {pruneAddress(myEvmAddress, true)}</span>
            <TextClipboardButton value={myEvmAddress} class="*:size-5" />
          {/key}
        </p>
      </div>
      <hr class="mb-1 border-white/10" />
    {/if}
    <div class="relative">
      <div
        class="flex w-full items-center gap-4"
        class:opacity-50={nativeToken}
      >
        <TokenSelector
          disabled={isLoading || isTransfering}
          {selectedToken}
          {onSelectToken}
          tokens={supportTokens}
        />
        {#if selectedToken}
          <a
            class="flex items-center gap-1 text-sm font-medium text-white/60"
            title="View token ledger canister"
            href="https://dashboard.internetcomputer.org/canister/{selectedToken.canisterId}"
            target="_blank"
          >
            <span>{pruneCanister(selectedToken.canisterId)}</span>
            <span class="*:size-4"><ArrowRightUpLine /></span>
          </a>
        {/if}
      </div>
    </div>

    <div class="grid grid-cols-[1fr_1fr] items-center justify-center gap-4">
      <!-- From Section -->
      <div class="">
        <p class="mb-1 text-sm text-white/60">Chain</p>
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
            class="text-primary-600 me-2 size-4 flex-shrink-0 rounded-sm border-gray-300 bg-gray-100 ring-0"
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
      <input
        type="number"
        name="tokenAmount"
        disabled={isLoading || isTransfering}
        bind:value={fromAmount}
        oninput={validateSendAmount}
        inputmode="decimal"
        step="1.0"
        placeholder="0.0"
        data-1p-ignore
        autocomplete="off"
        class="w-full flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left font-mono text-xl leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20"
      />
      {#if selectedBridge}
        <div class="mt-1 flex items-center gap-4 text-sm text-white/60">
          <span
            >Your balance: {selectedBridge.displayAmount(fromBalanceIcp)}</span
          >
          <span
            >Native {fromChain?.name} balance: {selectedBridge.displayNativeAmount(
              fromChain?.name!,
              fromBalanceNative
            )}</span
          >
        </div>
      {/if}
    </div>

    <div class="relative">
      <p class="mb-1 flex items-center gap-1 text-sm text-white/60">
        <span>To {fromChain?.name} address:</span>
      </p>
      <input
        type="text"
        name="thirdAddress"
        disabled={isLoading || isTransfering}
        bind:value={thirdAddress}
        oninput={validateThirdAddress}
        placeholder={'0x...'}
        data-1p-ignore
        autocomplete="off"
        class="mb-1 w-full min-w-0 flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20"
      />
      <label
        class="flex items-center text-sm font-medium text-white/60 rtl:text-right"
        ><input
          type="checkbox"
          name="confirmAddress"
          disabled={isLoading || isTransfering}
          bind:checked={confirmAddress}
          class="text-primary-600 me-2 size-4 flex-shrink-0 rounded-sm border-gray-300 bg-gray-100 ring-0"
        />
        I confirmed the address is correct and not an exchange or contract address.
        Any tokens sent to an incorrect address will be unrecoverable.</label
      >
    </div>

    <div class="relative">
      {#if error}
        <p class="mb-1 text-sm text-red-400">{error}</p>
      {/if}
      {#if transferingProgress}
        {@const message = transferingProgress.message}
        {@const [tx, txUrl] = [
          transferingProgress.tx,
          transferingProgress.txUrl
        ]}
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
      {/if}

      {#if transferingProgress}
        <PrimaryButton
          onclick={resetTransfer}
          disabled={!transferingProgress.isComplete}
          isLoading={!transferingProgress.isComplete}
        >
          {#if transferingProgress.isComplete}
            <span class="text-green-500">Transfer completed, start again</span>
          {:else}
            <span>Transfering...</span>
          {/if}
        </PrimaryButton>
      {:else}
        <PrimaryButton
          onclick={onTransfer}
          disabled={disabledTransfering}
          isLoading={isLoading || isTransfering || !selectedBridge}
        >
          {isTransfering ? 'Transfering...' : 'Transfer tokens'}
        </PrimaryButton>
      {/if}
    </div>
  {/key}
</div>
