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
  <div class="mention-menu lattice-panel lattice-scroll-chrome" style="left: {x}px; top: {y}px;" role="listbox" aria-label="Mentions">
    {#each items as item, i (item.kind + ':' + item.id)}
      <button
        type="button"
        class="lattice-btn lattice-btn--menu-item lattice-btn--compact"
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
    box-shadow: var(--elev-3), var(--edge-lip);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
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
