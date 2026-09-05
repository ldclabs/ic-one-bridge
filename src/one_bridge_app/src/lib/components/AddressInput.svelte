<script lang="ts">
  let {
    value = $bindable(),
    disabled = false,
    placeholder = '0x...',
    oninput
  }: {
    value: string
    disabled?: boolean
    placeholder?: string
    oninput: (event: Event) => void
  } = $props()
</script>

<input
  type="text"
  name="thirdAddress"
  {disabled}
  bind:value
  {oninput}
  {placeholder}
  data-1p-ignore
  autocomplete="off"
  class="mb-1 w-full min-w-0 flex-1 rounded-xl border border-white/10 bg-white/10 p-2 text-left leading-8 ring-0 transition-all duration-200 outline-none placeholder:text-gray-500 invalid:border-red-400 focus:bg-white/20 disabled:cursor-not-allowed"
/>
