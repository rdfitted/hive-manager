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
  <div class="slash-menu" style="left: {x}px; top: {y}px;" role="listbox" aria-label="Commands">
    {#each items as cmd, i (cmd.name)}
      <button
        type="button"
        class="slash-item"
        class:active={i === activeIndex}
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
    background: var(--bg-elevated);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow-lg);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .slash-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 6px 8px;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: 12px;
    font-family: var(--font-mono);
    cursor: pointer;
    text-align: left;
  }

  .slash-item:hover,
  .slash-item.active {
    background: color-mix(in srgb, var(--accent-cyan) 16%, transparent);
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
