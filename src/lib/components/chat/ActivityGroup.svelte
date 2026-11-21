<script lang="ts">
  import ToolRenderHost from '../renderers/ToolRenderHost.svelte';
  import Glyph from '../ui/Glyph.svelte';
  import { formatRaw, passiveToolResult, type TranscriptRow } from './transcript';

  let { group, expanded = false, onchange }: {
    group: TranscriptRow; expanded?: boolean; onchange(open: boolean): void;
  } = $props();
  const toolCount = $derived(group.tools?.length ?? 0);
</script>

<details class="activity-group lattice-forced-colors-boundary" open={expanded} ontoggle={(event) => onchange(event.currentTarget.open)}>
  <summary><span class="caret"><Glyph name="chevron" size={10} /></span><span class="label">{toolCount} {toolCount === 1 ? 'tool call' : 'tool calls'}</span><span class="sep">{' · '}</span><span class="stat">{Math.round((group.durationMs ?? 0) / 1000)}s</span>{#if group.failedCount}<span class="sep">{' · '}</span><span class="stat failed">{group.failedCount} failed</span>{/if}</summary>
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
  /* A run of tool calls folds into one quiet row with its count and duration. */
  .activity-group { overflow: hidden; border: 1px solid var(--line); border-radius: var(--control-radius); color: var(--ink-muted); font-size: var(--text-small); }
  summary { display: flex; align-items: center; gap: 9px; min-height: 30px; padding: 0 11px; cursor: pointer; list-style: none; }
  summary::-webkit-details-marker { display: none; }
  summary:hover { color: var(--ink); }
  summary:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
  .caret { display: inline-flex; color: var(--ink-dim); transition: transform var(--motion-ui); }
  .activity-group[open] .caret { transform: rotate(90deg); }
  .sep { display: none; }
  .stat { color: var(--ink-dim); font: 11px/1 var(--font-mono); }
  .label + .sep + .stat { margin-left: auto; }
  .stat.failed { color: var(--tone-err); }
  .tool-call { margin: 0 9px 9px; padding: var(--space-2) var(--space-3); border-radius: var(--radius-sm); background: rgb(0 0 0 / .45); color: var(--chat-text); }
  h4 { margin: 0; color: var(--ink-muted); font: 500 11.5px/1.6 var(--font-mono); }
  pre { margin: 4px 0 0; white-space: pre-wrap; overflow-wrap: anywhere; font: 11.5px/1.6 var(--font-mono); }
  .truncation { color: var(--chat-text-meta); font-size: var(--text-small); }
  @media (forced-colors: active) { summary { color: ButtonText; } .tool-call { border: 1px solid CanvasText; } }
</style>
