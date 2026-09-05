<script lang="ts">
  import RefreshLine from '$lib/icons/refresh-line.svelte'
  import Spinner from '$lib/ui/Spinner.svelte'

  const {
    isLoading,
    onclick
  }: {
    isLoading: boolean
    onclick: () => void
  } = $props()
</script>

<button
  title="Refresh state"
  class="absolute top-2 right-4 rounded-full bg-white/10 p-1 text-white/60 hover:bg-white/20 hover:text-white/80"
  {onclick}
  disabled={isLoading}
>
  {#if isLoading}
    <Spinner class="size-5 text-white" />
  {:else}
    <span class="*:size-5"><RefreshLine /></span>
  {/if}
</button>
