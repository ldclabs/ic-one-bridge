<script lang="ts">
  import { BridgeCanisterAPI } from '$lib/canisters/bridge.svelte'
  import BridgeCard from '$lib/components/BridgeCard.svelte'
  import { BRIDGE_CANISTER_ID } from '$lib/constants'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import GithubFill from '$lib/icons/github-fill.svelte'
  import TwitterXLine from '$lib/icons/twitter-x-line.svelte'
  import { authStore } from '$lib/stores/auth.svelte'
  import { toastRun } from '$lib/stores/toast.svelte'
  import { type BridgeLogInfo } from '$lib/types/bridge'
  import { onMount } from 'svelte'

  const principal = $derived(authStore.identity.getPrincipal().toText())
  const isAuthenticated = $derived(authStore.identity.isAuthenticated)

  let bridge = $state<BridgeCanisterAPI | null>(null)
  let recentLogs: BridgeLogInfo[] = $state([])

  function onSignIn() {
    return authStore.signIn()
  }

  onMount(() => {
    return toastRun(async (_signal) => {
      if (bridge) return
      const _bridge = await BridgeCanisterAPI.loadBridge(BRIDGE_CANISTER_ID)
      // load sub-bridges in background
      _bridge.loadSubBridges()
      bridge = _bridge
    }).abort
  })

  function formatTimeAgo(timestamp: number) {
    const delta = Date.now() - new Date(timestamp).getTime()
    const minutes = Math.max(Math.round(delta / (60 * 1000)), 1)
    if (minutes > 60 * 24 * 36) {
      const days = Math.round(minutes / (60 * 24))
      return `${days} 天前`
    } else if (minutes > 60) {
      const hours = Math.round(minutes / 60)
      return `${hours} 小时前`
    }
    return `${minutes} 分钟前`
  }
</script>

<div class="min-h-screen bg-[var(--color-bg)] pb-24">
  <section
    class="relative isolate overflow-hidden bg-gradient-to-br from-[#10121a] via-[#111520] to-[#050608]"
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
            Computer, Ethereum, BNB Chain, and other EVM-compatible networks
            through a fully on-chain bridge.
          </p>

          <div class="mx-auto flex flex-wrap items-center gap-8 pt-2">
            <a
              class="flex flex-row items-center gap-1 text-white/70 transition hover:text-white"
              href="https://github.com/ldclabs/ic-one-bridge"
              target="_blank"
              rel="noreferrer"
            >
              <span class="*:size-6"><GithubFill /></span>
              <span class="">Source code</span>
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
          <BridgeCard {isAuthenticated} {onSignIn} {bridge} />
        {/key}
      </div>
    </div>
  </section>

  <section
    class="mx-auto w-full max-w-6xl px-6 pt-16 sm:px-10 {principal +
      isAuthenticated}"
  >
    <div class="rounded-3xl border border-white/10 bg-[#0e1119] p-6 sm:p-10">
      <header
        class="flex flex-col gap-2 pb-6 text-white sm:flex-row sm:items-end sm:justify-between"
      >
        <div>
          <h2 class="text-2xl font-semibold">Bridge logs</h2>
        </div>
        <button
          class="h-10 rounded-full border border-white/10 bg-white/5 px-4 text-xs font-semibold text-white/70 transition hover:border-[var(--color-accent)] hover:text-white"
          type="button"
          onclick={() => {}}
        >
          Refresh logs
        </button>
      </header>

      <div class="overflow-hidden rounded-2xl border border-white/5">
        <table
          class="min-w-full divide-y divide-white/5 text-left text-sm text-white/70"
        >
          <thead
            class="bg-white/5 text-xs tracking-[0.2em] text-white/40 uppercase"
          >
            <tr>
              <th class="px-4 py-3">Bridge chain</th>
              <th class="px-4 py-3">Token</th>
              <th class="px-4 py-3">Amount</th>
              <th class="px-4 py-3">Status</th>
              <th class="px-4 py-3">Time</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-white/5">
            {#each recentLogs as log}
              <tr class="bg-white/0">
                <td class="px-4 py-4 text-white">
                  <div class="flex flex-col">
                    <span class="font-semibold">
                      {log.from} → {log.to}
                    </span>
                    <span class="text-xs text-white/40">{log.to_tx || ''}</span>
                  </div>
                </td>
                <td class="px-4 py-4">{log.token}</td>
                <td class="px-4 py-4">
                  <div class="flex flex-col">
                    <span class="font-semibold text-white">{log.amount}</span>
                    <span class="text-xs text-white/40">
                      Fee {log.fee}
                    </span>
                  </div>
                </td>
                <td class="px-4 py-4">
                  <span
                    class="rounded-full border border-[var(--color-accent)]/30 bg-[var(--color-accent)]/10 px-3 py-1 text-xs font-medium text-[var(--color-accent)] uppercase"
                  >
                    {log.status}
                  </span>
                </td>
                <td class="px-4 py-4">{formatTimeAgo(log.finalized_at)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
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
