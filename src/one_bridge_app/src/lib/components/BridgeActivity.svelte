<script lang="ts">
  import type {
    BridgeTx,
    OperationInfo
  } from '$declarations/one_bridge_canister/one_bridge_canister.did.js'
  import {
    BridgingProgress,
    type BridgeCanisterAPI
  } from '$lib/canisters/bridge.svelte'
  import type { BridgeLogInfo } from '$lib/types/bridge'
  import {
    operationActions,
    operationStatus,
    chainName,
    canRecheck
  } from '$lib/utils/bridge-state'
  import { formatTimeAgo } from '$lib/utils/helper'
  import { errMessage } from '$lib/utils/tryrun'
  import { onMount } from 'svelte'
  import BridgeLogs from './BridgeLogs.svelte'

  const {
    bridge,
    mine = false
  }: { bridge: BridgeCanisterAPI; mine?: boolean } = $props()
  const PAGE_SIZE = 20
  let tab = $state<'pending' | 'operations' | 'history'>('pending')
  let cursors = $state<(bigint | undefined)[]>([undefined])
  let logs = $state<BridgeLogInfo[]>([])
  let operations = $state<OperationInfo[]>([])
  let more = $state(false)
  let loading = $state(false)
  let busy = $state(false)
  let error = $state('')
  let notice = $state('')
  let progress = $state<BridgingProgress | null>(null)
  let revision = 0
  let alive = true

  async function load() {
    const run = ++revision
    loading = true
    try {
      const cursor = cursors[cursors.length - 1]
      if (tab === 'operations') {
        const result = bridge.supportsRecovery
          ? await bridge.listOperations(PAGE_SIZE + 1, cursor)
          : []
        if (!alive || run !== revision) return
        operations = result.slice(0, PAGE_SIZE)
        more = result.length > PAGE_SIZE
      } else {
        const result =
          tab === 'pending'
            ? await bridge.listPendingLogs(mine, PAGE_SIZE + 1, cursor)
            : await (mine
                ? bridge.listMyFinalizedLogs(PAGE_SIZE + 1, cursor)
                : bridge.listFinalizedLogs(PAGE_SIZE + 1, cursor))
        if (!alive || run !== revision) return
        logs = result.slice(0, PAGE_SIZE)
        more =
          result.length > PAGE_SIZE &&
          (tab !== 'pending' || bridge.supportsRecovery)
      }
      error = ''
    } catch (err) {
      if (alive && run === revision) error = errMessage(err)
    } finally {
      if (alive && run === revision) loading = false
    }
  }

  function select(next: typeof tab) {
    tab = next
    cursors = [undefined]
    logs = []
    operations = []
    more = false
    load()
  }

  function next() {
    const cursor =
      tab === 'operations'
        ? operations.at(-1)?.id
        : tab === 'pending'
          ? logs.at(-1)?.taskId
          : logs.at(-1)?.id
    if (cursor === undefined) return
    cursors = [...cursors, cursor]
    load()
  }

  async function act(action: () => Promise<void>) {
    if (busy) return
    busy = true
    error = ''
    notice = ''
    try {
      await action()
    } catch (err) {
      if (alive) error = errMessage(err)
    } finally {
      busy = false
      // Preserve an action error while still refreshing the authoritative state.
      const actionError = error
      if (alive) {
        await load()
        if (actionError) error = actionError
      }
    }
  }

  function resume(operation: OperationInfo) {
    act(async () => {
      progress?.stop()
      const next = await bridge.resumeOperation(operation)
      if (!alive) {
        next?.stop()
        return
      }
      progress = next
      notice =
        'The original operation was resumed. No new bridge request was created.'
    })
  }

  function cancel(operation: OperationInfo) {
    if (
      !confirm(
        'Cancel this operation before execution? The canister will refuse if a payment may already have been sent. This does not refund a completed deposit.'
      )
    )
      return
    act(async () => {
      await bridge.cancelOperation(operation)
      if (alive) notice = 'Operation cancelled before execution.'
    })
  }

  function recheck(tx: BridgeTx) {
    act(async () => {
      await bridge.recheckTask(tx)
      if (alive)
        notice =
          'A confirmation check has been scheduled. This does not create another payout.'
    })
  }

  onMount(() => {
    load()
    const refresh = () => {
      if (!busy && !loading && !document.hidden) load()
    }
    const timer = setInterval(refresh, 15_000)
    window.addEventListener('bridge-activity', refresh)
    return () => {
      alive = false
      revision++
      clearInterval(timer)
      window.removeEventListener('bridge-activity', refresh)
      progress?.stop()
    }
  })
</script>

