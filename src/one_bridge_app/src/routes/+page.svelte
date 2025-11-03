<script lang="ts">
  import { BridgeCanisterAPI } from '$lib/canisters/bridge.svelte'
  import BridgeCard from '$lib/components/BridgeCard.svelte'
  import BridgeLogs from '$lib/components/BridgeLogs.svelte'
  import WalletCard from '$lib/components/WalletCard.svelte'
  import { BRIDGE_CANISTER_ID } from '$lib/constants'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import GithubFill from '$lib/icons/github-fill.svelte'
  import LogoutCircleRLine from '$lib/icons/logout-circle-r-line.svelte'
  import RefreshLine from '$lib/icons/refresh-line.svelte'
  import TwitterXLine from '$lib/icons/twitter-x-line.svelte'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import { type BridgeLogInfo } from '$lib/types/bridge'
  import Spinner from '$lib/ui/Spinner.svelte'
  import { onMount, tick } from 'svelte'

  const principal = $derived(authStore.identity.getPrincipal().toText())
  const isAuthenticated = $derived(authStore.identity.isAuthenticated)

  let mainBridge = $state<BridgeCanisterAPI | null>(null)
  let recentLogs: BridgeLogInfo[] = $state([])
  let myRecentLogs: BridgeLogInfo[] = $state([])
  let isLoading = $state(false)
  let isMyLoading = $state(false)
  let activeTab: 'bridge' | 'wallet' = $state('bridge')

  function onSignIn() {
    return authStore.signIn()
  }

  function fetchRecentLogs() {
    if (!mainBridge || isLoading) return

    isLoading = true
    toastRun(async (_signal) => {
      if (!mainBridge) return

      const bridges = [mainBridge, ...(await mainBridge.loadSubBridges())]

      const logs = await Promise.all(
        bridges.map((b) => b.listFinalizedLogs(20))
      ).then((logs) => logs.flat())
      // 按时间降序排序
      logs.sort((a, b) => Number(b.finalizedAt - a.finalizedAt))
      recentLogs = logs.slice(0, 20)
    }).finally(() => {
      setTimeout(() => {
        isLoading = false
      }, 1000)
    })
  }

  function fetchMyRecentLogs() {
    if (!mainBridge || isMyLoading) return

    isMyLoading = true
    toastRun(async (_signal) => {
      if (!mainBridge) return

      const bridges = [mainBridge, ...(await mainBridge.loadSubBridges())]

      const logs = await Promise.all(
        bridges.map((b) => b.listMyFinalizedLogs(20))
      ).then((logs) => logs.flat())
      // 按时间降序排序
      logs.sort((a, b) => Number(b.finalizedAt - a.finalizedAt))
      myRecentLogs = logs.slice(0, 20)
    }).finally(() => {
      setTimeout(() => {
        isMyLoading = false
      }, 1000)
    })
  }

  $effect(() => {
    if (mainBridge && isAuthenticated) {
      isLoading = false
      tick().then(() => {
        fetchMyRecentLogs()
      })
    } else {
      myRecentLogs = []
    }
  })

  onMount(() => {
    return toastRun(async (_signal) => {
      if (mainBridge) return

      const bridge = await BridgeCanisterAPI.loadBridge(BRIDGE_CANISTER_ID)
      // load sub-bridges in background
      bridge.loadSubBridges()
      mainBridge = bridge
      fetchRecentLogs()
    }).abort
  })
</script>

