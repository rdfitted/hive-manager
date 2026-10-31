<script lang="ts">
  import TitleBar from './TitleBar.svelte';
  import { Dialog } from '../ui';
  let open = $state(false);
  function handlePalette(event: KeyboardEvent) {
    const key = event.key.toLowerCase();
    if (event.ctrlKey && (key === '/' || key === 'k' || (event.shiftKey && key === 'p'))) open = true;
  }
</script>

<svelte:window onkeydown={handlePalette} />
<TitleBar />
<button type="button" onclick={() => open = true}>Open test dialog</button>
<Dialog bind:open label="Title bar test dialog">
  <button type="button" onclick={() => open = false}>Close test dialog</button>
</Dialog>
