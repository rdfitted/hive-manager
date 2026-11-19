<script lang="ts">
  import { Tooltip as BitsTooltip, type TooltipContentProps, type TooltipTriggerProps } from 'bits-ui';
  import { getAllContexts } from 'svelte';

  type Props = {
    text: string;
    shortcut?: string;
    side?: TooltipContentProps['side'];
    children: NonNullable<TooltipTriggerProps['child']>;
    triggerProps?: Omit<TooltipTriggerProps, 'child' | 'children'>;
  };

  let { text, shortcut, side = 'top', children, triggerProps }: Props = $props();
  // Bits UI's runed context has a named symbol but exposes no public provider lookup.
  // Isolated renders supply a provider only when the application provider is absent.
  const hasProvider = [...getAllContexts().keys()]
    .some((key) => typeof key === 'symbol' && key.description === 'Tooltip.Provider');

  function descriptions(tooltipId: unknown) {
    const externalId = triggerProps?.['aria-describedby'];
    return [externalId, typeof tooltipId === 'string' ? tooltipId : undefined]
      .filter(Boolean).join(' ') || undefined;
  }
</script>

{#snippet tooltip()}
  <BitsTooltip.Root>
    <BitsTooltip.Trigger {...triggerProps} aria-describedby={undefined}>
      {#snippet child({ props })}
        {@render children({ props: { ...props, 'aria-describedby': descriptions(props['aria-describedby']) } })}
      {/snippet}
    </BitsTooltip.Trigger>
    <BitsTooltip.Portal>
      <BitsTooltip.Content {side} sideOffset={6} collisionPadding={8} class="lattice-tooltip" role="tooltip">
        {text}{#if shortcut} <kbd>{shortcut}</kbd>{/if}
      </BitsTooltip.Content>
    </BitsTooltip.Portal>
  </BitsTooltip.Root>
{/snippet}

{#if hasProvider}
  {@render tooltip()}
{:else}
  <BitsTooltip.Provider>
    {@render tooltip()}
  </BitsTooltip.Provider>
{/if}
