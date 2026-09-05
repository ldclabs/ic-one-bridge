<script lang="ts">
  import type { MyAddresses } from '$lib/canisters/bridge.svelte'
  import TextClipboardButton from '$lib/ui/TextClipboardButton.svelte'
  import { pruneAddress } from '$lib/utils/helper'

  const { addresses }: { addresses: MyAddresses | null } = $props()

  const rows: [string, string][] = $derived(
    addresses
      ? [
          ['ICP', addresses.icp],
          ['SOL', addresses.svm],
          ['EVM', addresses.evm]
        ]
      : []
  )
</script>

{#if rows.length > 0}
  <div class="mb-3 flex flex-col gap-1 text-sm text-white/80">
    <p class="mb-1 text-white/60">Your address</p>
    {#each rows as [label, address] (label)}
      <p class="flex items-center gap-1">
        <span>{label}: {pruneAddress(address, true)}</span>
        <TextClipboardButton
          value={address}
          class="text-white/60 *:size-5 hover:text-white/80"
        />
      </p>
    {/each}
  </div>
  <hr class="mb-1 border-white/10" />
{/if}
