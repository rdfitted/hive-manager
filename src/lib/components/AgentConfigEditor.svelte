<script module lang="ts">
  import { apiUrl } from '$lib/config';

  export type CliLoginStatus = 'yes' | 'no' | 'unknown';

  export interface CliHealthEntry {
    cli: string;
    resolved: boolean;
    binPath: string | null;
    loggedIn: CliLoginStatus;
    detail: string;
    staleHint: boolean;
  }

  export type CliHealthMap = Record<string, CliHealthEntry>;
  export type CliHealthTone = 'healthy' | 'warning' | 'error' | 'pending';

  const CLI_HEALTH_CACHE_MS = 30_000;
  let cachedCliHealth: CliHealthMap | null = null;
  let cachedCliHealthAt = 0;
  let cliHealthRequest: Promise<CliHealthMap> | null = null;

  export function normalizeCliHealth(payload: unknown): CliHealthMap {
    if (typeof payload !== 'object' || payload === null) return {};
    const rawClis = (payload as { clis?: unknown }).clis;
    const entries: Array<[string | undefined, unknown]> = Array.isArray(rawClis)
      ? rawClis.map((entry): [string | undefined, unknown] => [undefined, entry])
      : typeof rawClis === 'object' && rawClis !== null
        ? Object.entries(rawClis as Record<string, unknown>)
        : [];
    const normalized: CliHealthMap = {};

    for (const [key, value] of entries) {
      if (typeof value !== 'object' || value === null) continue;
      const candidate = value as Record<string, unknown>;
      const cli = typeof candidate.cli === 'string'
        ? candidate.cli
        : typeof candidate.name === 'string'
          ? candidate.name
          : key;
      if (!cli || typeof candidate.resolved !== 'boolean') continue;
      const loggedIn: CliLoginStatus = candidate.loggedIn === 'yes' || candidate.loggedIn === 'no'
        ? candidate.loggedIn
        : 'unknown';

      normalized[cli] = {
        cli,
        resolved: candidate.resolved,
        binPath: typeof candidate.binPath === 'string' ? candidate.binPath : null,
        loggedIn,
        detail: typeof candidate.detail === 'string' ? candidate.detail : '',
        staleHint: candidate.staleHint === true,
      };
    }

    return normalized;
  }

  export async function fetchCliHealth(force = false): Promise<CliHealthMap> {
    if (
      !force &&
      cachedCliHealth &&
      Date.now() - cachedCliHealthAt < CLI_HEALTH_CACHE_MS
    ) {
      return cachedCliHealth;
    }
    if (cliHealthRequest) return cliHealthRequest;

    cliHealthRequest = (async () => {
      const response = await fetch(apiUrl('/api/cli-health'));
      if (!response.ok) throw new Error(`CLI health request failed (${response.status})`);
      const health = normalizeCliHealth(await response.json());
      if (Object.keys(health).length === 0) throw new Error('CLI health response was empty');
      cachedCliHealth = health;
      cachedCliHealthAt = Date.now();
      return health;
    })();

    try {
      return await cliHealthRequest;
    } finally {
      cliHealthRequest = null;
    }
  }

  export function cliHealthTone(
    health: CliHealthEntry | undefined,
    error: string | null = null,
  ): CliHealthTone {
    if (!health) return error ? 'warning' : 'pending';
    if (!health.resolved) return health.staleHint ? 'warning' : 'error';
    if (health.loggedIn === 'no') return 'error';
    if (health.loggedIn === 'unknown') return 'warning';
    return 'healthy';
  }

  export function cliHealthLabel(
    health: CliHealthEntry | undefined,
    loading = false,
    error: string | null = null,
  ): string {
    if (!health) {
      if (loading) return 'Checking…';
      if (error) return 'Health unavailable';
      return 'Not checked';
    }
    if (!health.resolved) return health.staleHint ? 'Not on current PATH' : 'Not installed';
    if (health.loggedIn === 'no') return 'Login required';
    if (health.loggedIn === 'unknown') return 'Auth unknown';
    return 'Ready';
  }

  export function cliHealthMessage(
    health: CliHealthEntry | undefined,
    loading = false,
    error: string | null = null,
  ): string {
    if (!health) {
      if (loading) return 'Checking whether this CLI can launch on this machine.';
      if (error) return error;
      return 'CLI health has not been checked yet.';
    }
    if (!health.resolved && health.staleHint) {
      const detail = health.detail ? `${health.detail} ` : 'The executable is missing from the current PATH. ';
      return `${detail}Restarting Hive Manager after updating PATH may help.`;
    }
    if (!health.resolved) return health.detail || 'The executable is not installed or cannot be launched.';
    if (health.loggedIn === 'no') return health.detail || 'The CLI is installed but needs authentication.';
    if (health.loggedIn === 'unknown') {
      return health.detail || 'The CLI is installed, but authentication cannot be verified automatically.';
    }
    return health.detail || 'The CLI is installed and authenticated.';
  }

  export function cliHealthTitle(
    health: CliHealthEntry | undefined,
    loading = false,
    error: string | null = null,
  ): string {
    const message = cliHealthMessage(health, loading, error);
    return health?.binPath ? `${message} Executable: ${health.binPath}` : message;
  }
