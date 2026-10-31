<script lang="ts">
  import { Dialog as BitsDialog, AlertDialog } from 'bits-ui';
  import type { Snippet } from 'svelte';
  import type { HTMLAttributes } from 'svelte/elements';

  type Props = Omit<HTMLAttributes<HTMLDivElement>, 'children'> & {
    open: boolean;
    onOpenChange?: (open: boolean) => void;
    label?: string;
    labelledby?: string;
    variant?: 'dialog' | 'alert';
    placement?: 'center' | 'top';
    children?: Snippet;
  };

  let {
    open = $bindable(false), onOpenChange, label, labelledby,
    variant = 'dialog', placement = 'center', class: className = '', children, ...rest
  }: Props = $props();
  const Primitive = $derived(variant === 'alert' ? AlertDialog : BitsDialog);

  function changeOpen(value: boolean) {
    open = value;
    onOpenChange?.(value);
  }

  function interactOutside(event: PointerEvent) {
    if (event.target instanceof Element && event.target.closest('[data-window-chrome]')) {
      event.preventDefault();
    }
  }
</script>

<Primitive.Root {open} onOpenChange={changeOpen}>
  {#if open}
    <Primitive.Portal>
      <Primitive.Overlay class="dialog-backdrop lattice-modal-backdrop" />
      <Primitive.Content onInteractOutside={interactOutside}>
        {#snippet child({ props })}
          <div {...props} {...rest} class="dialog-content lattice-modal lattice-scroll-chrome {className}" class:top={placement === 'top'} aria-modal="true" aria-label={label} aria-labelledby={labelledby}>
            {@render children?.()}
          </div>
        {/snippet}
      </Primitive.Content>
    </Primitive.Portal>
  {/if}
</Primitive.Root>

<style>
  .dialog-backdrop { position: fixed; inset: 0; z-index: 110; }
  .dialog-content {
    position: fixed;
    z-index: 111;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: max-content;
    max-width: calc(100vw - 32px);
    max-height: 90vh;
    font-family: var(--font-body);
  }
  .dialog-content.top { top: min(14vh, 6rem); transform: translateX(-50%); }
</style>
