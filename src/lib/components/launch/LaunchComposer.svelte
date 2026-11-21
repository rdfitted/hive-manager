<script lang="ts">
  import type { SessionTemplate } from '$lib/types/domain';
  import { FolderSimple, SlidersHorizontal } from 'phosphor-svelte';
  import { Glyph, IconButton } from '$lib/components/ui';

  export let landing = false;
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
  const modes = [
    { value: 'hive', label: 'Hive' },
    { value: 'fusion', label: 'Fusion' },
    { value: 'solo', label: 'Solo' },
    { value: 'research', label: 'Research' },
    { value: 'debate', label: 'Debate' },
    { value: 'templates', label: 'Templates' },
  ];

  export function focusTask() {
    if (!launching) editor?.focus();
  }

  function focusCard(event: PointerEvent) {
    if (!launching && event.target === event.currentTarget) {
      event.preventDefault();
      focusTask();
    }
  }

  $: projectName = projectPath.trim().replace(/[\\/]+$/, '').split(/[\\/]/).pop() || 'Choose folder';
  $: minWorkers = mode === 'fusion' || mode === 'debate' ? Math.min(2, workerCount) : 1;

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.isComposing) {
      event.preventDefault();
      if (!launching && projectPath.trim() && mode !== 'templates') onlaunch();
    }
  }
</script>