<div class="min-h-screen bg-(--color-bg) pb-24">
  <section
    class="relative isolate overflow-hidden bg-linear-to-br from-[#10121a] via-[#111520] to-[#050608]"
  >
    <div class="absolute inset-0 -z-10 opacity-80">
      <div
        class="pointer-events-none absolute top-16 -left-10 h-64 w-64 rounded-full bg-[radial-gradient(circle_at_top,var(--color-accent),transparent_70%)] blur-3xl"
      ></div>
      <div
        class="pointer-events-none absolute -right-10 bottom-20 h-72 w-72 rounded-full bg-[radial-gradient(circle_at_bottom,var(--color-purple),transparent_70%)] blur-3xl"
      ></div>
    </div>

    <div
      class="mx-auto flex w-full max-w-6xl flex-col gap-16 px-4 py-10 sm:px-10 sm:py-20"
    >
      <div class="grid items-start gap-16 lg:grid-cols-2">
        <div class="flex flex-col gap-6 text-balance">
          <div
            class="inline-flex items-center gap-2 self-start text-xs text-white/70 backdrop-blur"
          >
            <span
              class="rounded-full border border-white/10 bg-white/5 px-4 py-1"
              >Fully On-Chain</span
            >
            <span
              class="rounded-full border border-white/10 bg-white/5 px-4 py-1"
              >Multi-Chain</span
            >
          </div>

          <h1
            class="text-center text-4xl font-semibold tracking-tight text-white sm:text-5xl"
          >
            <img
              src="/_assets/logo.webp"
              alt="One Bridge Logo"
              class="mx-auto size-64 rounded-full p-2 shadow-2xl shadow-black/50"
            />
            <span>One Bridge</span>
          </h1>
          <p
            class="mx-auto w-full max-w-xl text-center text-base text-white/70 sm:text-lg"
          >
            Enables seamless multi-chain token transfers across the Internet
            Computer, Solana, Ethereum, BNB Chain and other EVM-compatible
            networks through a fully on-chain bridge.
          </p>

          <div class="mx-auto flex flex-wrap items-center gap-8 pt-2">
            <a
              class="flex flex-row items-center gap-1 text-white/70 transition hover:text-white"
              href="https://github.com/ldclabs/ic-one-bridge"
              target="_blank"
              rel="noreferrer"
            >
              <span class="*:size-6"><GithubFill /></span>
              <span class="">Open source</span>
              <span class="*:size-5"><ArrowRightUpLine /></span>
            </a>
            <a
              class="flex flex-row items-center gap-1 text-white/70 transition hover:text-white"
              href="https://x.com/ICPandaDAO"
              target="_blank"
              rel="noreferrer"
            >
              <span class="*:size-6"><TwitterXLine /></span>
              <span class="">Support</span>
              <span class="*:size-5"><ArrowRightUpLine /></span>
            </a>
          </div>
        </div>

        {#key principal + isAuthenticated}
          <div class="relative" data-1p-ignore>
            <div class="mx-6 -mb-px flex items-center justify-between">
              <div class="flex items-center">
                <button
                  class="rounded-t-xl px-6 py-2 transition-colors {activeTab ===
                  'bridge'
                    ? 'cursor-default bg-white/10 text-white'
                    : 'text-white/60 hover:text-white/80'}"
                  onclick={() => (activeTab = 'bridge')}
                  disabled={activeTab === 'bridge'}
                >
                  Bridge
                </button>
                {#if isAuthenticated && mainBridge}
                  <button
                    class="rounded-t-xl px-6 py-2 transition-colors {activeTab ===
                    'wallet'
                      ? 'cursor-default bg-white/10 text-white'
                      : 'text-white/60 hover:text-white/80'}"
                    onclick={() => (activeTab = 'wallet')}
                    disabled={activeTab === 'wallet'}
                  >
                    Wallet
                  </button>
                {/if}
              </div>
              {#if isAuthenticated}
                <button
                  title="Logout"
                  class="-mr-4 flex items-center gap-2 rounded-t-xl p-2 text-white/60 transition-colors hover:text-white/80"
                  onclick={() => {
                    activeTab = 'bridge'
                    authStore.logout()
                  }}
                >
                  <span class="*:size-5"><LogoutCircleRLine /></span>
                  <span class="">Logout</span>
                </button>
              {/if}
            </div>
            <div class="relative">
              <div class:hidden={activeTab !== 'bridge'}>
                <BridgeCard {isAuthenticated} {onSignIn} {mainBridge} />
              </div>
              {#if isAuthenticated && mainBridge}
                <div class:hidden={activeTab !== 'wallet'}>
                  <WalletCard {mainBridge} />
                </div>
              {/if}
            </div>
          </div>
        {/key}
      </div>
    </div>
  </section>

  {#if isAuthenticated}
    <section class="mx-auto w-full max-w-6xl px-6 pt-10 sm:px-10">
      <div class="rounded-xl border border-white/10 bg-[#0e1119] p-6 sm:p-10">
        <header
          class="flex flex-col gap-2 pb-6 text-white sm:flex-row sm:items-end sm:justify-between"
        >
          <div>
            <h2 class="text-2xl font-semibold">My bridge logs</h2>
          </div>
          <button
            class="flex h-10 items-center rounded-xl bg-white/5 px-4 text-xs font-semibold text-white/70 transition hover:bg-white/20 hover:text-white"
            type="button"
            onclick={fetchMyRecentLogs}
            disabled={isLoading || isMyLoading}
          >
            {#if isMyLoading}
              <Spinner class="mr-2 size-5 text-white" />
            {:else}
              <span class="mr-2 *:size-5"><RefreshLine /></span>
            {/if}
            <span>Refresh logs</span>
          </button>
        </header>

        <div
          class="max-h-[800px] overflow-auto rounded-xl border border-white/5"
        >
          <BridgeLogs logs={myRecentLogs} />
        </div>
      </div>
    </section>
  {/if}

  <section class="mx-auto w-full max-w-6xl px-6 pt-10 sm:px-10">
    <div class="rounded-xl border border-white/10 bg-[#0e1119] p-6 sm:p-10">
      <header
        class="flex flex-col gap-2 pb-6 text-white sm:flex-row sm:items-end sm:justify-between"
      >
        <div>
          <h2 class="text-2xl font-semibold">Bridge logs</h2>
        </div>
        <button
          class="flex h-10 items-center rounded-xl bg-white/5 px-4 text-xs font-semibold text-white/70 transition hover:bg-white/20 hover:text-white"
          type="button"
          onclick={fetchRecentLogs}
          disabled={isLoading || isMyLoading}
        >
          {#if isLoading}
            <Spinner class="mr-2 size-5 text-white" />
          {:else}
            <span class="mr-2 *:size-5"><RefreshLine /></span>
          {/if}
          <span>Refresh logs</span>
        </button>
      </header>

      <div class="max-h-[800px] overflow-auto rounded-xl border border-white/5">
        <BridgeLogs logs={recentLogs} />
      </div>
    </div>
  </section>

  <footer id="page-footer" class="text-surface-400 px-4 pt-12 pb-24">
    <div class="flex h-16 flex-col items-center">
      <p class="flex flex-row items-center gap-1">
        <span class="text-sm">© {new Date().getFullYear()}</span>
        <a class="" href="https://panda.fans" target="_blank"
          ><img
            class="w-28"
            src="/_assets/icpanda-dao-white.svg"
            alt="ICPanda DAO"
          /></a
        >
      </p>
      <p class="mt-2 text-center text-sm antialiased">
        Breathing life into sovereign AI.<br />We are building the open-source
        stack for agents to remember, transact, and evolve as first-class
        citizens in Web3.<br />
        <a
          class="underline underline-offset-4"
          href="https://anda.ai"
          target="_blank">Anda.AI</a
        >
        <span class="mx-1">|</span>
        <a class="underline underline-offset-4" href="https://dmsg.net"
          >dMsg.net</a
        >
      </p>
    </div>
  </footer>
</div>
