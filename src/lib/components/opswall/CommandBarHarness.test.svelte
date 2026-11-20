<script lang="ts">
  import { untrack } from 'svelte';
  import type { AgentInfo } from '$lib/stores/sessions';
  import ChatPane from '../chat/ChatPane.svelte';
  import CommandBar from './CommandBar.svelte';

  let { sessionId, agents, targetId = $bindable(null), initialDrafts = {}, ondelivery }: {
    sessionId: string; agents: AgentInfo[]; targetId?: string | null; initialDrafts?: Record<string, string>;
    ondelivery?: (sessionId: string, agentId: string, state: string) => void;
  } = $props();
  let drafts = $state<Record<string, string>>({ ...untrack(() => initialDrafts) });
</script>

<CommandBar {sessionId} {agents} bind:targetId bind:drafts {ondelivery}>
  {#snippet children(route)}
    {@const key = `${sessionId}:${route.agentId}`}
    <ChatPane {sessionId} agentId={agents[0]?.id ?? ''} events={[]} composerRoute={route}
      bind:draft={() => drafts[key] ?? '', (value) => { drafts = { ...drafts, [key]: value }; }} />
  {/snippet}
</CommandBar>
