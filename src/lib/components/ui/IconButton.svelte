<script lang="ts">
  import type { Component, Snippet } from 'svelte';
  import type { HTMLButtonAttributes } from 'svelte/elements';
  import Glyph, { type GlyphName } from './Glyph.svelte';
  import Tooltip from './Tooltip.svelte';

  type Icon = Component<{ size?: number }>;
  type Props = Omit<HTMLButtonAttributes, 'children' | 'size' | 'aria-label' | 'aria-pressed'> & {
    label: string;
    tooltip: string;
    shortcut?: string;
    pressed?: boolean;
    variant?: 'ghost' | 'primary';
    size?: 'xs' | 'sm' | 'md';
    /** A Phosphor component, a zero-argument snippet, or a name from the local glyph set. */
    icon: Icon | Snippet | GlyphName;
  };

  let {
    label, tooltip, icon, shortcut, pressed, variant = 'ghost', size = 'md',
    class: className, id, ...buttonProps
  }: Props = $props();

  // Zero-argument snippets take only the render anchor. Phosphor components
  // take the anchor and props in both client and server builds.
  const glyph = $derived(typeof icon === 'string' ? icon : undefined);
  const IconComponent = $derived(typeof icon !== 'string' && icon.length > 1 ? icon as Icon : undefined);
  const iconSnippet = $derived(typeof icon === 'string' || IconComponent ? undefined : icon as Snippet);
</script>

<Tooltip text={tooltip} {shortcut} triggerProps={{ ...buttonProps, id: id ?? undefined }}>
  {#snippet children({ props })}
    <button
      {...props}
      class={['lattice-icon-button', className]}
      data-variant={variant}
      data-size={size}
      aria-label={label}
      aria-pressed={pressed}
    >
      <span aria-hidden="true">
        {#if glyph}
          <Glyph name={glyph} size={size === 'md' ? 16 : 14} />
        {:else if IconComponent}
          <IconComponent size={size === 'md' ? 16 : 14} />
        {:else}
          {@render iconSnippet?.()}
        {/if}
      </span>
    </button>
  {/snippet}
</Tooltip>

<style>
  /* Mockup pill: no ring at rest, the glyph carries the control. Hover lifts the
     ink; pressed takes the strong glass with a strong hairline. */
  .lattice-icon-button {
    display: inline-grid;
    flex: none;
    place-items: center;
    width: var(--control-height);
    height: var(--control-height);
    padding: 0;
    border: 1px solid transparent;
    border-radius: var(--radius-full);
    background: transparent;
    color: var(--ink-muted);
    cursor: pointer;
    transition: background-color var(--motion-ui), color var(--motion-ui), border-color var(--motion-ui);
  }

  .lattice-icon-button > span { display: inline-flex; }

  .lattice-icon-button[data-size='sm'] {
    width: var(--control-height-sm);
    height: var(--control-height-sm);
  }

  .lattice-icon-button[data-size='xs'] {
    width: 24px;
    height: 24px;
  }

  .lattice-icon-button[data-variant='ghost']:hover:not(:disabled) {
    background: var(--wash-strong);
    color: var(--ink);
  }

  .lattice-icon-button[data-variant='ghost'][aria-pressed='true'] {
    background: var(--glass-strong);
    border-color: var(--line-strong);
    color: var(--ink);
  }

  .lattice-icon-button[data-variant='primary'] {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-ink);
  }

  .lattice-icon-button:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }

  .lattice-icon-button:disabled {
    opacity: .55;
    cursor: default;
  }

  @media (forced-colors: active) {
    .lattice-icon-button { border-color: ButtonText; }
    .lattice-icon-button[aria-pressed='true'] { border-color: Highlight; }
    .lattice-icon-button:focus-visible { outline-color: Highlight; }
    .lattice-icon-button:disabled { border-color: GrayText; opacity: 1; }
  }
</style>
