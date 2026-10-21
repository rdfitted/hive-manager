<script lang="ts">
  import type { SessionTemplate } from '$lib/types/domain';

  export let mode = 'hive';
  export let value = '';
  export let projectPath = '';
  export let workerCount = 1;
  export let maxWorkers = 3;
  export let preset = 'custom';
  export let templates: SessionTemplate[] = [];
  export let templatesLoading = false;
  export let templatesError: string | null = null;
  export let launching = false;
  export let error = '';
  export let onmode: (mode: string) => void;
  export let onvalue: (value: string) => void;
  export let onworkers: (count: number) => void;
  export let onpreset: (id: string) => void;
  export let onbrowse: () => void;
  export let onadvanced: () => void;
  export let onlaunch: () => void;
  export let oncancel: () => void;

  $: minWorkers = mode === 'fusion' || mode === 'debate' ? Math.min(2, workerCount) : 1;

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.isComposing) {
      event.preventDefault();
      if (!launching && projectPath.trim() && mode !== 'templates') onlaunch();
    }
  }
</script>

<form class="launch-composer" on:submit|preventDefault={onlaunch}>
  <label class="task-label" for="launch-task">What would you like to work on?</label>
  <textarea
    id="launch-task"
    class="lattice-input lattice-scroll-chrome"
    {value}
    on:input={(event) => onvalue(event.currentTarget.value)}
    rows="6"
    placeholder="Describe a task for your session…"
    disabled={launching}
    aria-describedby="launch-submit-hint"
    on:keydown={handleKeydown}
  ></textarea>

  <div class="launch-chips">
    <label class="launch-chip">
      <span>Mode</span>
      <select class="lattice-input" value={mode} on:change={(event) => onmode(event.currentTarget.value)} disabled={launching}>
        <option value="hive">Hive</option>
        <option value="fusion">Fusion</option>
        <option value="solo">Solo</option>
        <option value="research">Research</option>
        <option value="debate">Debate</option>
        <option value="templates">Templates</option>
      </select>
    </label>
    <label class="launch-chip">
      <span>Workers</span>
      <select class="lattice-input" value={workerCount} on:change={(event) => onworkers(Number(event.currentTarget.value))} disabled={launching || mode === 'solo' || mode === 'templates'}>
        {#each Array.from({ length: maxWorkers - minWorkers + 1 }, (_, index) => index + minWorkers) as count}
          <option value={count}>{count}</option>
        {/each}
      </select>
    </label>
    <label class="launch-chip preset-chip">
      <span>Preset</span>
      <select class="lattice-input" value={preset} on:change={(event) => onpreset(event.currentTarget.value)} disabled={launching}>
        <option value="custom">Custom (keep current configuration)</option>
        {#each templates as template (template.id)}
          <option value={template.id}>{template.name}</option>
        {/each}
      </select>
    </label>
    <div class="launch-chip project-chip">
      <span id="launch-project-label">Project</span>
      <button type="button" class="lattice-btn lattice-btn--secondary" on:click={onbrowse} disabled={launching} aria-labelledby="launch-project-label launch-project-value" title={projectPath || 'Select a project folder'}>
        <span id="launch-project-value">{projectPath || 'Choose folder…'}</span>
      </button>
    </div>
  </div>

  {#if templatesLoading}<p class="hint" role="status">Loading presets…</p>{/if}
  {#if templatesError}<p class="hint" role="status">Presets unavailable: {templatesError}. You can still configure a session.</p>{/if}
  {#if mode === 'templates'}<p class="hint">Choose a preset to fill in your session.</p>{/if}
  {#if error}<div class="error-message lattice-forced-colors-boundary" role="alert">{error}</div>{/if}

  <div class="composer-actions">
    <button type="button" class="lattice-btn lattice-btn--ghost" on:click={onadvanced} disabled={launching}>Advanced</button>
    <span class="hint" id="launch-submit-hint">Ctrl/Cmd+Enter to launch · Enter for a new line</span>
    <button type="button" class="lattice-btn lattice-btn--secondary" on:click={oncancel} disabled={launching}>Cancel</button>
    <button type="submit" class="lattice-btn lattice-btn--primary lattice-btn--filled" disabled={launching || !projectPath.trim() || mode === 'templates'} aria-busy={launching}>{launching ? 'Launching...' : 'Launch'}</button>
  </div>
</form>

<style>
  .launch-composer { display: flex; flex-direction: column; gap: var(--space-4); }
  .task-label { color: var(--text-primary); font-size: var(--text-base); }
  textarea { width: 100%; min-height: 10rem; resize: vertical; line-height: 1.6; }
  .launch-chips { display: flex; flex-wrap: wrap; gap: var(--space-3); align-items: end; }
  .launch-chip { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .launch-chip > span { font-size: var(--text-small); color: var(--text-secondary); }
  .launch-chip select { width: 100%; }
  .preset-chip, .project-chip { flex: 1 1 12rem; }
  .project-chip button { max-width: 100%; text-transform: none; }
  #launch-project-value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .composer-actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .composer-actions .hint { flex: 1 1 10rem; }
  .hint { margin: 0; color: var(--text-secondary); font-size: var(--text-small); }
  .error-message { color: var(--status-error); }
</style>
