<script lang="ts">
  import { Tooltip as BitsTooltip } from 'bits-ui';
  import Tooltip from './Tooltip.svelte';

  let { provider = false }: { provider?: boolean } = $props();
</script>

{#snippet help()}
  <Tooltip text="Open help" shortcut="F1" side="bottom">
    {#snippet children({ props })}
      <button {...props}>Help</button>
    {/snippet}
  </Tooltip>
{/snippet}

{#if provider}
  <BitsTooltip.Provider delayDuration={42}>
    {@render help()}
  </BitsTooltip.Provider>
{:else}
  {@render help()}
{/if}