</script>

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { AgentConfig } from '$lib/stores/sessions';
  import { cliOptions, getDefaultModel, normalizeModelId } from '$lib/config/clis';

  export let config: AgentConfig;
  export let showLabel: boolean = true;
  export let idPrefix: string;
  export let cliHealth: CliHealthMap = {};
  export let cliHealthLoading: boolean = false;
  export let cliHealthError: string | null = null;

  const dispatch = createEventDispatcher<{ change: AgentConfig }>();

  interface PresetOption {
    value: string;
    label: string;
  }

  const claudePresets: PresetOption[] = [
    { value: 'fable-high', label: 'Fable 5 (High effort)' },
    { value: 'fable-max', label: 'Fable 5 (Max effort)' },
    { value: 'fable', label: 'Fable 5' },
    { value: 'opus-high', label: 'Opus (High effort)' },
    { value: 'opus-low', label: 'Opus (Low effort)' },
    { value: 'opus', label: 'Opus' },
    { value: 'claude-opus-4-6-high', label: 'Opus 4.6 (High effort)' },
    { value: 'claude-opus-4-6-low', label: 'Opus 4.6 (Low effort)' },
    { value: 'claude-opus-4-5', label: 'Opus 4.5' },
    { value: 'claude-sonnet-4-6', label: 'Sonnet 4.6' },
    { value: 'claude-sonnet-4-5', label: 'Sonnet 4.5' },
    { value: 'claude-haiku-4-5', label: 'Haiku 4.5' },
  ];

  const codexPresets: PresetOption[] = [
    { value: 'codex-gpt-5-6-sol', label: 'GPT-5.6 Sol' },
    { value: 'codex-gpt-5-6-sol-low', label: 'GPT-5.6 Sol (Low effort)' },
    { value: 'codex-gpt-5-6-sol-medium', label: 'GPT-5.6 Sol (Medium effort)' },
    { value: 'codex-gpt-5-6-sol-high', label: 'GPT-5.6 Sol (High effort)' },
    { value: 'codex-gpt-5-6-sol-xhigh', label: 'GPT-5.6 Sol (Extra high effort)' },
    { value: 'codex-gpt-5-6-sol-max', label: 'GPT-5.6 Sol (Max effort)' },
    { value: 'codex-gpt-5-6-sol-ultra', label: 'GPT-5.6 Sol (Ultra effort)' },
    { value: 'codex-gpt-5-5-low', label: 'GPT-5.5 (Low effort)' },
    { value: 'codex-gpt-5-5-medium', label: 'GPT-5.5 (Medium effort)' },
    { value: 'codex-gpt-5-5-high', label: 'GPT-5.5 (High effort)' },
    { value: 'codex-gpt-5-5-xhigh', label: 'GPT-5.5 (Extra high effort)' },
    { value: 'codex-gpt-5-4-low', label: 'GPT-5.4 (Low effort)' },
    { value: 'codex-gpt-5-4-medium', label: 'GPT-5.4 (Medium effort)' },
    { value: 'codex-gpt-5-4-high', label: 'GPT-5.4 (High effort)' },
    { value: 'codex-gpt-5-4-xhigh', label: 'GPT-5.4 (Extra high effort)' },
    { value: 'codex-gpt-5-3-low', label: 'GPT-5.3 Codex (Low effort)' },
    { value: 'codex-gpt-5-3-medium', label: 'GPT-5.3 Codex (Medium effort)' },
    { value: 'codex-gpt-5-3-high', label: 'GPT-5.3 Codex (High effort)' },
    { value: 'codex-gpt-5-3-xhigh', label: 'GPT-5.3 Codex (Extra high effort)' },
  ];

  const geminiPresets: PresetOption[] = [
    { value: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro Preview' },
    { value: 'gemini-3-pro-preview', label: 'Gemini 3.0 Pro Preview' },
    { value: 'gemini-3-flash-preview', label: 'Gemini 3.0 Flash Preview' },
    { value: 'gemini-2.5-pro', label: 'Gemini 2.5 Pro' },
    { value: 'gemini-2.5-flash', label: 'Gemini 2.5 Flash' },
    { value: 'gemini-2.5-flash-lite', label: 'Gemini 2.5 Flash Lite' },
  ];

  // Antigravity (agy) has no model-selection flag. Model is read from
  // ~/.gemini/antigravity-cli/settings.json globally. We do not expose a
  // preset dropdown; the UI shows a static note instead.
  const antigravityPresets: PresetOption[] = [];

  const cursorPresets: PresetOption[] = [
    { value: 'composer-2.5', label: 'Composer 2.5 (latest)' },
    { value: 'composer-2', label: 'Composer 2.0' },
    { value: 'composer-2-fast', label: 'Composer 2.0 Fast' },
    { value: 'composer-1', label: 'Composer 1' },
  ];

  const droidPresets: PresetOption[] = [
    { value: 'glm-5.1', label: 'GLM 5.1' },
    { value: 'glm-4.7', label: 'GLM 4.7' },
  ];

  const opencodePresets: PresetOption[] = [
    { value: 'opencode/big-pickle', label: 'BigPickle' },
    { value: 'opencode/grok', label: 'Grok' },
  ];

  const qwenPresets: PresetOption[] = [
    { value: 'qwen3-coder', label: 'Qwen3 Coder' },
    { value: 'qwen2.5-coder', label: 'Qwen2.5 Coder' },
  ];

  const presetsByCliType: Record<string, PresetOption[]> = {
    claude: claudePresets,
    codex: codexPresets,
    gemini: geminiPresets,
    antigravity: antigravityPresets,
    cursor: cursorPresets,
    droid: droidPresets,
    opencode: opencodePresets,
    qwen: qwenPresets,
  };

  $: presetOptions = presetsByCliType[config.cli] ?? [];

  function configuredEffort(value: AgentConfig): string | undefined {
    const flags = value.flags || [];
    if (value.cli === 'codex') {
      for (let i = 0; i < flags.length - 1; i += 1) {
        if (flags[i] === '-c' || flags[i] === '--config') {
          const match = flags[i + 1].match(/^model_reasoning_effort=["']?([^"']+)["']?$/);
          if (match) return match[1];
        }
      }
    }

    if (value.cli === 'claude') {
      for (let i = 0; i < flags.length - 1; i += 1) {
        if (flags[i] !== '--settings') continue;
        try {
          const settings = JSON.parse(flags[i + 1]) as { effortLevel?: string };
          if (settings.effortLevel) return settings.effortLevel;
        } catch {
          // Preserve custom settings; they simply cannot map to a preset.
        }
      }
    }

    return undefined;
  }

  function inferSelectedPreset(value: AgentConfig): string {
    const effort = configuredEffort(value);
    if (value.cli === 'codex' && value.model && effort) {
      const model = normalizeModelId(value.cli, value.model);
      const candidate = `codex-${model.replaceAll('.', '-')}-${effort}`;
      if (codexPresets.some((preset) => preset.value === candidate)) return candidate;
    }
    if (value.cli === 'codex' && value.model && !effort) {
      const model = normalizeModelId(value.cli, value.model);
      const candidate = `codex-${model.replaceAll('.', '-')}`;
      if (codexPresets.some((preset) => preset.value === candidate)) return candidate;
    }
    if (value.cli === 'claude' && value.model === 'opus') {
      if (effort === 'high' || effort === 'low') return `opus-${effort}`;
      return 'opus';
    }
    if (value.cli === 'claude' && value.model === 'fable') {
      if (effort === 'high' || effort === 'max') return `fable-${effort}`;
      return 'fable';
    }
    if (value.model && presetOptions.some((preset) => preset.value === value.model)) {
      return value.model;
    }
    return 'custom';
  }

  $: selectedPreset = inferSelectedPreset(config);
  $: effectiveModel = config.model
    ? normalizeModelId(config.cli, config.model)
    : getDefaultModel(config.cli) || 'CLI default';
  $: effectiveEffort = configuredEffort(config);
  $: selectedCliHealth = cliHealth[config.cli];

  $: presetDescription = config.cli === 'claude'
    ? 'Claude effort presets add --settings {"effortLevel":"low|high|max"}'
    : config.cli === 'codex'
      ? 'Adds -c model_reasoning_effort="low|medium|high|xhigh|max|ultra"'
      : config.cli === 'gemini'
        ? 'Gemini model IDs for `gemini -m`'
        : config.cli === 'cursor'
          ? 'Cursor Composer mode selection'
          : config.cli === 'droid'
            ? 'Droid GLM model selection'
            : config.cli === 'opencode'
              ? 'OpenCode multi-model selection'
              : config.cli === 'qwen'
                ? 'Qwen Code CLI model selection'
                : '';

  function handleCliChange(e: Event) {
    const target = e.target as HTMLSelectElement;
    const nextCli = target.value;
    const baseFlags = stripManagedEffortFlags('codex', stripManagedEffortFlags('claude', config.flags || []));

    let model: string | undefined = getDefaultModel(nextCli) || undefined;
    let flags = [...baseFlags];

    if (nextCli === 'claude') {
      flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));
    } else if (nextCli === 'codex') {
      flags.push('-c', 'model_reasoning_effort="medium"');
    } else if (nextCli === 'gemini') {
      // Aligns with clis.ts defaultModel, Rust storage::default_config, and
      // CliRegistry::default_model("gemini") — single source of truth.
      model = getDefaultModel('gemini');
    } else if (nextCli === 'antigravity') {
      // agy has no --model flag; model is set globally in settings.json.
      model = undefined;
    } else if (nextCli === 'droid') {
      model = getDefaultModel('droid');
    } else if (nextCli === 'cursor') {
      model = getDefaultModel('cursor');
    } else if (nextCli === 'opencode') {
      model = getDefaultModel('opencode');
    } else if (nextCli === 'qwen') {
      model = getDefaultModel('qwen');
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

  function stripManagedEffortFlags(cli: string, flags: string[]): string[] {
    const cleaned: string[] = [];

    for (let i = 0; i < flags.length; i += 1) {
      const flag = flags[i];

      if (cli === 'claude' && flag === '--settings' && i + 1 < flags.length) {
        try {
          const parsed = JSON.parse(flags[i + 1]) as { effortLevel?: string; [key: string]: unknown };
          if (typeof parsed.effortLevel === 'string') {
            // Strip effortLevel but preserve other keys
            const { effortLevel: _, ...rest } = parsed;
            if (Object.keys(rest).length > 0) {
              // Other keys exist, keep --settings with remaining keys
              cleaned.push(flag);
              cleaned.push(JSON.stringify(rest));
            }
            // Skip the original --settings pair regardless
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

  function applyPreset(preset: string): void {
    if (preset === 'custom') {
      return;
    }

    const cleanedFlags = stripManagedEffortFlags('codex', stripManagedEffortFlags('claude', config.flags || []));
    let model = config.model;
    let flags = [...cleanedFlags];

    switch (preset) {
      case 'opus-high':
        model = 'opus';
        flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));
        break;
      case 'opus-low':
        model = 'opus';
        flags.push('--settings', JSON.stringify({ effortLevel: 'low' }));
        break;
      case 'opus':
        model = 'opus';
        break;
      case 'claude-opus-4-6-high':
        model = 'claude-opus-4-6';
        flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));
        break;
      case 'claude-opus-4-6-low':
        model = 'claude-opus-4-6';
        flags.push('--settings', JSON.stringify({ effortLevel: 'low' }));
        break;
      case 'claude-opus-4-5':
        model = 'claude-opus-4-5';
        break;
      case 'claude-sonnet-4-6':
        model = 'claude-sonnet-4-6';
        break;
      case 'claude-sonnet-4-5':
        model = 'claude-sonnet-4-5-20250929';
        break;
      case 'claude-haiku-4-5':
        model = 'claude-haiku-4-5';
        break;
      case 'fable-high':
        model = 'fable';
        flags.push('--settings', JSON.stringify({ effortLevel: 'high' }));
        break;
      case 'fable-max':
        model = 'fable';
        flags.push('--settings', JSON.stringify({ effortLevel: 'max' }));
        break;
      case 'fable':
        model = 'fable';
        break;
      case 'codex-gpt-5-6-sol-low':
        model = 'gpt-5.6-sol';
        flags.push('-c', 'model_reasoning_effort="low"');
        break;
      case 'codex-gpt-5-6-sol-medium':
        model = 'gpt-5.6-sol';
        flags.push('-c', 'model_reasoning_effort="medium"');
        break;
      case 'codex-gpt-5-6-sol-high':
        model = 'gpt-5.6-sol';
        flags.push('-c', 'model_reasoning_effort="high"');
        break;
      case 'codex-gpt-5-6-sol-xhigh':
        model = 'gpt-5.6-sol';
        flags.push('-c', 'model_reasoning_effort="xhigh"');
        break;
      case 'codex-gpt-5-6-sol-max':
        model = 'gpt-5.6-sol';
        flags.push('-c', 'model_reasoning_effort="max"');
        break;
      case 'codex-gpt-5-6-sol-ultra':
        model = 'gpt-5.6-sol';
        flags.push('-c', 'model_reasoning_effort="ultra"');
        break;
      case 'codex-gpt-5-6-sol':
        model = 'gpt-5.6-sol';
        break;
      case 'codex-gpt-5-5-low':
        model = 'gpt-5.5';
        flags.push('-c', 'model_reasoning_effort="low"');
        break;
      case 'codex-gpt-5-5-medium':
        model = 'gpt-5.5';
        flags.push('-c', 'model_reasoning_effort="medium"');
        break;
      case 'codex-gpt-5-5-high':
        model = 'gpt-5.5';
        flags.push('-c', 'model_reasoning_effort="high"');
        break;
      case 'codex-gpt-5-5-xhigh':
        model = 'gpt-5.5';
        flags.push('-c', 'model_reasoning_effort="xhigh"');
        break;
      case 'codex-gpt-5-4-low':
        model = 'gpt-5.4';
        flags.push('-c', 'model_reasoning_effort="low"');
        break;
      case 'codex-gpt-5-4-medium':
        model = 'gpt-5.4';
        flags.push('-c', 'model_reasoning_effort="medium"');
        break;
      case 'codex-gpt-5-4-high':
        model = 'gpt-5.4';
        flags.push('-c', 'model_reasoning_effort="high"');
        break;
      case 'codex-gpt-5-4-xhigh':
        model = 'gpt-5.4';
        flags.push('-c', 'model_reasoning_effort="xhigh"');
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
      case 'gemini-3.1-pro-preview':
      case 'gemini-3-pro-preview':
      case 'gemini-3-flash-preview':
      case 'gemini-2.5-pro':
      case 'gemini-2.5-flash':
      case 'gemini-2.5-flash-lite':
        model = preset;
        break;
      case 'composer-2.5':
      case 'composer-2':
      case 'composer-2-fast':
      case 'composer-1':
        model = preset;
        break;
      case 'glm-5.1':
      case 'glm-4.7':
        model = preset;
        break;
      case 'opencode/big-pickle':
      case 'opencode/grok':
        model = preset;
        break;
      case 'qwen3-coder':
      case 'qwen2.5-coder':
        model = preset;
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
      <label for={`${idPrefix}-label`}>Label</label>
      <input
        id={`${idPrefix}-label`}
        type="text"
        placeholder="Optional display name"
        value={config.label || ''}
        on:input={handleLabelChange}
      />
    </div>
  {/if}

  <div class="field">
    <div class="cli-label-row">
      <label for={`${idPrefix}-cli`}>CLI</label>
      <span
        class="cli-health-badge {cliHealthTone(selectedCliHealth, cliHealthError)}"
        title={cliHealthTitle(selectedCliHealth, cliHealthLoading, cliHealthError)}
        aria-label={`CLI health: ${cliHealthLabel(selectedCliHealth, cliHealthLoading, cliHealthError)}`}
      >
        <span class="health-dot" aria-hidden="true"></span>
        {cliHealthLabel(selectedCliHealth, cliHealthLoading, cliHealthError)}
      </span>
    </div>
    <select
      id={`${idPrefix}-cli`}
      value={config.cli}
      on:change={handleCliChange}
      class="cli-select"
      aria-describedby={`${idPrefix}-cli-description`}
    >
      {#each cliOptions as cli}
        <option value={cli.value} title={cli.description}>
          {cli.label}
        </option>
      {/each}
    </select>
    <span class="cli-description" id={`${idPrefix}-cli-description`}>
      {cliOptions.find(c => c.value === config.cli)?.description || ''}
    </span>
    <span class="cli-health-message {cliHealthTone(selectedCliHealth, cliHealthError)}">
      {cliHealthMessage(selectedCliHealth, cliHealthLoading, cliHealthError)}
    </span>
  </div>

  {#if config.cli === 'antigravity'}
    <div class="field">
      <span class="label-text" id={`${idPrefix}-model-label`}>Model</span>
      <div class="settings-note">
        Set globally in <code>~/.gemini/antigravity-cli/settings.json</code>
        (<code>"model"</code> key). Per-worker override is not supported by
        <code>agy</code>.
      </div>
    </div>
  {:else if config.cli === 'claude' || config.cli === 'codex' || config.cli === 'gemini' || config.cli === 'cursor' || config.cli === 'droid' || config.cli === 'opencode' || config.cli === 'qwen'}
    <div class="field">
      <label for={`${idPrefix}-preset`}>Model &amp; Effort</label>
      <select
        id={`${idPrefix}-preset`}
        value={selectedPreset}
        on:change={handlePresetChange}
        class="cli-select"
        aria-describedby={`${idPrefix}-preset-description ${idPrefix}-effective-model`}
      >
        <option value="custom">Custom (keep current model)</option>
        {#each presetOptions as preset}
          <option value={preset.value}>
            {preset.label}
          </option>
        {/each}
      </select>
      <span class="effective-model" id={`${idPrefix}-effective-model`}>
        Effective: {effectiveModel}{effectiveEffort ? ` · ${effectiveEffort} effort` : ''}
      </span>
      <span class="cli-description" id={`${idPrefix}-preset-description`}>{presetDescription}</span>
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

  .cli-label-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .cli-health-badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 70%;
    padding: 2px 7px;
    border: 1px solid currentColor;
    border-radius: 999px;
    font-size: 10px;
    font-weight: 600;
    line-height: 1.2;
    white-space: nowrap;
  }

  .health-dot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: currentColor;
  }

  .cli-health-badge.healthy,
  .cli-health-message.healthy {
    color: var(--status-success);
  }

  .cli-health-badge.warning,
  .cli-health-message.warning {
    color: var(--status-warning);
  }

  .cli-health-badge.error,
  .cli-health-message.error {
    color: var(--status-error);
  }

  .cli-health-badge.pending,
  .cli-health-message.pending {
    color: var(--text-disabled);
  }

  .cli-health-message {
    font-size: 10px;
    line-height: 1.35;
  }

  label,
  .label-text {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  input,
  .cli-select {
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
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
    color: var(--text-secondary);
    opacity: 0.7;
  }

  .effective-model {
    font-size: 11px;
    color: var(--accent-cyan);
    font-family: var(--font-mono);
  }

  input::placeholder {
    color: var(--text-secondary);
    opacity: 0.6;
  }

  input:focus,
  .cli-select:focus {
    outline: none;
    border-color: var(--accent-cyan);
  }

  .settings-note {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-secondary);
    padding: 8px 10px;
    background: var(--bg-void);
    border: 1px dashed var(--border-structural);
    border-radius: var(--radius-sm);
  }

  .settings-note code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    font-size: 11px;
    background: rgba(255, 255, 255, 0.05);
    padding: 1px 4px;
    border-radius: 3px;
  }
</style>
