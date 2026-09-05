<script lang="ts">
  import type { BridgeCanisterAPI } from '$lib/canisters/bridge.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import { pruneCanister } from '$lib/utils/helper'
  import { type TokenInfo } from '$lib/utils/token'
  import TokenSelector from './TokenSelector.svelte'

  const {
    bridge,
    tokens,
    selectedToken,
    onSelectToken,
    disabled = false,
    dimmed = false
  }: {
    bridge: BridgeCanisterAPI | null
    tokens: TokenInfo[]
    selectedToken: TokenInfo | null
    onSelectToken: (token: TokenInfo) => void
    disabled?: boolean
    dimmed?: boolean
  } = $props()
</script>

<div class="flex w-full items-center gap-4" class:opacity-50={dimmed}>
  <TokenSelector {disabled} {selectedToken} {onSelectToken} {tokens} />
  {#if bridge}
    <a
      class="flex items-center gap-1 text-sm font-medium text-white/60"
      title="View the bridge canister"
      href="https://dashboard.internetcomputer.org/canister/{bridge.canisterId.toText()}"
      target="_blank"
    >
      <span>Bridge</span>
      <span>{pruneCanister(bridge.canisterId.toText())}</span>
      <span class="*:size-4"><ArrowRightUpLine /></span>
    </a>
  {/if}
</div>
