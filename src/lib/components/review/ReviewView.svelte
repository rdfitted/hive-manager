<script lang="ts">
  import { apiUrl } from '$lib/config';
  import { activeSession } from '$lib/stores/sessions';
  import Diff from '../renderers/Diff.svelte';

  interface Cell { id: string; name: string; agents: string[] }
  interface FileDiff { path: string; unified: string; binary: boolean; truncated: boolean }
  interface ReviewDiff { base: string; agent_id: string; files: FileDiff[]; truncated: boolean }
  interface Draft { file: string; line: number | undefined; comment: string }
  let cells = $state<Cell[]>([]);
  let cellId = $state('');
  let agentId = $state('');
  let diff = $state<ReviewDiff | null>(null);
  let drafts = $state<Record<string, Draft>>({});
  let reviewed = $state<Record<string, boolean>>({});
  let loading = $state(false);
  let sending = $state(false);
  let error = $state('');
  let receipt = $state('');
  let revision = 0;
  const selectedCell = $derived(cells.find((cell) => cell.id === cellId));
  const sessionId = $derived($activeSession?.id ?? '');
  const hasComments = $derived(Object.values(drafts).some((draft) => draft.file.trim() || draft.comment.trim()));
  const invalidLine = $derived(Object.values(drafts).some((draft) => draft.comment.trim() && (!draft.line || !Number.isInteger(draft.line) || draft.line < 1)));

  async function request(path: string, options?: RequestInit) {
    const response = await fetch(apiUrl(path), options);
    if (!response.ok) throw new Error(`Request failed (${response.status})`);
    return response;
  }

  $effect(() => {
    const id = sessionId;
    let current = true;
    cells = []; cellId = ''; agentId = ''; error = '';
    if (id) {
      void request(`/api/sessions/${encodeURIComponent(id)}/cells`)
        .then((response) => response.json())
        .then((value: Cell[]) => {
          if (!current) return;
          cells = value;
          cellId = value[0]?.id ?? '';
          agentId = value[0]?.agents[0] ?? '';
        }).catch((cause: Error) => { if (current) error = cause.message; });
    }
    return () => { current = false; };
  });

  $effect(() => {
    const id = sessionId; const cell = cellId; const agent = agentId;
    revision += 1;
    let current = true;
    diff = null; drafts = {}; reviewed = {}; receipt = ''; error = ''; loading = false;
    if (id && cell && agent) {
      loading = true;
      void request(`/api/sessions/${encodeURIComponent(id)}/cells/${encodeURIComponent(cell)}/review-diff?agent_id=${encodeURIComponent(agent)}`)
        .then((response) => response.json())
        .then((value: ReviewDiff) => {
          if (!current) return;
          if (value.agent_id !== agent) throw new Error('Review response belongs to another principal.');
          drafts = Object.fromEntries(value.files.map((file) => [file.path, { file: '', line: undefined, comment: '' }]));
          diff = value;
        }).catch((cause: Error) => { if (current) error = cause.message; })
        .finally(() => { if (current) loading = false; });
    }
    return () => { current = false; };
  });

  async function send() {
    if (sending || !diff || diff.agent_id !== agentId || !hasComments || invalidLine || loading) return;
    const id = sessionId; const agent = diff.agent_id; const cell = cellId;
    const sentRevision = revision;
    const comments = Object.entries(drafts).flatMap(([path, draft]) => [
      draft.file.trim() ? `${path}: ${draft.file.trim()}` : '',
      draft.comment.trim() ? `${path}:${draft.line || 'unspecified line'}: ${draft.comment.trim()}` : '',
    ]).filter(Boolean);
    sending = true; error = ''; receipt = '';
    try {
      const response = await request(`/api/sessions/${encodeURIComponent(id)}/inject`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ target_agent_id: agent, message: `Requested changes (advisory review):\n\n${comments.join('\n\n')}`, submit: true }),
      });
      const body = await response.text();
      if (revision === sentRevision && sessionId === id && agentId === agent && cellId === cell) receipt = body;
    } catch (cause) {
      if (revision === sentRevision && sessionId === id && agentId === agent && cellId === cell) error = String(cause);
    } finally { sending = false; }
  }
