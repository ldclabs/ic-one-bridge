<script lang="ts">
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { type BridgeLogInfo } from '$lib/types/bridge'
  import { formatTimeAgo, pruneAddress } from '$lib/utils/helper'

  let {
    logs
  }: {
    logs: BridgeLogInfo[]
  } = $props()
</script>

<table
  class="min-w-full divide-y divide-white/5 text-left text-sm text-white/70"
>
  <thead class="bg-white/5 text-xs tracking-[0.2em] text-white/40 uppercase">
    <tr>
      <th class="px-4 py-3">Bridge chain</th>
      <th class="px-4 py-3">Token</th>
      <th class="px-4 py-3">Amount</th>
      <th class="px-4 py-3">Status</th>
      <th class="px-4 py-3">Finalized</th>
    </tr>
  </thead>
  <tbody class="divide-y divide-white/5">
    {#each logs as log}
      <tr class="bg-white/0">
        <td class="px-4 py-4 text-white">
          <div class="flex flex-col">
            <span class="font-semibold">
              {log.from} → {log.to}
            </span>
            {#if log.toTx && log.toTxUrl}
              <a
                class="flex items-center gap-1 text-xs text-white/40"
                href={log.toTxUrl}
                target="_blank"
              >
                <span>{'To ' + log.to + ' Tx: ' + pruneAddress(log.toTx)}</span>
                <span class="*:size-4"><ArrowRightUpLine /></span>
              </a>
            {:else if log.fromTx && log.fromTxUrl}
              <a
                class="flex items-center gap-1 text-xs text-white/60"
                href={log.fromTxUrl}
                target="_blank"
              >
                <span
                  >{'From ' +
                    log.from +
                    ' Tx: ' +
                    pruneAddress(log.fromTx)}</span
                >
                <span class="*:size-4"><ArrowRightUpLine /></span>
              </a>
            {/if}
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
        <td class="px-4 py-4">{formatTimeAgo(log.finalizedAt)}</td>
      </tr>
    {/each}
  </tbody>
</table>
