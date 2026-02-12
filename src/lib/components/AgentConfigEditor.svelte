<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { AgentConfig } from '$lib/stores/sessions';

  export let config: AgentConfig;
  export let showLabel: boolean = true;

  const dispatch = createEventDispatcher<{ change: AgentConfig }>();

  interface PresetOption {
    value: string;
    label: string;
  }

  // Predefined CLI options with descriptions
  const cliOptions = [
    { value: 'claude', label: 'Claude Code', description: 'Anthropic Claude (Opus 4.6)' },
    { value: 'gemini', label: 'Gemini CLI', description: 'Google Gemini Pro' },
    { value: 'opencode', label: 'OpenCode', description: 'BigPickle, Grok, multi-model' },
    { value: 'codex', label: 'Codex', description: 'OpenAI GPT-5.3' },
    { value: 'cursor', label: 'Cursor', description: 'Cursor CLI via WSL (Opus 4.6)' },
    { value: 'droid', label: 'Droid', description: 'GLM 4.7 (Factory Droid CLI)' },
    { value: 'qwen', label: 'Qwen', description: 'Qwen Code CLI (Qwen3-Coder)' },
  ];

  const claudePresets: PresetOption[] = [
    { value: 'claude-opus-4-6-high', label: 'Opus 4.6 (High effort)' },
    { value: 'claude-opus-4-6-low', label: 'Opus 4.6 (Low effort)' },
    { value: 'claude-sonnet-4-5', label: 'Sonnet 4.5' },
  ];

  const codexPresets: PresetOption[] = [
    { value: 'codex-gpt-5-3-low', label: 'GPT-5.3 Codex (Low effort)' },
    { value: 'codex-gpt-5-3-medium', label: 'GPT-5.3 Codex (Medium effort)' },
    { value: 'codex-gpt-5-3-high', label: 'GPT-5.3 Codex (High effort)' },
    { value: 'codex-gpt-5-3-xhigh', label: 'GPT-5.3 Codex (Extra high effort)' },
  ];

  $: presetOptions = config.cli === 'claude'
    ? claudePresets
    : config.cli === 'codex'
      ? codexPresets
      : [];

  $: selectedPreset = detectPreset(config);

  $: presetDescription = config.cli === 'claude'
    ? 'Opus presets add --settings {"effortLevel":"high|low"}'
    : config.cli === 'codex'
      ? 'Adds -c model_reasoning_effort="low|medium|high|xhigh"'
      : '';

  function handleCliChange(e: Event) {
    const target = e.target as HTMLSelectElement;
    const nextCli = target.value;
    const baseFlags = stripManagedEffortFlags('codex', stripManagedEffortFlags('claude', config.flags || []));

    let model: string | undefined = undefined;
    let flags = [...baseFlags];

    if (nextCli === 'claude') {
      model = 'claude-opus-4-6';
      flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));
    } else if (nextCli === 'codex') {
      model = 'gpt-5.3-codex';
      flags.push('-c', 'model_reasoning_effort="medium"');
    }

    config = {
      ...config,
      cli: nextCli,
      model,
      flags,
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

  function parseClaudeEffort(flags: string[]): string | undefined {
    for (let i = 0; i < flags.length; i += 1) {
      if (flags[i] !== '--settings' || i + 1 >= flags.length) {
        continue;
      }

      try {
        const parsed = JSON.parse(flags[i + 1]) as { effortLevel?: string };
        if (typeof parsed.effortLevel === 'string') {
          return parsed.effortLevel.toLowerCase();
        }
      } catch {
        // Ignore non-JSON settings values
      }
    }

    return undefined;
  }

  function parseCodexEffort(flags: string[]): string | undefined {
    for (let i = 0; i < flags.length; i += 1) {
      const flag = flags[i];
      if ((flag !== '-c' && flag !== '--config') || i + 1 >= flags.length) {
        continue;
      }

      const setting = flags[i + 1].trim();
      const match = setting.match(/^model_reasoning_effort\s*=\s*(.+)$/);
      if (!match) {
        continue;
      }

      const raw = match[1].trim();
      return raw.replace(/^['"]|['"]$/g, '').toLowerCase();
    }

    return undefined;
  }

  function stripManagedEffortFlags(cli: string, flags: string[]): string[] {
    const cleaned: string[] = [];

    for (let i = 0; i < flags.length; i += 1) {
      const flag = flags[i];

      if (cli === 'claude' && flag === '--settings' && i + 1 < flags.length) {
        try {
          const parsed = JSON.parse(flags[i + 1]) as { effortLevel?: string };
          if (typeof parsed.effortLevel === 'string') {
            i += 1;
            continue;
          }
        } catch {
          // Not our managed settings payload; keep it.
        }
      }

      if (cli === 'codex' && (flag === '-c' || flag === '--config') && i + 1 < flags.length) {
        if (flags[i + 1].trim().startsWith('model_reasoning_effort=')) {
          i += 1;
          continue;
        }
      }

      cleaned.push(flag);
    }

    return cleaned;
  }

  function detectPreset(agent: AgentConfig): string {
    const flags = agent.flags || [];
    const model = (agent.model || '').toLowerCase();

    if (agent.cli === 'claude') {
      const effort = parseClaudeEffort(flags);

      if (model.includes('sonnet')) {
        return 'claude-sonnet-4-5';
      }

      if ((model.includes('opus') || model === '') && effort === 'low') {
        return 'claude-opus-4-6-low';
      }

      if ((model.includes('opus') || model === '') && effort === 'high') {
        return 'claude-opus-4-6-high';
      }

      return 'custom';
    }

    if (agent.cli === 'codex') {
      const effort = parseCodexEffort(flags);
      const isGpt53 = model.includes('gpt-5.3');

      if (isGpt53 && effort === 'low') return 'codex-gpt-5-3-low';
      if (isGpt53 && effort === 'medium') return 'codex-gpt-5-3-medium';
      if (isGpt53 && effort === 'high') return 'codex-gpt-5-3-high';
      if (isGpt53 && effort === 'xhigh') return 'codex-gpt-5-3-xhigh';

      return 'custom';
    }

    return 'custom';
  }

  function applyPreset(preset: string): void {
    if (preset === 'custom') {
      return;
    }

    const cleanedFlags = stripManagedEffortFlags('codex', stripManagedEffortFlags('claude', config.flags || []));
    let model = config.model;
    let flags = [...cleanedFlags];

    switch (preset) {
      case 'claude-opus-4-6-high':
        model = 'claude-opus-4-6';
        flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));
        break;
      case 'claude-opus-4-6-low':
        model = 'claude-opus-4-6';
        flags.push('--settings', JSON.stringify({ effortLevel: 'low' }));
        break;
      case 'claude-sonnet-4-5':
        model = 'claude-sonnet-4-5-20250929';
        break;
      case 'codex-gpt-5-3-low':
        model = 'gpt-5.3-codex';
        flags.push('-c', 'model_reasoning_effort="low"');
        break;
      case 'codex-gpt-5-3-medium':
        model = 'gpt-5.3-codex';
        flags.push('-c', 'model_reasoning_effort="medium"');
        break;
      case 'codex-gpt-5-3-high':
        model = 'gpt-5.3-codex';
        flags.push('-c', 'model_reasoning_effort="high"');
        break;
      case 'codex-gpt-5-3-xhigh':
        model = 'gpt-5.3-codex';
        flags.push('-c', 'model_reasoning_effort="xhigh"');
        break;
      default:
        return;
    }

    config = {
      ...config,
      model,
      flags,
    };
    dispatch('change', config);
  }

  function handlePresetChange(e: Event): void {
    const target = e.target as HTMLSelectElement;
    applyPreset(target.value);
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

  {#if config.cli === 'claude' || config.cli === 'codex'}
    <div class="field">
      <label for="preset">Model &amp; Effort</label>
      <select
        id="preset"
        value={selectedPreset}
        on:change={handlePresetChange}
        class="cli-select"
      >
        {#each presetOptions as preset}
          <option value={preset.value}>
            {preset.label}
          </option>
        {/each}
        <option value="custom">Custom (preserve current)</option>
      </select>
      <span class="cli-description">{presetDescription}</span>
    </div>
  {/if}
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
