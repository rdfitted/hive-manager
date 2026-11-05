<script lang="ts">
  import ToolRenderHost from '../renderers/ToolRenderHost.svelte';
  import { formatRaw, passiveToolResult, type TranscriptRow } from './transcript';

  let { group, expanded = false, onchange }: {
    group: TranscriptRow; expanded?: boolean; onchange(open: boolean): void;
  } = $props();
</script>

<details class="activity-group lattice-forced-colors-boundary" open={expanded} ontoggle={(event) => onchange(event.currentTarget.open)}>
  <summary>Worked for {Math.round((group.durationMs ?? 0) / 1000)}s{#if group.failedCount} · {group.failedCount} failed{/if}</summary>
  {#if expanded}
    {#each group.tools ?? [] as tool (tool.id)}
      <article class="tool-call" aria-label={tool.name}>
        <h4>{tool.name} · {tool.failed ? 'Failed' : tool.complete ? 'Complete' : 'Running'}</h4>
        {#if tool.input !== undefined}<pre aria-label="Tool input">{formatRaw(tool.input)}</pre>{/if}
        {#if tool.result !== undefined}
          {@const passive = passiveToolResult(tool.result)}
          {#if passive}
            <ToolRenderHost message={{ id: tool.id, timestamp: tool.ts, from: 'tool', content: '', renderer: passive.renderer, data: passive.data }} />
          {:else}<pre aria-label="Tool result">{formatRaw(tool.result)}</pre>{/if}
        {/if}
        {#if tool.truncated}<span class="truncation">Output truncated</span>{/if}
      </article>
    {/each}
  {/if}
</details>

<style>
  .activity-group { margin-bottom: var(--space-3); color: var(--chat-text-meta); }
  summary { cursor: pointer; font-size: var(--text-small); padding: var(--space-2) 0; }
  .tool-call { padding: var(--space-2); border-radius: var(--radius-md); background: var(--chat-well); color: var(--chat-text); }
  h4 { margin: 0; font-size: var(--text-small); font-weight: 500; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--font-mono); font-size: var(--text-small); }
  .truncation { color: var(--chat-text-meta); font-size: var(--text-small); }
  @media (forced-colors: active) { summary { color: ButtonText; } .tool-call { border: 1px solid CanvasText; } }
</style>
