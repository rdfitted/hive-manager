<script lang="ts">
  import { agentActivity, now } from '$lib/stores/agentActivity';

  let { agentId, compact = false }: { agentId: string; compact?: boolean } = $props();
  const activity = $derived($agentActivity[agentId]);
  const summary = $derived(activity?.summary || activity?.latestEventLabel || 'No activity yet');
  const age = $derived(activity?.lastActivityAt == null ? null : Math.max(0, $now - activity.lastActivityAt));
  const relative = $derived(age === null ? null : age < 60_000 ? 'just now' : age < 3_600_000
    ? `${Math.floor(age / 60_000)}m ago` : age < 86_400_000
      ? `${Math.floor(age / 3_600_000)}h ago` : `${Math.floor(age / 86_400_000)}d ago`);
</script>

<span class="activity-line" class:compact title={summary}>
  <span class="summary">{summary}</span>
  {#if relative}<span class="relative">· {relative}</span>{/if}
</span>

<style>
  .activity-line { display: flex; gap: 4px; min-width: 0; color: var(--text-secondary); font-size: 12px; }
  .activity-line.compact { font-size: 11px; }
  .summary { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .relative { flex-shrink: 0; white-space: nowrap; }
</style>
