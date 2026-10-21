<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import { terminalLifecycle, type TerminalProps } from './terminalLifecycle';

  let { agentId, isFocused, isVisible, onReady }: TerminalProps = $props();
  const id = untrack(() => agentId);
  let node: HTMLSpanElement;
  terminalLifecycle.created.push(id);

  onMount(() => {
    terminalLifecycle.terminals.set(id, {
      get props() { return { agentId, isFocused, isVisible, onReady }; }, node,
    });
    if (onReady) {
      terminalLifecycle.trace.push(`ready:${id}`);
      onReady();
    }
  });
  onDestroy(() => {
    terminalLifecycle.destroyed.push(id);
    terminalLifecycle.terminals.delete(id);
  });
</script>

<span bind:this={node}>scrollback:{agentId}</span>
