<script module lang="ts">
  import { invoke, isTauri } from '@tauri-apps/api/core';
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

  export interface PresetCatalogueEntry {
    provider: string;
    id: string;
    label: string;
    model: string;
    flags: string[];
  }

  const CLI_HEALTH_CACHE_MS = 30_000;
  let cachedCliHealth: CliHealthMap | null = null;
  let cachedCliHealthAt = 0;
  let cliHealthRequest: Promise<CliHealthMap> | null = null;
  let cachedPresetCatalogue: PresetCatalogueEntry[] | null = null;
  let presetCatalogueRequest: Promise<PresetCatalogueEntry[]> | null = null;

  export function normalizePresetCatalogue(payload: unknown): PresetCatalogueEntry[] {
    if (typeof payload !== 'object' || payload === null) return [];
    const rawPresets = (payload as { presets?: unknown }).presets;
    if (!Array.isArray(rawPresets)) return [];

    return rawPresets.flatMap((entry) => {
      if (typeof entry !== 'object' || entry === null) return [];
      const preset = entry as Record<string, unknown>;
      if (
        typeof preset.provider !== 'string' ||
        typeof preset.id !== 'string' ||
        typeof preset.label !== 'string' ||
        typeof preset.model !== 'string' ||
        !Array.isArray(preset.flags) ||
        !preset.flags.every((flag) => typeof flag === 'string')
      ) {
        return [];
      }

      return [{
        provider: preset.provider,
        id: preset.id,
        label: preset.label,
        model: preset.model,
        flags: preset.flags as string[],
      }];
    });
  }

  export async function fetchPresetCatalogue(force = false): Promise<PresetCatalogueEntry[]> {
    if (!force && cachedPresetCatalogue) return cachedPresetCatalogue;
    if (presetCatalogueRequest) return presetCatalogueRequest;

    presetCatalogueRequest = (async () => {
      const response = await fetch(apiUrl('/api/preset-catalogue'));
      if (!response.ok) throw new Error(`Preset catalogue request failed (${response.status})`);
      const presets = normalizePresetCatalogue(await response.json());
      if (presets.length === 0) throw new Error('Preset catalogue response was empty');
      cachedPresetCatalogue = presets;
      return presets;
    })();

    try {
      return await presetCatalogueRequest;
    } finally {
      presetCatalogueRequest = null;
    }
  }

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
      let payload: unknown;
      if (isTauri()) {
        payload = await invoke<unknown>('get_cli_health');
      } else {
        const response = await fetch(apiUrl('/api/cli-health'));
        if (!response.ok) throw new Error(`CLI health request failed (${response.status})`);
        payload = await response.json();
      }

      const health = normalizeCliHealth(payload);
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
  import { createEventDispatcher, onMount } from 'svelte';
  import type { AgentConfig } from '$lib/stores/sessions';
  import { cliOptions, getDefaultModel, normalizeModelId } from '$lib/config/clis';

  export let config: AgentConfig;
  export let showLabel: boolean = true;
  export let idPrefix: string;
  export let cliHealth: CliHealthMap = {};
  export let cliHealthLoading: boolean = false;
  export let cliHealthError: string | null = null;

  const dispatch = createEventDispatcher<{ change: AgentConfig }>();
  let presetCatalogue: PresetCatalogueEntry[] = [];
  let presetCatalogueLoading = true;
  let presetCatalogueError: string | null = null;

  onMount(() => {
    let mounted = true;
    fetchPresetCatalogue()
      .then((presets) => {
        if (mounted) presetCatalogue = presets;
      })
      .catch((error: unknown) => {
        if (mounted) {
          presetCatalogueError = error instanceof Error
            ? error.message
            : 'Preset catalogue is unavailable';
        }
      })
      .finally(() => {
        if (mounted) presetCatalogueLoading = false;
      });

    return () => {
      mounted = false;
    };
  });

  $: presetOptions = presetCatalogue.filter((preset) => preset.provider === config.cli);

  function sameFlags(left: string[], right: string[]): boolean {
    return left.length === right.length && left.every((flag, index) => flag === right[index]);
  }

  function inferSelectedPreset(
    value: AgentConfig,
    options: PresetCatalogueEntry[],
  ): string {
    if (!value.model) return 'custom';
    const model = normalizeModelId(value.cli, value.model);
    const flags = value.flags || [];
    return options.find((preset) => (
      preset.model === model && sameFlags(preset.flags, flags)
    ))?.id ?? 'custom';
  }

  function presetEffort(preset: PresetCatalogueEntry | undefined): string | undefined {
    if (!preset) return undefined;
    if (preset.provider === 'codex') {
      const effort = preset.flags.find((flag) => flag.startsWith('model_reasoning_effort='));
      return effort?.match(/^model_reasoning_effort=["']?([^"']+)["']?$/)?.[1];
    }
    if (preset.provider === 'claude') {
      const settingsIndex = preset.flags.indexOf('--settings');
      if (settingsIndex >= 0 && settingsIndex + 1 < preset.flags.length) {
        try {
          return (JSON.parse(preset.flags[settingsIndex + 1]) as { effortLevel?: string }).effortLevel;
        } catch {
          return undefined;
        }
      }
    }
    return undefined;
  }

  $: selectedPreset = inferSelectedPreset(config, presetOptions);
  $: selectedPresetEntry = presetOptions.find((preset) => preset.id === selectedPreset);
  $: effectiveModel = config.model
    ? normalizeModelId(config.cli, config.model)
    : getDefaultModel(config.cli) || 'CLI default';
  $: effectiveEffort = presetEffort(selectedPresetEntry);
  $: selectedCliHealth = cliHealth[config.cli];

  $: presetDescription = config.cli === 'claude'
    ? 'Claude effort presets add --settings {"effortLevel":"low|high|max"}'
    : config.cli === 'codex'
      ? 'Adds -c model_reasoning_effort="low|medium|high|xhigh|max|ultra"'
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

  function applyPreset(presetId: string): void {
    if (presetId === 'custom') {
      return;
    }

    const preset = presetOptions.find((option) => option.id === presetId);
    if (!preset) {
      return;
    }

    const cleanedFlags = stripManagedEffortFlags(
      'codex',
      stripManagedEffortFlags('claude', config.flags || []),
    );
    config = {
      ...config,
      model: preset.model,
      flags: [...cleanedFlags, ...preset.flags],
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
        class="lattice-input"
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
      <!-- Indeterminate CLI health-check action affordance, not content loading. -->
      <span
        class="cli-health-badge lattice-forced-colors-boundary {cliHealthTone(selectedCliHealth, cliHealthError)}"
        title={cliHealthTitle(selectedCliHealth, cliHealthLoading, cliHealthError)}
        aria-label={`CLI health: ${cliHealthLabel(selectedCliHealth, cliHealthLoading, cliHealthError)}`}
        aria-busy={cliHealthLoading}
      >
        <span class="health-dot" aria-hidden="true"></span>
        {cliHealthLabel(selectedCliHealth, cliHealthLoading, cliHealthError)}
      </span>
    </div>
    <select
      id={`${idPrefix}-cli`}
      value={config.cli}
      on:change={handleCliChange}
      class="cli-select lattice-input"
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

  {#if config.cli === 'claude' || config.cli === 'codex' || config.cli === 'cursor' || config.cli === 'droid' || config.cli === 'opencode' || config.cli === 'qwen'}
    <div class="field">
      <label for={`${idPrefix}-preset`}>Model &amp; Effort</label>
      <select
        id={`${idPrefix}-preset`}
        value={selectedPreset}
        on:change={handlePresetChange}
        class="cli-select lattice-input"
        aria-describedby={`${idPrefix}-preset-description ${idPrefix}-effective-model`}
      >
        <option value="custom">Custom (keep current model)</option>
        {#if presetCatalogueLoading}
          <option disabled>Loading presets…</option>
        {:else if presetCatalogueError}
          <option disabled>Presets unavailable</option>
        {/if}
        {#each presetOptions as preset}
          <option value={preset.id}>
            {preset.label}
          </option>
        {/each}
      </select>
      <span class="effective-model" id={`${idPrefix}-effective-model`}>
        Effective: {effectiveModel}{effectiveEffort ? ` · ${effectiveEffort} effort` : ''}
      </span>
      <span class="cli-description" id={`${idPrefix}-preset-description`}>
        {presetCatalogueError || presetDescription}
      </span>
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
    /* Executable-health pill is distinct from the session status-badge contract. */
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 70%;
    padding: 2px 7px;
    box-shadow: inset 0 0 0 1px currentColor;
    border-radius: var(--radius-full);
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

  label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  .lattice-input {
    width: 100%;
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

</style>
