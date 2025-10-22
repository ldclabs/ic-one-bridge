<script lang="ts">
  import ArrowDownSLine from '$lib/icons/arrow-down-s-line.svelte'
  import type { Chain } from '$lib/types/bridge'
  import Dropdown from '$lib/ui/Dropdown.svelte'

  let {
    disabled = false,
    selectedChain,
    onSelectChain,
    chains,
    disabledChainName,
    containerClass = ''
  }: {
    disabled?: boolean
    selectedChain: Chain | null
    onSelectChain: (chain: Chain) => void
    chains: Chain[]
    disabledChainName: string
    containerClass?: string
  } = $props()

  let open = $state(false)

  const handleSelect = (chain: Chain) => {
    onSelectChain(chain)
    open = false
  }
</script>

{#snippet trigger()}
  <div class="flex min-w-0 items-center gap-2 text-sm">
    {#if selectedChain}
      <img
        src={selectedChain.logo}
        alt="{selectedChain.fullName} Logo"
        class="size-6 rounded"
      />
      <span class="truncate text-white/90">{selectedChain.fullName}</span>
    {:else}
      <span class="leading-8 text-white/90">Select chain</span>
    {/if}
  </div>
  <div class="flex items-center justify-center *:size-6">
    <ArrowDownSLine />
  </div>
{/snippet}

<Dropdown
  {open}
  {disabled}
  {trigger}
  {containerClass}
  triggerClass="grid w-full grid-cols-[1fr_24px] rounded-xl p-2 duration-200"
  menuClass="top-full z-20 mt-2 w-60 rounded-xl border border-white/20 bg-black shadow-lg"
>
  <ul class="py-4">
    {#each chains as chain (chain.name)}
      {@const isDisabled =
        chain.name === disabledChainName || chain.name === selectedChain?.name}
      <li>
        <button
          onclick={() => !isDisabled && handleSelect(chain)}
          disabled={isDisabled}
          class="flex w-full items-center gap-2 p-2 text-left text-sm {isDisabled
            ? 'cursor-not-allowed text-gray-500'
            : 'text-white/80 hover:bg-white/20 hover:text-white'}"
        >
          <img
            src={chain.logo}
            alt="{chain.fullName} Logo"
            class="size-6 rounded"
          />
          <span>{chain.fullName}</span>
        </button>
      </li>
    {/each}
  </ul>
</Dropdown>
