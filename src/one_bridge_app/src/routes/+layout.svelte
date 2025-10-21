<script lang="ts">
  import Toast from '$lib/components/Toast.svelte'
  import '../app.css'
  import '../lib/styles/theme.css'

  let { children } = $props()
</script>

{@render children()}

<Toast />