<form class="launch-composer" class:landing on:submit|preventDefault={onlaunch}>
  <label class="task-label" for="launch-task">What would you like to work on?</label>
  <div class="composer-input lattice-forced-colors-boundary" role="group" aria-label="Launch composer" on:pointerdown={focusCard}>
    <textarea
      bind:this={editor}
      id="launch-task"
      class="lattice-input lattice-scroll-chrome lattice-forced-colors-boundary"
      {value}
      on:input={(event) => onvalue(event.currentTarget.value)}
      rows={landing ? 3 : 6}
      placeholder="Describe a task for your session…"
      disabled={launching}
      aria-describedby="launch-submit-hint"
      on:keydown={handleKeydown}
    ></textarea>

    <div class="launch-chips">
      {#if landing}
        <fieldset class="mode-pills" aria-label="Launch mode" disabled={launching}>
          {#each modes as choice (choice.value)}
            <label class="mode-pill">
              <input type="radio" name="launch-mode" value={choice.value} checked={mode === choice.value} on:change={() => onmode(choice.value)} />
              <span class="lattice-forced-colors-boundary">{choice.label}</span>
            </label>
          {/each}
        </fieldset>
      {/if}
      <label class="launch-chip" class:compact-mode={landing}>
        <span>Mode</span>
        <select class="lattice-input" value={mode} on:change={(event) => onmode(event.currentTarget.value)} disabled={launching}>
          {#each modes as choice (choice.value)}
            <option value={choice.value}>{choice.label}</option>
          {/each}
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
      {#if !landing}
        <div class="launch-chip project-chip">
          <span id="launch-project-label">Project</span>
          <button type="button" class="lattice-btn lattice-btn--secondary" on:click={onbrowse} disabled={launching} aria-labelledby="launch-project-label launch-project-value" title={projectPath || 'Select a project folder'}>
            <span id="launch-project-value">{projectPath || 'Choose folder…'}</span>
          </button>
        </div>
      {/if}
      {#if !landing && (mode === 'hive' || mode === 'solo')}
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
        <button type="button" class="lattice-btn lattice-btn--secondary qa-chip" aria-pressed={qaEnabled} aria-describedby={qaCaveat ? 'launch-qa-caveat' : undefined} title={landing && qaCaveat ? qaCaveat : undefined} on:click={() => onqa(!qaEnabled)} disabled={launching}>{qaSummary}</button>
      {/if}
    </div>

    {#if showQa && qaCaveat}<p class="hint quiet-hint" id="launch-qa-caveat">{qaCaveat}</p>{/if}

    {#if templatesLoading}<p class="hint" role="status">Loading presets…</p>{/if}
    {#if templatesError}<p class="hint" role="status">Presets unavailable: {templatesError}. You can still configure a session.</p>{/if}
    {#if mode === 'templates'}<p class="hint">Choose a preset to fill in your session.</p>{/if}
    {#if error}<div class="error-message lattice-forced-colors-boundary" role="alert">{error}</div>{/if}

    <div class="composer-actions">
      {#if landing}
        <button type="button" class="lattice-btn lattice-btn--ghost project-pill" on:click={onbrowse} disabled={launching}
          aria-label={projectPath ? `Project ${projectPath}` : 'Project'} title={projectPath || 'Select a project folder'}>
          <FolderSimple size={14} aria-hidden="true" /><span>{projectName}</span>
        </button>
        <IconButton
          type="button"
          icon={SlidersHorizontal}
          size="sm"
          label="Advanced"
          tooltip="Advanced: roles, models, transports"
          onclick={onadvanced}
          disabled={launching}
        />
      {:else}
        <button type="button" class="lattice-btn lattice-btn--ghost" on:click={onadvanced} disabled={launching}>Advanced</button>
      {/if}
      <span class="hint quiet-hint" id="launch-submit-hint">Ctrl/Cmd+Enter to launch · Enter for a new line</span>
      {#if showSmoke}<button type="button" class="lattice-btn lattice-btn--secondary smoke" on:click={onsmoke} disabled={launching || !projectPath.trim() || mode === 'templates'}>Smoke Test</button>{/if}
      {#if oncancel}<button type="button" class="lattice-btn lattice-btn--secondary" on:click={oncancel} disabled={launching}>Cancel</button>{/if}
      <button type="submit" class="lattice-btn composer-send" title="Launch (Ctrl/Cmd+Enter)" disabled={launching || !projectPath.trim() || mode === 'templates'} aria-busy={launching}>
        {#if landing}<Glyph name="send" size={14} /><span class="sr-only">{launching ? 'Launching...' : 'Launch'}</span>{:else}{launching ? 'Launching...' : 'Launch'}{/if}
      </button>
    </div>
  </div>
</form>

<style>
  .launch-composer { display: flex; flex-direction: column; gap: var(--space-4); }
  .landing { container: launch-composer / inline-size; gap: 0; }
  .task-label { color: var(--text-primary); font-size: var(--text-base); }
  /* On the landing the question reads as the line under the lockup. */
  .landing .task-label { margin: 0 0 26px; color: var(--ink-muted); font: italic 19px/1.4 var(--font-display); text-align: center; }
  .composer-input { display: flex; flex-direction: column; gap: var(--space-3); min-width: 0; padding: var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-shell); background: var(--input); }
  .composer-input:focus-within { border-color: var(--accent); }
  .landing .composer-input { gap: 10px; padding: 16px 16px 12px; border-color: var(--line); border-radius: var(--radius-glass); background: var(--sheen), var(--surface); box-shadow: var(--glass-shadow); transition: border-color var(--motion-ui); }
  .landing .composer-input:focus-within { border-color: var(--accent); }
  @supports ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
    .landing .composer-input { background: var(--sheen), var(--glass); -webkit-backdrop-filter: var(--glass-blur); backdrop-filter: var(--glass-blur); }
  }
  textarea { display: block; box-sizing: border-box; width: 100%; min-height: 12rem; resize: vertical; line-height: 1.6; padding: 0; border: 0; border-radius: 0; background: transparent; }
  .landing textarea { min-height: 4.75rem; color: var(--ink); font-size: 15px; line-height: 1.5; }
  .landing textarea::placeholder { color: var(--ink-dim); }
  textarea:focus { border: 0; outline: none; box-shadow: none; }
  textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .landing textarea:focus-visible { outline: none; }
  .composer-send { flex-shrink: 0; min-height: 32px; border: 1px solid transparent; border-radius: var(--control-radius); background: var(--accent); color: var(--on-accent); }
  .composer-send:hover:enabled { background: var(--accent); }
  .landing .composer-send { width: 28px; min-height: 28px; height: 28px; padding: 0; border-radius: var(--radius-full); color: var(--accent-ink); }
  .launch-chips { display: flex; flex-wrap: wrap; gap: var(--space-3); align-items: end; min-width: 0; }
  .launch-chip { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .launch-chip > span { font-size: var(--text-small); color: var(--ink-muted); }
  .launch-chip select { width: 100%; }
  .compact-mode { display: none; }
  .mode-pills { display: flex; flex: 1 0 100%; flex-wrap: wrap; gap: var(--space-2); min-width: 0; margin: 0; padding: 0; border: 0; }
  .mode-pill { position: relative; }
  .mode-pill input { position: absolute; opacity: 0; width: 1px; height: 1px; }
  /* Mode pills carry no ring at rest; the chosen one takes the strong glass. */
  .mode-pill span {
    display: flex;
    align-items: center;
    height: 28px;
    padding-inline: 13px;
    border: 1px solid transparent;
    border-radius: 14px;
    color: var(--ink-muted);
    font-size: 12.5px;
    cursor: pointer;
    transition: background-color var(--motion-ui), color var(--motion-ui), border-color var(--motion-ui);
  }
  .mode-pill:hover span { color: var(--ink); }
  .mode-pill input:checked + span { background: var(--glass-strong); color: var(--ink); border-color: var(--line); font-weight: 600; }
  .mode-pill input:focus-visible + span { outline: 2px solid var(--focus); outline-offset: 2px; }
  .mode-pill input:disabled + span { opacity: .55; cursor: default; }
  /* Landing: modes lead under a hairline; the setup reads as one quiet line. */
  .landing .launch-chips { align-items: center; gap: 6px 14px; padding-top: 10px; border-top: 1px solid var(--line); }
  .landing .mode-pills { gap: 6px; margin-left: -4px; }
  .landing .launch-chip { flex-direction: row; align-items: center; gap: 2px; }
  .landing .launch-chip > span { color: var(--ink-dim); font-size: 11.5px; }
  .landing .launch-chip select { height: 26px; min-height: 26px; padding: 0 20px 0 6px; border-color: transparent; border-radius: 13px; background-color: transparent; color: var(--ink-muted); font-size: 11.5px; }
  .landing .launch-chip select:hover:enabled { border-color: var(--line-strong); color: var(--ink); }
  .landing .preset-chip { flex: 0 1 15rem; }
  .landing .qa-chip { min-height: 26px; padding: 0 10px; border-radius: 13px; --lattice-btn-ring: var(--line-strong); background: transparent; color: var(--ink-muted); font-size: 11.5px; }
  .landing .qa-chip[aria-pressed='true'] { --lattice-btn-ring: transparent; background: var(--accent-bg); color: var(--accent); }
  @container launch-composer (max-width: 26rem) {
    .mode-pills { display: none; }
    .compact-mode { display: flex; }
  }
  .preset-chip, .project-chip { flex: 1 1 12rem; }
  .project-chip button { max-width: 100%; text-transform: none; }
  .qa-chip { max-width: 100%; white-space: normal; text-align: left; text-transform: none; }
  #launch-project-value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .composer-actions { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .composer-actions .hint { flex: 1 1 10rem; }
  .landing .composer-actions { justify-content: flex-end; gap: 6px; margin-top: -2px; }
  .project-pill { min-width: 0; max-width: 16rem; min-height: 28px; height: 28px; margin-right: auto; margin-left: -6px; padding: 0 12px 0 10px; gap: 7px; border-radius: 14px; color: var(--ink-muted); font-size: 12.5px; }
  .project-pill span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .landing .smoke { min-height: 28px; padding: 0 12px; border-radius: 14px; --lattice-btn-ring: var(--line-strong); background: transparent; font-size: 12.5px; }
  .hint { margin: 0; color: var(--ink-muted); font-size: var(--text-small); }
  /* Kept for assistive technology and the control tooltips, off the landing surface. */
  .landing .quiet-hint, .sr-only { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .error-message { color: var(--status-error); }
  @media (forced-colors: active) {
    .composer-input { border-color: CanvasText; }
    .composer-input:focus-within { border-color: Highlight; }
    .landing .composer-input { background: Canvas; -webkit-backdrop-filter: none; backdrop-filter: none; box-shadow: none; }
    textarea:focus-visible { outline: 2px solid Highlight; outline-offset: 2px; }
    .composer-send { border-color: ButtonText; background: ButtonFace; color: ButtonText; }
    .composer-send:disabled { color: GrayText; opacity: 1; }
    .mode-pill span.lattice-forced-colors-boundary { border-color: ButtonText !important; }
    .mode-pill input:checked + span.lattice-forced-colors-boundary { border-color: Highlight !important; color: Highlight; }
    .mode-pill input:focus-visible + span { outline-color: Highlight; }
    .mode-pill input:disabled + span.lattice-forced-colors-boundary { border-color: GrayText !important; color: GrayText; opacity: 1; }
  }
</style>
