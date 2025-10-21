<script lang="ts">
  let {
    isConnected,
    onclick
  }: {
    isConnected: boolean
    onclick: () => void
  } = $props()
</script>

<button
  {onclick}
  class="bg-bridge-secondary hover:bg-bridge-secondary-hover flex items-center space-x-2 rounded-lg px-4 py-2 font-semibold text-white transition-colors duration-200"
>
  <span
    class={`h-2.5 w-2.5 rounded-full ${isConnected ? 'bg-green-400' : 'bg-red-500'}`}
  ></span>
  <span>{isConnected ? 'Connected' : 'Connect Wallet'}</span>
</button>