<section class="mx-auto w-full max-w-6xl px-4 pt-10 sm:px-10">
  <div class="rounded-xl border border-white/10 bg-[#0e1119] p-5 sm:p-8">
    <header class="flex flex-wrap items-center justify-between gap-4 pb-5">
      <div>
        <h2 class="text-xl font-semibold text-white"
          >{mine ? 'My activity' : 'Bridge activity'}
          <span class="text-white/40">· {bridge.token?.symbol}</span></h2
        >
        <p class="mt-1 text-xs text-white/50"
          >{mine
            ? 'Transfers and recoverable operations stay available after you reload.'
            : 'Pending transfers and finalized history.'}</p
        >
      </div>
      <button
        class="rounded-lg bg-white/5 px-4 py-2 text-sm text-white/80 hover:bg-white/10 disabled:opacity-40"
        onclick={() => load()}
        disabled={loading || busy}>{loading ? 'Refreshing…' : 'Refresh'}</button
      >
    </header>
    <div class="mb-5 flex flex-wrap gap-2" aria-label="Activity views">
      {#each ['pending', ...(mine ? ['operations'] : []), 'history'] as value}
        <button
          class="rounded-lg px-4 py-2 text-sm {tab === value
            ? 'bg-white/15 text-white'
            : 'bg-white/5 text-white/50 hover:text-white'}"
          aria-pressed={tab === value}
          disabled={busy}
          onclick={() => select(value as typeof tab)}
          >{value === 'pending'
            ? 'In progress'
            : value === 'operations'
              ? 'Operations & recovery'
              : 'History'}</button
        >
      {/each}
    </div>
    {#if error}<p role="alert" class="mb-4 text-sm break-words text-red-300"
        >{error}</p
      >{/if}
    {#if notice}<p role="status" class="mb-4 text-sm text-emerald-300"
        >{notice}</p
      >{/if}
    {#if progress}<p role="status" class="mb-4 text-sm text-white/70"
        >{progress.status}: {progress.message || 'Bridge completed.'}</p
      >{/if}
    {#if tab === 'operations'}
      {#if !bridge.supportsRecovery}
        <p class="py-6 text-sm text-white/50"
          >Operation recovery becomes available after this canister is upgraded.</p
        >
      {:else if !operations.length}
        <p class="py-6 text-sm text-white/50"
          >{loading ? 'Loading operations…' : 'No operations on this page.'}</p
        >
      {/if}
      <div class="max-h-[640px] space-y-3 overflow-auto">
        {#each operations as operation (operation.id)}
          {@const actions = operationActions(operation)}
          {@const deposit = operation.deposit[0]}
          {@const task = operation.related_task[0]}
          <article class="rounded-lg border border-white/10 p-4">
            <div class="flex flex-wrap items-start justify-between gap-3">
              <div>
                <p class="font-medium text-white"
                  >{deposit
                    ? `${chainName(deposit.from)} → ${chainName(deposit.to)}`
                    : operation.kind}
                  <span class="text-xs font-normal text-white/40"
                    >#{String(operation.id)}</span
                  ></p
                >
                {#if deposit}<p class="mt-1 text-sm text-white/70"
                    >{bridge.displayAmount(deposit.amount)}
                    {bridge.token?.symbol} · fee {bridge.displayAmount(
                      deposit.fee
                    )}</p
                  >{/if}
                <p class="mt-1 text-xs text-white/40"
                  >{formatTimeAgo(Number(operation.created_at))} · revision {String(
                    operation.revision
                  )}</p
                >
              </div>
              <span
                class="text-sm {actions.review
                  ? 'text-amber-200'
                  : 'text-cyan-200'}">{operationStatus(operation)}</span
              >
            </div>
            {#if operation.error[0]}<p
                class="mt-3 text-sm break-words text-amber-100/80"
                >{operation.error[0]}</p
              >{/if}
            {#if actions.review}<p class="mt-2 text-sm text-white/60"
                >This payment needs governance reconciliation. It cannot be
                cleared by retrying or cancelling here.</p
              >{/if}
            {#if operation.reconciliation[0]}<details
                class="mt-3 text-xs text-white/50"
                ><summary class="cursor-pointer"
                  >Governance reconciliation evidence</summary
                ><p class="mt-2 break-words"
                  >{operation.reconciliation[0].evidence}</p
                ></details
              >{/if}
            <div class="mt-3 flex flex-wrap gap-2">
              {#if actions.resume}<button
                  disabled={busy}
                  class="rounded-md bg-cyan-300/10 px-3 py-2 text-sm text-cyan-200 disabled:opacity-40"
                  onclick={() => resume(operation)}
                  >{'Completed' in operation.phase ||
                  'Signed' in operation.phase
                    ? 'Follow original transfer'
                    : 'Resume operation'}</button
                >{/if}
              {#if actions.cancel}<button
                  disabled={busy}
                  class="rounded-md bg-white/5 px-3 py-2 text-sm text-white/70 disabled:opacity-40"
                  onclick={() => cancel(operation)}
                  >Cancel before execution</button
                >{/if}
              {#if task && canRecheck(task)}<button
                  disabled={busy}
                  class="rounded-md bg-white/5 px-3 py-2 text-sm text-white/70 disabled:opacity-40"
                  onclick={() => recheck(task.from_tx)}
                  >Check confirmation</button
                >{/if}
            </div>
          </article>
        {/each}
      </div>
    {:else}
      {#if !logs.length}<p class="py-6 text-sm text-white/50"
          >{loading
            ? 'Loading transfers…'
            : tab === 'pending'
              ? 'No pending transfers on this page.'
              : 'No finalized transfers on this page.'}</p
        >{/if}
      <div class="max-h-[640px] overflow-auto rounded-lg border border-white/5"
        ><BridgeLogs
          {logs}
          onRecheck={mine && tab === 'pending'
            ? (log) => recheck(log.fromTransaction)
            : undefined}
          {busy}
        /></div
      >
    {/if}
    <nav
      aria-label="Activity pagination"
      class="mt-5 flex items-center justify-between gap-4 text-sm text-white/60"
    >
      <button
        disabled={loading || busy || cursors.length === 1}
        class="rounded-lg bg-white/5 px-4 py-2 disabled:opacity-30"
        onclick={() => {
          cursors = cursors.slice(0, -1)
          load()
        }}>Previous</button
      >
      <span>Page {cursors.length}</span>
      <button
        disabled={loading || busy || !more}
        class="rounded-lg bg-white/5 px-4 py-2 disabled:opacity-30"
        onclick={next}>Next</button
      >
    </nav>
  </div>
</section>
