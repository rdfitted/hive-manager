<script lang="ts">
  import { untrack } from 'svelte';
  import type { AgentInfo } from '$lib/stores/sessions';
  import CommandBar from './CommandBar.svelte';
  let { sessionId, agents, targetId = $bindable(null), initialDrafts = {} }: {
    sessionId: string; agents: AgentInfo[]; targetId?: string | null; initialDrafts?: Record<string, string>;
  } = $props();
  let drafts = $state<Record<string, string>>({ ...untrack(() => initialDrafts) });
</script>

<CommandBar {sessionId} {agents} bind:targetId bind:drafts />
