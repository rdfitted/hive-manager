<script lang="ts">
  import { Portal } from 'bits-ui';
  import type { Snippet } from 'svelte';

  let { x, y, label, kind, class: className = '', children }: {
    x: number; y: number; label: string; kind: 'mention' | 'slash';
    class?: string; children: Snippet;
  } = $props();
</script>

<!-- Viewport coordinates must escape Dialog's layout containment. This surface never moves focus. -->
<Portal>
  <div class="caret-menu lattice-panel lattice-scroll-chrome {className}" class:slash={kind === 'slash'} style="left: {x}px; top: {y}px;" role="listbox" tabindex="-1" aria-label={label} onpointerdown={(event) => event.stopPropagation()}>
    {@render children()}
  </div>
</Portal>

<style>
  .caret-menu {
    position: fixed;
    z-index: 200;
    pointer-events: auto;
    min-width: 200px;
    max-width: 320px;
    max-height: 240px;
    overflow-y: auto;
    box-shadow: var(--elev-3), var(--edge-lip);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-family: var(--font-body);
  }
  .caret-menu.slash { min-width: 220px; max-width: 340px; }
</style>
