<script lang="ts">
  import type { SessionTemplate } from '$lib/types/domain';

  export let mode = 'hive';
  export let codexAppServer = false;
  export let claudeStreamJson = false;
  export let oncodextransport: (enabled: boolean) => void = () => {};
  export let onclaudetransport: (enabled: boolean) => void = () => {};
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
  export let oncancel: (() => void) | undefined = undefined;
  export let showQa = false;
  export let qaEnabled = true;
  export let qaSummary = '';
  export let qaCaveat = '';
  export let onqa: (on: boolean) => void = () => {};
  export let showSmoke = false;
  export let onsmoke: () => void = () => {};

  let editor: HTMLTextAreaElement;

  function focusCard(event: PointerEvent) {
    if (!launching && event.target === event.currentTarget) {
      event.preventDefault();
      editor.focus();
    }
  }

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
  <div class="composer-input lattice-forced-colors-boundary" role="group" aria-label="Launch composer" on:pointerdown={focusCard}>
    <textarea
      bind:this={editor}
      id="launch-task"
      class="lattice-input lattice-scroll-chrome lattice-forced-colors-boundary"
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
      {#if mode === 'hive' || mode === 'solo'}
        <label class="launch-chip">
          <span>Codex transport</span>
          <select class="lattice-input" value={codexAppServer ? 'protocol' : 'pty'} on:change={(event) => oncodextransport(event.currentTarget.value === 'protocol')} disabled={launching}>
            <option value="pty">PTY</option>
            <option value="protocol">App server (experimental)</option>
          </select>
        </label>
        <label class="launch-chip">
          <span>Claude transport</span>
          <select class="lattice-input" value={claudeStreamJson ? 'protocol' : 'pty'} on:change={(event) => onclaudetransport(event.currentTarget.value === 'protocol')} disabled={launching}>
            <option value="pty">PTY</option>
            <option value="protocol">Stream JSON (experimental)</option>
          </select>
        </label>
      {/if}
      {#if showQa}
        <button type="button" class="lattice-btn lattice-btn--secondary qa-chip" aria-pressed={qaEnabled} aria-describedby={qaCaveat ? 'launch-qa-caveat' : undefined} on:click={() => onqa(!qaEnabled)} disabled={launching}>{qaSummary}</button>
      {/if}
    </div>

    {#if showQa && qaCaveat}<p class="hint" id="launch-qa-caveat">{qaCaveat}</p>{/if}

    {#if templatesLoading}<p class="hint" role="status">Loading presets…</p>{/if}
    {#if templatesError}<p class="hint" role="status">Presets unavailable: {templatesError}. You can still configure a session.</p>{/if}
    {#if mode === 'templates'}<p class="hint">Choose a preset to fill in your session.</p>{/if}
    {#if error}<div class="error-message lattice-forced-colors-boundary" role="alert">{error}</div>{/if}

    <div class="composer-actions">
      <button type="button" class="lattice-btn lattice-btn--ghost" on:click={onadvanced} disabled={launching}>Advanced</button>
      <span class="hint" id="launch-submit-hint">Ctrl/Cmd+Enter to launch · Enter for a new line</span>
      {#if showSmoke}<button type="button" class="lattice-btn lattice-btn--secondary" on:click={onsmoke} disabled={launching || !projectPath.trim() || mode === 'templates'}>Smoke Test</button>{/if}
      {#if oncancel}<button type="button" class="lattice-btn lattice-btn--secondary" on:click={oncancel} disabled={launching}>Cancel</button>{/if}
      <button type="submit" class="lattice-btn composer-send" disabled={launching || !projectPath.trim() || mode === 'templates'} aria-busy={launching}>{launching ? 'Launching...' : 'Launch'}</button>
    </div>
  </div>
</form>

<style>
  .launch-composer { display: flex; flex-direction: column; gap: var(--space-4); }
  .task-label { color: var(--text-primary); font-size: var(--text-base); }
  .composer-input { display: flex; flex-direction: column; gap: var(--space-3); min-width: 0; padding: var(--space-3); border: 1px solid var(--border-control); border-radius: var(--radius-shell); background: var(--bg-void); }
  .composer-input:focus-within { border-color: var(--accent-cyan); }
  textarea { display: block; box-sizing: border-box; width: 100%; min-height: 12rem; resize: vertical; line-height: 1.6; padding: 0; border: 0; border-radius: 0; background: transparent; }
  textarea:focus, textarea:focus-visible { border: 0; outline: none; box-shadow: none; }
  .composer-send { flex-shrink: 0; min-height: 32px; border: 1px solid transparent; border-radius: var(--radius-full); background: var(--text-primary); color: var(--bg-void); }
  .composer-send:hover:enabled { background: var(--text-secondary); }
  .launch-chips { display: flex; flex-wrap: wrap; gap: var(--space-3); align-items: end; min-width: 0; }
  .launch-chip { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .launch-chip > span { font-size: var(--text-small); color: var(--text-secondary); }
  .launch-chip select { width: 100%; }
  .preset-chip, .project-chip { flex: 1 1 12rem; }
  .project-chip button { max-width: 100%; text-transform: none; }
  .qa-chip { max-width: 100%; white-space: normal; text-align: left; text-transform: none; }
  #launch-project-value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .composer-actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .composer-actions .hint { flex: 1 1 10rem; }
  .hint { margin: 0; color: var(--text-secondary); font-size: var(--text-small); }
  .error-message { color: var(--status-error); }
  @media (forced-colors: active) {
    .composer-input { border-color: CanvasText; }
    .composer-input:focus-within { border-color: Highlight; }
    textarea:focus-visible { outline: 2px solid Highlight; outline-offset: 2px; }
    .composer-send { border-color: ButtonText; background: ButtonFace; color: ButtonText; }
    .composer-send:disabled { color: GrayText; opacity: 1; }
  }
</style>
