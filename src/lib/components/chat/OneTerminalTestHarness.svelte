<script module lang="ts">
  import type Terminal from '../Terminal.svelte';

  let terminalComponent: typeof Terminal;

  export function useTerminal(component: typeof Terminal) {
    terminalComponent = component;
  }
</script>

<script lang="ts">
  import type { ComponentProps } from 'svelte';
  import CountingTerminal from '../session/testing/CountingTerminal.svelte';

  const props: ComponentProps<typeof Terminal> = $props();
  const ActualTerminal = terminalComponent;
</script>

<CountingTerminal agentId={props.agentId} isFocused={props.isFocused ?? false} isVisible={props.isVisible ?? true} />
<ActualTerminal {...props} />
