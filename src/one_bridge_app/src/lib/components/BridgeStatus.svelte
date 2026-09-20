<script lang="ts">
  import type { BridgeCanisterAPI } from '$lib/canisters/bridge.svelte'
  const {
    bridge,
    chains = [],
    wallet = false,
    native = false
  }: {
    bridge: BridgeCanisterAPI | null
    chains?: string[]
    wallet?: boolean
    native?: boolean
  } = $props()
  const runtime = $derived(bridge?.runtime)
  const reason = $derived(
    wallet && chains[0] === 'ICP'
      ? null
      : bridge?.readiness(chains, !wallet, native)
  )
</script>

{#if reason}
  <p
    role="status"
    class="rounded-lg border border-amber-300/20 bg-amber-300/5 px-4 py-3 text-sm leading-relaxed text-amber-100"
    >{reason}</p
  >
{/if}
{#if runtime}
  <details
    class="rounded-lg border border-white/10 px-4 py-3 text-xs text-white/60"
  >
    <summary class="cursor-pointer text-white/80"
      >Bridge status & limits</summary
    >
    <dl class="mt-3 grid grid-cols-2 gap-x-4 gap-y-2">
      <dt>Ledger verification</dt><dd
        >{runtime.ledger_verified ? 'Ready' : 'In progress'}</dd
      >
      <dt>EVM / Solana keys</dt><dd
        >{runtime.keys_ready[0] ? 'Ready' : 'Waiting'} / {runtime.keys_ready[1]
          ? 'Ready'
          : 'Waiting'}</dd
      >
      <dt>Solana token</dt><dd
        >{runtime.svm_mint_verified ? 'Verified' : 'Not verified'}</dd
      >
      <dt>Migration remaining</dt><dd>{String(runtime.migration_remaining)}</dd>
      <dt>Pending tasks</dt><dd>{String(runtime.pending_count)}</dd>
      <dt>Unresolved operations</dt><dd
        >{String(runtime.unresolved_operations)}</dd
      >
      <dt>Queue limit / per account</dt><dd
        >{runtime.resource_limits.max_pending} / {runtime.resource_limits
          .max_pending_per_user}</dd
      >
      <dt>Requests per account / hour</dt><dd
        >{runtime.resource_limits.requests_per_user_hour}</dd
      >
      <dt>Requests globally / hour</dt><dd
        >{runtime.resource_limits.requests_per_hour}</dd
      >
      <dt>Concurrent requests</dt><dd
        >{runtime.resource_limits.max_active_requests}</dd
      >
    </dl>
    <p class="mt-3 leading-relaxed"
      >Hourly limits reset at the start of the next UTC hour. Remaining request
      allowance is not published. Refreshing activity does not use the signing
      request allowance.</p
    >
    {#each runtime.evm_fee_limits.filter( ([chain]) => chains.includes(chain) ) as [chain, limits]}
      <p class="mt-2 leading-relaxed"
        >{chain} gas ceiling: {bridge?.displayNativeAmount(
          chain,
          limits.max_transaction_fee
        )} per transaction; {bridge?.displayNativeAmount(
          chain,
          limits.max_hourly_fee
        )} per hour for bridge payouts.</p
      >
    {/each}
  </details>
{/if}
