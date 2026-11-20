<script lang="ts">
  import type { Snippet } from 'svelte';
  import AgentPaneBody from './AgentPaneBody.svelte';
  import ChatPane from './ChatPane.svelte';
  import type { PaneMode } from './paneMode';

  let { mode, terminal, statusLine }: { mode: PaneMode; terminal: Snippet; statusLine: Snippet } = $props();
</script>

<AgentPaneBody {mode} {terminal} {statusLine}>
  {#snippet chat()}
    <ChatPane sessionId="test-session" agentId="test-agent" events={[]} visible={false} />
  {/snippet}
</AgentPaneBody>
