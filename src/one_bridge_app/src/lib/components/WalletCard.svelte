<script lang="ts">
  import { page } from '$app/state'
  import {
    type BridgeCanisterAPI,
    TransferingProgress
  } from '$lib/canisters/bridge.svelte'
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
  import Spinner from '../ui/Spinner.svelte'
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
  let mySolAddress = $state<string>('')
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
  let gasFee = $state<bigint>(0n)
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
    Promise.all([mainBridge.mySvmAddress(), mainBridge.myEvmAddress()]).then(
      ([svmAddr, evmAddr]) => {
        mySolAddress = svmAddr
        myEvmAddress = evmAddr
        refreshMyTokenInfo()
      }
    )
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
      const userBalance = fromBalanceNative - gasFee
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
      fromBalanceIcp - (fromChain.name === 'ICP' ? gasFee : 0n)
    const amount = selectedBridge.parseAmount(fromAmount || 0)
    let err = ''
    if (amount < selectedBridge.tokenDisplay.amount) {
      err = `Minimum transfer amount is ${selectedBridge.tokenDisplay.display()}`
    } else if (amount > userBalance) {
      err = `Insufficient balance, should be less than ${selectedBridge.tokenDisplay.displayValue(
        userBalance
      )}`
    } else if (fromChain.name !== 'ICP' && fromBalanceNative < gasFee) {
      err = `Insufficient ${fromChain.name} balance to cover gas fee`
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

  async function refreshMyTokenInfo(all: boolean = false) {
    await tick()

    if (
      !mainBridge ||
      !selectedBridge ||
      !fromChain ||
      !myIcpAddress ||
      !mySolAddress ||
      !myEvmAddress
    ) {
      fromBalanceIcp = 0n
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
          fromBalanceNative = await icp.getICPBalanceOf(
            authStore.identity.getPrincipal()
          )
          gasFee = nativeToken ? 10000n : selectedBridge.token!.fee
          break
        case 'SOL':
          fromAddress = mySolAddress
          const svm = await selectedBridge.loadSvmTokenAPI()
          const splBalance = await svm!.getSplBalance(mySolAddress)
          fromBalanceIcp = selectedBridge.svmToIcpAmount(splBalance)
          fromBalanceNative = await svm!.getBalance(mySolAddress)
          gasFee = 10000n
          break
        default:
          const evm = await selectedBridge.loadEVMTokenAPI(fromChain.name)
          fromAddress = myEvmAddress
          const v = await evm.getErc20Balance(myEvmAddress)
          fromBalanceIcp = selectedBridge.evmToIcpAmount(fromChain.name, v)
          fromBalanceNative = await evm.getBalance(myEvmAddress)
          gasFee = await evm.gasFeeEstimation()
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
      refreshMyTokenInfo()
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
        } else if (fromChain.name === 'SOL') {
          const svm = await selectedBridge.loadSvmTokenAPI()
          const signedTx = nativeToken
            ? await selectedBridge.buildSolTransferTx(thirdAddress, amount)
            : await selectedBridge.buildSplTransferTx(thirdAddress, amount)
          const tx = await svm!.sendRawTransaction(signedTx)
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain: 'SOL',
            native: nativeToken,
            isFinalized: false,
            Sol: tx
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
        setTimeout(() => {
          refreshMyTokenInfo()
        }, 5000)
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

    <div class="grid grid-cols-[1fr_1fr] items-center justify-center gap-4">
      <!-- From Section -->
      <div class="">
        <p class="mb-1 flex items-center gap-2 text-sm text-white/60">
          <span>Chain</span>
          {#if selectedBridge && fromChain && !nativeToken}
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
                <span>Token</span>
                <span>{pruneCanister(token, false)}</span>
                <span class="*:size-4"><ArrowRightUpLine /></span>
              </a>
            {/if}
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
      <input
        type="number"
        name="tokenAmount"
        disabled={isLoading || isTransfering}
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
        <div class="mt-1 flex items-center gap-2 text-sm text-white/60">
          <span
            >Your balance: {selectedBridge.displayAmount(fromBalanceIcp)}</span
          >
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
        class="mb-1 w-full min-w-0 flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20 disabled:cursor-not-allowed"
      />
      <label
        class="flex items-center text-sm font-medium text-white/60 rtl:text-right"
        ><input
          type="checkbox"
          name="confirmAddress"
          disabled={isLoading || isTransfering}
          bind:checked={confirmAddress}
          class="text-primary-600 me-2 size-4 shrink-0 rounded-sm border-gray-300 bg-gray-100 ring-0 disabled:cursor-not-allowed"
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
