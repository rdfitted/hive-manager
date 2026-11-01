<script lang="ts">
  import TitleBar from './TitleBar.svelte';
  import { Dialog } from '../ui';
  let { deferShortcutMount = false, shortcutsEnabled = true }: { deferShortcutMount?: boolean; shortcutsEnabled?: boolean } = $props();
  let open = $state(false);
  let pendingOpen = false;
  function handlePalette(event: KeyboardEvent) {
    const key = event.key.toLowerCase();
    if (shortcutsEnabled && event.ctrlKey && (key === '/' || key === 'k' || (event.shiftKey && key === 'p'))) {
      if (deferShortcutMount) pendingOpen = true;
      else open = true;
    }
  }
  export function mountPendingDialog() {
    if (pendingOpen) {
      pendingOpen = false;
      open = true;
    }
  }
</script>

<svelte:window onkeydown={handlePalette} />
<TitleBar />
<button type="button" onclick={() => open = true}>Open test dialog</button>
<Dialog bind:open label="Title bar test dialog">
  <button type="button" onclick={() => open = false}>Close test dialog</button>
</Dialog>
