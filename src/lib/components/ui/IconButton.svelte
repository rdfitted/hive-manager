<script lang="ts">
  import type { Component, Snippet } from 'svelte';
  import type { HTMLButtonAttributes } from 'svelte/elements';
  import Tooltip from './Tooltip.svelte';

  type Icon = Component<{ size?: number }>;
  type Props = Omit<HTMLButtonAttributes, 'children' | 'size' | 'aria-label' | 'aria-pressed'> & {
    label: string;
    tooltip: string;
    shortcut?: string;
    pressed?: boolean;
    variant?: 'ghost' | 'primary';
    size?: 'sm' | 'md';
    icon: Icon | Snippet;
  };

  let {
    label, tooltip, icon, shortcut, pressed, variant = 'ghost', size = 'md',
    class: className, id, ...buttonProps
  }: Props = $props();

  // Zero-argument snippets take only the render anchor. Phosphor components
  // take the anchor and props in both client and server builds.
  const IconComponent = $derived(icon.length > 1 ? icon as Icon : undefined);
  const iconSnippet = $derived(IconComponent ? undefined : icon as Snippet);
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
        {#if IconComponent}
          <IconComponent size={size === 'sm' ? 14 : 16} />
        {:else}
          {@render iconSnippet?.()}
        {/if}
      </span>
    </button>
  {/snippet}
</Tooltip>

<style>
  .lattice-icon-button {
    display: inline-grid;
    flex: none;
    place-items: center;
    width: var(--control-height);
    height: var(--control-height);
    padding: 0;
    border: 1px solid var(--border-control);
    border-radius: var(--radius-full);
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
  }

  .lattice-icon-button > span { display: inline-flex; }

  .lattice-icon-button[data-size='sm'] {
    width: var(--control-height-sm);
    height: var(--control-height-sm);
  }

  .lattice-icon-button[data-variant='ghost']:hover:not(:disabled),
  .lattice-icon-button[data-variant='ghost'][aria-pressed='true'] {
    background: var(--accent-bg);
    color: var(--accent);
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