</script>

<div class="review-view lattice-scroll-content">
  {#if !sessionId}<p>Select a session to review changes.</p>{:else}
    <div class="pickers">
      <label>Cell<select value={cellId} disabled={sending} onchange={(event) => { cellId = event.currentTarget.value; agentId = cells.find((cell) => cell.id === cellId)?.agents[0] ?? ''; }}>
        {#each cells as cell (cell.id)}<option value={cell.id}>{cell.name}</option>{/each}
      </select></label>
      <label>Principal<select bind:value={agentId} disabled={sending}>
        {#each selectedCell?.agents ?? [] as agent (agent)}<option value={agent}>{agent}</option>{/each}
      </select></label>
    </div>
    <p class="muted">Advisory review · reviewed marks stay in this view.</p>
    {#if error}<p role="alert">{error}</p>{/if}
    {#if loading}<p role="status">Loading changes…</p>{:else if diff}
      <p class="base">Base: {diff.base}</p>
      {#if diff.truncated}<p role="status">Diff limit reached. Some changes are omitted.</p>{/if}
      {#if !diff.files.length}<p>No changes for this principal.</p>{/if}
      {#each diff.files as file (file.path)}
        <section class="file-review" aria-label={file.path}>
          <details open>
            <summary>{file.path}</summary>
            {#if file.binary}<p>Binary file — preview unavailable.</p>{:else}<Diff data={{ unified: file.unified }} />{/if}
            {#if file.truncated}<p>File preview truncated.</p>{/if}
          </details>
          <label class="reviewed"><input type="checkbox" bind:checked={reviewed[file.path]} /> Reviewed {file.path}</label>
          <label>File comment for {file.path}<textarea bind:value={drafts[file.path].file} disabled={sending}></textarea></label>
          {#if !file.binary}
            <label>Line number for {file.path}<input type="number" min="1" step="1" bind:value={drafts[file.path].line} disabled={sending} /></label>
            <label>Line comment for {file.path}<textarea bind:value={drafts[file.path].comment} disabled={sending}></textarea></label>
          {/if}
        </section>
      {/each}
      {#if invalidLine}<p>Enter a positive line number for each line comment.</p>{/if}
      <button class="lattice-btn lattice-btn--primary" disabled={!hasComments || invalidLine || sending} onclick={send}>{sending ? 'Sending…' : 'Send requested changes'}</button>
    {:else if !loading && cells.length}<p>Select a principal to review.</p>{/if}
    {#if receipt}<section aria-label="Injection receipt"><h3>Receipt</h3><pre>{receipt}</pre></section>{/if}
  {/if}
</div>

<style>
  .review-view { overflow: auto; padding: 12px; color: var(--text-primary); font-size: var(--text-small); }
  .pickers, label { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
  .pickers { gap: 10px; }
  select, textarea, input { max-width: 100%; color: var(--text-primary); background: var(--bg-sunken); border: 1px solid var(--border-structural); border-radius: var(--radius-sm); padding: 6px; font: inherit; }
  textarea { resize: vertical; min-height: 52px; }
  .muted, .base { color: var(--text-secondary); }
  .base, pre, summary { overflow-wrap: anywhere; }
  .base, pre { font-family: var(--font-mono); font-size: 11px; }
  pre { white-space: pre-wrap; }
  .file-review { padding: 12px 0; border-top: 1px solid var(--border-structural); display: flex; flex-direction: column; gap: 10px; }
  summary { cursor: pointer; margin-bottom: 8px; font-family: var(--font-mono); }
  .reviewed { flex-direction: row; align-items: center; }
  .review-view > button { max-width: 100%; white-space: normal; }
  @media (forced-colors: active) {
    select, textarea, input, .file-review { border-color: CanvasText; }
    .muted, .base { color: CanvasText; }
  }
</style>
