<script lang="ts">
  let {
    value = $bindable(),
    disabled = false,
    oninput
  }: {
    value: number | undefined
    disabled?: boolean
    oninput: (event: Event) => void
  } = $props()
</script>

<!-- `min` keeps a negative amount from ever reaching the parser -->
<input
  type="number"
  name="tokenAmount"
  {disabled}
  bind:value
  {oninput}
  inputmode="decimal"
  placeholder="0.0"
  min="0"
  step="any"
  data-1p-ignore
  autocomplete="off"
  class="w-full flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left font-mono text-xl leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20 disabled:cursor-not-allowed"
/>
