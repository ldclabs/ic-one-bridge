<script lang="ts">
  import ArrowDownSLine from '$lib/icons/arrow-down-s-line.svelte'
  import ArrowRightUpLine from '$lib/icons/arrow-right-up-line.svelte'
  import Dropdown from '$lib/ui/Dropdown.svelte'
  import type { TokenInfo } from '$lib/utils/token'

  let {
    disabled = false,
    selectedToken,
    onSelectToken,
    tokens,
    containerClass = ''
  }: {
    disabled?: boolean
    selectedToken: TokenInfo | null
    onSelectToken: (token: TokenInfo) => void
    tokens: TokenInfo[]
    containerClass?: string
  } = $props()

  let open = $state(false)

  const handleSelect = (token: TokenInfo) => {
    onSelectToken(token)
    open = false
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
    <span class="*:size-6">
      <ArrowDownSLine />
    </span>
  </div>
{/snippet}

<Dropdown
  {open}
  {disabled}
  {trigger}
  {containerClass}
  triggerClass="px-0 py-2 duration-200 overflow-hidden disabled:cursor-not-allowed disabled:opacity-50"
  menuClass="top-full mt-2 w-60 rounded-xl border border-white/20 bg-black shadow-lg"
>
  <ul class="py-4">
    {#each tokens as token (token.name)}
      {@const isDisabled = token.name === selectedToken?.name}
      <li>
        <button
          onclick={() => handleSelect(token)}
          disabled={isDisabled}
          class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-white/80 hover:bg-white/20 hover:text-white disabled:cursor-not-allowed disabled:opacity-50"
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
  <div class="mb-6">
    <a
      type="button"
      class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-white/80 hover:bg-white/20 hover:text-white"
      href="https://github.com/ldclabs/ic-one-bridge/blob/main/token_listing.md"
      target="_blank"
    >
      <span>Token Listing</span>
      <span class="*:size-4"><ArrowRightUpLine /></span>
    </a>
  </div>
</Dropdown>
