<script lang="ts">
  import type { SlashCommand } from '$lib/composer/commands';

  interface Props {
    items: SlashCommand[];
    activeIndex: number;
    x: number;
    y: number;
    onselect: (cmd: SlashCommand) => void;
  }

  let { items, activeIndex, x, y, onselect }: Props = $props();
</script>

{#if items.length > 0}
  <div class="slash-menu lattice-panel lattice-scroll-chrome" style="left: {x}px; top: {y}px;" role="listbox" aria-label="Commands">
    {#each items as cmd, i (cmd.name)}
      <button
        type="button"
        class="lattice-btn lattice-btn--menu-item lattice-btn--compact"
        role="option"
        aria-selected={i === activeIndex}
        onmousedown={(e) => { e.preventDefault(); onselect(cmd); }}
      >
        <span class="cmd">{cmd.label}</span>
        <span class="desc">{cmd.description}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .slash-menu {
    position: fixed;
    z-index: 200;
    min-width: 220px;
    max-width: 340px;
    max-height: 240px;
    overflow-y: auto;
    box-shadow: var(--elev-3), var(--edge-lip);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .cmd {
    color: var(--accent-cyan);
    font-weight: 600;
    flex-shrink: 0;
  }

  .desc {
    color: var(--text-secondary);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
