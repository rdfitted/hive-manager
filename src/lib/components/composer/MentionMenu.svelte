<script lang="ts">
  import type { MentionItem } from '$lib/composer/sources';

  interface Props {
    items: MentionItem[];
    /** Highlighted index (controlled by the parent Composer for keyboard nav). */
    activeIndex: number;
    /** Caret-anchor position in viewport pixels. */
    x: number;
    y: number;
    onselect: (item: MentionItem) => void;
  }

  let { items, activeIndex, x, y, onselect }: Props = $props();

  function kindGlyph(kind: MentionItem['kind']): string {
    if (kind === 'agent') return '@';
    if (kind === 'session') return '#';
    return '□'; // file
  }
</script>

{#if items.length > 0}
  <div class="mention-menu" style="left: {x}px; top: {y}px;" role="listbox" aria-label="Mentions">
    {#each items as item, i (item.kind + ':' + item.id)}
      <button
        type="button"
        class="mention-item"
        class:active={i === activeIndex}
        role="option"
        aria-selected={i === activeIndex}
        onmousedown={(e) => { e.preventDefault(); onselect(item); }}
      >
        <span class="glyph">{kindGlyph(item.kind)}</span>
        <span class="label">{item.label}</span>
        {#if item.detail}<span class="detail">{item.detail}</span>{/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .mention-menu {
    position: fixed;
    z-index: 200;
    min-width: 200px;
    max-width: 320px;
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

  .mention-item {
    display: flex;
    align-items: center;
    gap: 8px;
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

  .mention-item:hover,
  .mention-item.active {
    background: color-mix(in srgb, var(--accent-cyan) 16%, transparent);
  }

  .glyph {
    color: var(--accent-cyan);
    flex-shrink: 0;
    width: 12px;
    text-align: center;
  }

  .label {
    font-weight: 600;
    white-space: nowrap;
  }

  .detail {
    color: var(--text-secondary);
    font-size: 11px;
    margin-left: auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 160px;
  }
</style>
