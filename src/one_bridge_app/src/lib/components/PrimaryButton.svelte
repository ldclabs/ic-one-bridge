<script lang="ts">
  import Spinner from '$lib/ui/Spinner.svelte'

  let { children, disabled = false, isLoading = false, ...props } = $props()
</script>

<button
  {...props}
  disabled={disabled || isLoading}
  class="mx-auto flex items-center justify-center space-x-2 rounded-xl bg-white/10 px-4 py-3 font-bold text-white shadow transition-all duration-300 hover:bg-white/20 disabled:cursor-not-allowed disabled:opacity-50 sm:w-2/3"
>
  {#if isLoading}
    <Spinner class="mr-3 -ml-1 size-5 text-white" />
  {/if}
  <span>{@render children()}</span>
</button>
