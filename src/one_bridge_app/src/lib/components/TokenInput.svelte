<script lang="ts">
  import ArrowDownSLine from '$lib/icons/arrow-down-s-line.svelte'
  import type { TokenInfo } from '$lib/utils/token'
  import Dropdown from './Dropdown.svelte'

  let {
    value = $bindable(),
    disabled = false,
    selectedToken,
    onSelectToken,
    validate = (element: HTMLInputElement) => {},
    tokens
  }: {
    value: number | undefined
    disabled?: boolean
    selectedToken: TokenInfo | null
    onSelectToken: (token: TokenInfo) => void
    validate?: (element: HTMLInputElement) => void
    tokens: TokenInfo[]
  } = $props()

  let open = $state(false)

  const handleSelect = (token: TokenInfo) => {
    onSelectToken(token)
    open = false
  }

  function oninput(event: Event) {
    const target = event.target as HTMLInputElement
    validate(target)
  }
</script>

{#snippet trigger()}
  <div class="flex min-w-0 items-center gap-2 text-sm">
    {#if selectedToken}
      <img
        src={selectedToken.logo}
        alt="{selectedToken.name} Logo"
        class="size-8 rounded"
      />
      <span class="truncate font-medium text-white/90"
        >{selectedToken.symbol}</span
      >
    {:else}
      <span class="leading-8 font-medium text-white/90">Select token</span>
    {/if}
  </div>
  <div class="flex items-center justify-center *:size-6">
    <ArrowDownSLine />
  </div>
{/snippet}

<div class="flex w-full flex-row items-center gap-4">
  <input
    type="number"
    name="tokenAmount"
    bind:value
    {oninput}
    inputmode="decimal"
    step="1.0"
    placeholder="0.0"
    pattern="^[0-9]*[.,]?[0-9]*$"
    class="min-w-0 flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left font-mono text-xl leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20"
  />
  <Dropdown
    {open}
    {disabled}
    {trigger}
    containerClass="flex-none shrink-0"
    triggerClass="grid grid-cols-[1fr_24px] rounded-xl px-0 py-2 duration-200 overflow-hidden"
    menuClass="top-full mt-2 w-60 rounded-xl border border-white/20 bg-black shadow-lg"
  >
    <ul class="py-4">
      {#each tokens as token (token.name)}
        {@const isDisabled = token.name === selectedToken?.name}
        <li>
          <button
            onclick={() => handleSelect(token)}
            disabled={isDisabled}
            class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm {isDisabled
              ? 'cursor-not-allowed text-gray-500'
              : 'text-white/80 hover:bg-white/20 hover:text-white'}"
          >
            <img
              src={token.logo}
              alt="{token.name} Logo"
              class="size-8 rounded"
            />
            <div class="flex flex-col items-start">
              <span>{token.symbol}</span>
              <span class="text-xs text-white/60">{token.name}</span>
            </div>
          </button>
        </li>
      {/each}
    </ul>
  </Dropdown>
</div>
