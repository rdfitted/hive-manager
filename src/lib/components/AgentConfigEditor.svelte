<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { AgentConfig } from '$lib/stores/sessions';

  export let config: AgentConfig;
  export let showLabel: boolean = true;

  const dispatch = createEventDispatcher<{ change: AgentConfig }>();

  // Predefined CLI options with descriptions
  const cliOptions = [
    { value: 'claude', label: 'Claude Code', description: 'Anthropic Claude (Opus 4.5)' },
    { value: 'gemini', label: 'Gemini CLI', description: 'Google Gemini Pro' },
    { value: 'opencode', label: 'OpenCode', description: 'BigPickle, Grok, multi-model' },
    { value: 'codex', label: 'Codex', description: 'OpenAI GPT-5.2' },
    { value: 'cursor', label: 'Cursor', description: 'Cursor CLI via WSL (Opus 4.5)' },
    { value: 'droid', label: 'Droid', description: 'GLM 4.7 (Factory Droid CLI)' },
    { value: 'qwen', label: 'Qwen', description: 'Qwen Code CLI (Qwen3-Coder)' },
  ];

  function handleCliChange(e: Event) {
    const target = e.target as HTMLSelectElement;
    config = {
      ...config,
      cli: target.value,
    };
    dispatch('change', config);
  }

  function handleLabelChange(e: Event) {
    const target = e.target as HTMLInputElement;
    config = {
      ...config,
      label: target.value || undefined,
    };
    dispatch('change', config);
  }
</script>

<div class="config-editor">
  {#if showLabel}
    <div class="field">
      <label for="label">Label</label>
      <input
        id="label"
        type="text"
        placeholder="Optional display name"
        value={config.label || ''}
        on:input={handleLabelChange}
      />
    </div>
  {/if}

  <div class="field">
    <label for="cli">CLI</label>
    <select
      id="cli"
      value={config.cli}
      on:change={handleCliChange}
      class="cli-select"
    >
      {#each cliOptions as cli}
        <option value={cli.value} title={cli.description}>
          {cli.label}
        </option>
      {/each}
    </select>
    <span class="cli-description">
      {cliOptions.find(c => c.value === config.cli)?.description || ''}
    </span>
  </div>
</div>

<style>
  .config-editor {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  label {
    font-size: 12px;
    font-weight: 500;
    color: var(--color-text-muted);
  }

  input,
  .cli-select {
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    background: var(--color-bg);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    color: var(--color-text);
  }

  .cli-select {
    cursor: pointer;
    appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'%3E%3Cpath fill='%239ca3af' d='M3 4.5L6 7.5L9 4.5'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 10px center;
    padding-right: 30px;
  }

  .cli-description {
    font-size: 11px;
    color: var(--color-text-muted);
    opacity: 0.7;
  }

  input::placeholder {
    color: var(--color-text-muted);
    opacity: 0.6;
  }

  input:focus,
  .cli-select:focus {
    outline: none;
    border-color: var(--color-primary, #8b5cf6);
  }
</style>
