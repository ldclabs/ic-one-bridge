<script lang="ts">
  import RefreshLine from '$lib/icons/refresh-line.svelte'
  import { type BridgeLogInfo } from '$lib/types/bridge'
  import Spinner from '$lib/ui/Spinner.svelte'
  import BridgeLogs from './BridgeLogs.svelte'

  const {
    title,
    logs,
    isLoading,
    disabled = false,
    onRefresh
  }: {
    title: string
    logs: BridgeLogInfo[]
    isLoading: boolean
    disabled?: boolean
    onRefresh: () => void
  } = $props()
</script>

<section class="mx-auto w-full max-w-6xl px-6 pt-10 sm:px-10">
  <div class="rounded-xl border border-white/10 bg-[#0e1119] p-6 sm:p-10">
    <header
      class="flex flex-col gap-2 pb-6 text-white sm:flex-row sm:items-end sm:justify-between"
    >
      <h2 class="text-2xl font-semibold">{title}</h2>
      <button
        class="flex h-10 items-center rounded-xl bg-white/5 px-4 text-xs font-semibold text-white/70 transition hover:bg-white/20 hover:text-white"
        type="button"
        onclick={onRefresh}
        {disabled}
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
      <BridgeLogs {logs} />
    </div>
  </div>
</section>
