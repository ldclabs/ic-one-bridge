<script lang="ts">
  let {
    checked = $bindable(false),
    disabled = false
  }: {
    checked: boolean
    disabled?: boolean
  } = $props()
</script>

<!-- a transfer to a contract or an exchange deposit address is unrecoverable,
     so both forms ask for the same explicit acknowledgement -->
<label
  class="flex items-center text-sm font-medium text-white/60 rtl:text-right"
  ><input
    type="checkbox"
    name="confirmAddress"
    {disabled}
    bind:checked
    class="text-primary-600 me-2 size-4 shrink-0 rounded-sm border-gray-300 bg-gray-100 ring-0 disabled:cursor-not-allowed"
  />
  I confirmed the address is correct and not an exchange or contract address. Any
  tokens sent to an incorrect address will be unrecoverable.</label
>
