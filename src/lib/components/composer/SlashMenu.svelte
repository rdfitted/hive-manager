<script lang="ts">
  import CaretMenu from '../ui/CaretMenu.svelte';
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
  <CaretMenu {x} {y} label="Commands" kind="slash" class="slash-menu">
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
  </CaretMenu>
{/if}

<style>

  .cmd {
    color: var(--accent);
    font-weight: 600;
    flex-shrink: 0;
  }

  .desc {
    color: var(--text-muted);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
