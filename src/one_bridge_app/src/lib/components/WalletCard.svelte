<script lang="ts">
  import {
    TransferingProgress,
    type BridgeCanisterAPI,
    type MyAddresses
  } from '$lib/canisters/bridge.svelte'
  import { type Chain } from '$lib/chains'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { formDefault } from '$lib/prefs'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import { pruneAddress } from '$lib/utils/helper'
  import { type TokenInfo } from '$lib/utils/token'
  import { tick, untrack } from 'svelte'
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
    mainBridge
  }: {
    mainBridge: BridgeCanisterAPI
  } = $props()

  const defaultToken = formDefault('Token', 'PANDA')
  const defaultFrom = formDefault('From', 'ICP')

  let myAddresses = $state<MyAddresses | null>(null)
  let bridges = $state<BridgeCanisterAPI[]>([])
  // the effect below keeps it in sync, only the initial value is read here
  let selectedBridge = $state<BridgeCanisterAPI>(untrack(() => mainBridge))
  let supportChains = $state<Chain[]>([])
  let supportTokens = $state<TokenInfo[]>([])
  let bridgeCanister = $derived(selectedBridge.canisterId.toText())
  let selectedToken = $state<TokenInfo | null>(null)
  let fromChain = $state<Chain | null>(null)
  let fromAddress = $state<string>('')
  let fromBalance = $state<bigint>(0n)
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
    return toastRun(async (_signal) => {
      myAddresses = await mainBridge.myAddresses(
        authStore.identity.getPrincipal().toText()
      )
      await refreshMyTokenInfo()
    }).abort
  })

  $effect(() => {
    selectedBridge = mainBridge
    loadBridges(mainBridge).then(() => {
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
      // keep the current selection when it is still supported, this effect also
      // reruns whenever the bridge state is refreshed
      fromChain =
        supportChains.find(
          (c) => c.name === (fromChain?.name || defaultFrom)
        ) ||
        supportChains[0] ||
        null

      await refreshMyTokenInfo()
    }).abort
  })

  async function loadBridges(main: BridgeCanisterAPI) {
    bridges = [main, ...(await main.loadSubBridges())]
    supportTokens = bridges.map((b) => b.token).filter((t) => t !== null)
  }

  function resetTransfer() {
    isTransfering = false
    thirdAddress = ''
    confirmAddress = false
    fromAmount = undefined
    error = null
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
    if (!selectedBridge.token || !fromChain) {
      return [0n, '']
    }

    const value = Math.max(fromAmount || 0, 0)
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
    if (amount < selectedBridge.minAmount) {
      err = `Minimum transfer amount is ${selectedBridge.displayAmount(
        selectedBridge.minAmount
      )}`
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
    await tick()

    if (!selectedBridge || !fromChain || !myAddresses) {
      fromBalance = 0n
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
      // a token transfer on ICP pays the ledger fee in the token itself;
      // everything else pays the chain's own fee
      gasFee =
        fromChain.name === 'ICP' && !nativeToken
          ? selectedBridge.token!.fee
          : account.nativeFee
    } finally {
      isLoading = false
    }
  }

  function onSelectNativeToken() {
    // the gas fee and the amount unit depend on it, refresh before validating again
    error = null
    refreshMyTokenInfo()
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
    const [amount, err] = validateTransfer()
    error = err || null
    if (isTransfering || err || amount <= 0n) return

    isTransfering = true
    toastRun(async () => {
      if (!selectedBridge.state || !selectedBridge.token || !fromChain) {
        return
      }

      const chain = fromChain.name
      try {
        if (chain === 'ICP') {
          const icp = await selectedBridge.loadICPTokenAPI()
          const idx = nativeToken
            ? await icp.transferICP(thirdAddress, amount)
            : await icp.transfer(thirdAddress, amount)
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain,
            native: nativeToken,
            // an ICP ledger transfer either returns a block index or throws
            isFinalized: true,
            Icp: idx
          })
        } else if (chain === 'SOL') {
          const svm = await selectedBridge.loadSvmTokenAPI()
          const signedTx = nativeToken
            ? await selectedBridge.buildSolTransferTx(thirdAddress, amount)
            : await selectedBridge.buildSplTransferTx(thirdAddress, amount)
          const tx = await svm!.sendRawTransaction(signedTx)
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain,
            native: nativeToken,
            isFinalized: false,
            Sol: tx
          })
        } else {
          const evm = await selectedBridge.loadEVMTokenAPI(chain)
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
          const tx = await evm.sendRawTransaction(signedTx)
          transferingProgress = TransferingProgress.track(selectedBridge, {
            chain,
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
    <RefreshButton {isLoading} onclick={() => refreshMyTokenInfo(true)} />
    <AccountAddresses addresses={myAddresses} />

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
