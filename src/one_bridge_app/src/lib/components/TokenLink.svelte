<script lang="ts">
  import type { BridgeCanisterAPI } from '$lib/canisters/bridge.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { pruneCanister } from '$lib/utils/helper'

  const {
    bridge,
    chain,
    label = ''
  }: {
    bridge: BridgeCanisterAPI | null
    chain: string | undefined
    label?: string
  } = $props()

  // the token's own page on that chain's explorer, empty when the bridge
  // carries no token there
  const link = $derived(bridge && chain ? bridge.tokenOn(chain) : ['', ''])
</script>

{#if link[0] && link[1]}
  <a
    class="flex items-center gap-1 text-sm font-medium text-white/60"
    title="View {link[0]} info"
    href={link[1]}
    target="_blank"
  >
    {#if label}<span>{label}</span>{/if}
    <span>{pruneCanister(link[0], false)}</span>
    <span class="*:size-4"><ArrowRightUpLine /></span>
  </a>
{/if}
