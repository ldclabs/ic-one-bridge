<script lang="ts">
  let {
    value = $bindable(''),
    disabled = false,
    oninput
  }: {
    value: string
    disabled?: boolean
    oninput: (event: Event) => void
  } = $props()
</script>

<!-- Keep decimal input exact, including amounts larger than Number.MAX_SAFE_INTEGER. -->
<input
  type="text"
  name="tokenAmount"
  {disabled}
  bind:value
  {oninput}
  inputmode="decimal"
  placeholder="0.0"
  data-1p-ignore
  autocomplete="off"
  class="w-full flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left font-mono text-xl leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20 disabled:cursor-not-allowed"
/>
