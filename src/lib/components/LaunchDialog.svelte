<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import AgentConfigEditor from './AgentConfigEditor.svelte';
  import type { AgentConfig, HiveLaunchConfig, SwarmLaunchConfig, PlannerConfig } from '$lib/stores/sessions';

  export let show: boolean = false;

  const dispatch = createEventDispatcher<{
    close: void;
    launchHive: HiveLaunchConfig;
    launchSwarm: SwarmLaunchConfig;
  }>();

  type SessionMode = 'hive' | 'swarm';

  let mode: SessionMode = 'hive';
  let projectPath = '';
  let prompt = '';
  let launching = false;
  let error = '';

  // Queen config (shared)
  let queenConfig: AgentConfig = {
    cli: 'claude',
    flags: [],
    label: undefined,
  };

  // Hive workers
  let hiveWorkers: AgentConfig[] = [
    { cli: 'claude', flags: [], label: undefined },
    { cli: 'claude', flags: [], label: undefined },
  ];

  // Swarm planners with their workers
  let swarmPlanners: { config: AgentConfig; domain: string; workers: AgentConfig[] }[] = [
    {
      config: { cli: 'claude', flags: [], label: undefined },
      domain: 'frontend',
      workers: [
        { cli: 'claude', flags: [], label: undefined },
      ],
    },
  ];

  function createDefaultConfig(): AgentConfig {
    return { cli: 'claude', flags: [], label: undefined };
  }

  function addHiveWorker() {
    if (hiveWorkers.length < 6) {
      hiveWorkers = [...hiveWorkers, createDefaultConfig()];
    }
  }

  function removeHiveWorker(index: number) {
    if (hiveWorkers.length > 1) {
      hiveWorkers = hiveWorkers.filter((_, i) => i !== index);
    }
  }

  function addPlanner() {
    if (swarmPlanners.length < 4) {
      swarmPlanners = [
        ...swarmPlanners,
        {
          config: createDefaultConfig(),
          domain: '',
          workers: [createDefaultConfig()],
        },
      ];
    }
  }

  function removePlanner(index: number) {
    if (swarmPlanners.length > 1) {
      swarmPlanners = swarmPlanners.filter((_, i) => i !== index);
    }
  }

  function addPlannerWorker(plannerIndex: number) {
    if (swarmPlanners[plannerIndex].workers.length < 4) {
      swarmPlanners[plannerIndex].workers = [
        ...swarmPlanners[plannerIndex].workers,
        createDefaultConfig(),
      ];
      swarmPlanners = [...swarmPlanners];
    }
  }

  function removePlannerWorker(plannerIndex: number, workerIndex: number) {
    if (swarmPlanners[plannerIndex].workers.length > 1) {
      swarmPlanners[plannerIndex].workers = swarmPlanners[plannerIndex].workers.filter(
        (_, i) => i !== workerIndex
      );
      swarmPlanners = [...swarmPlanners];
    }
  }

  async function browseForFolder() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: 'Select Project Folder',
    });
    if (selected && typeof selected === 'string') {
      projectPath = selected;
    }
  }

  async function handleSubmit() {
    if (!projectPath.trim()) return;

    launching = true;
    error = '';

    try {
      if (mode === 'hive') {
        const config: HiveLaunchConfig = {
          project_path: projectPath,
          queen_config: queenConfig,
          workers: hiveWorkers,
          prompt: prompt || undefined,
        };
        dispatch('launchHive', config);
      } else {
        const config: SwarmLaunchConfig = {
          project_path: projectPath,
          queen_config: queenConfig,
          planners: swarmPlanners.map((p) => ({
            config: p.config,
            domain: p.domain,
            workers: p.workers,
          })),
          prompt: prompt || undefined,
        };
        dispatch('launchSwarm', config);
      }
    } catch (err) {
      error = String(err);
      launching = false;
    }
  }

  function handleClose() {
    if (!launching) {
      dispatch('close');
    }
  }

  function handleOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      handleClose();
    }
  }

  // Reset state when closed
  $: if (!show) {
    launching = false;
    error = '';
  }
</script>

{#if show}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="dialog-overlay" on:click={handleOverlayClick} role="presentation">
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="dialog" on:click|stopPropagation role="dialog" aria-modal="true" tabindex="-1">
      <h2>Launch New Session</h2>

      <div class="mode-tabs">
        <button
          class="mode-tab"
          class:active={mode === 'hive'}
          on:click={() => (mode = 'hive')}
          type="button"
        >
          Hive
        </button>
        <button
          class="mode-tab"
          class:active={mode === 'swarm'}
          on:click={() => (mode = 'swarm')}
          type="button"
        >
          Swarm
        </button>
      </div>

      <form on:submit|preventDefault={handleSubmit}>
        <div class="form-group">
          <label for="projectPath">Project Path</label>
          <div class="path-picker">
            <input
              id="projectPath"
              type="text"
              bind:value={projectPath}
              placeholder="Select a project folder..."
              readonly
              required
            />
            <button type="button" class="browse-button" on:click={browseForFolder}>
              Browse
            </button>
          </div>
        </div>

        <div class="form-section">
          <h3>Queen Configuration</h3>
          <AgentConfigEditor bind:config={queenConfig} showLabel={true} />
        </div>

        {#if mode === 'hive'}
          <div class="form-section">
            <div class="section-header">
              <h3>Workers ({hiveWorkers.length})</h3>
              <button type="button" class="add-button" on:click={addHiveWorker} disabled={hiveWorkers.length >= 6}>
                + Add
              </button>
            </div>
            <div class="workers-list">
              {#each hiveWorkers as worker, i (i)}
                <div class="worker-card">
                  <div class="card-header">
                    <span class="card-title">Worker {i + 1}</span>
                    <button
                      type="button"
                      class="remove-button"
                      on:click={() => removeHiveWorker(i)}
                      disabled={hiveWorkers.length <= 1}
                    >
                      Remove
                    </button>
                  </div>
                  <AgentConfigEditor bind:config={worker} showLabel={true} />
                </div>
              {/each}
            </div>
          </div>
        {:else}
          <div class="form-section">
            <div class="section-header">
              <h3>Planners ({swarmPlanners.length})</h3>
              <button type="button" class="add-button" on:click={addPlanner} disabled={swarmPlanners.length >= 4}>
                + Add Planner
              </button>
            </div>
            <div class="planners-list">
              {#each swarmPlanners as planner, pi (pi)}
                <div class="planner-card">
                  <div class="card-header">
                    <span class="card-title">Planner {pi + 1}</span>
                    <button
                      type="button"
                      class="remove-button"
                      on:click={() => removePlanner(pi)}
                      disabled={swarmPlanners.length <= 1}
                    >
                      Remove
                    </button>
                  </div>
                  <div class="field">
                    <label for="domain-{pi}">Domain</label>
                    <input
                      id="domain-{pi}"
                      type="text"
                      bind:value={planner.domain}
                      placeholder="e.g., frontend, backend, testing"
                    />
                  </div>
                  <AgentConfigEditor bind:config={planner.config} showLabel={true} />

                  <div class="planner-workers">
                    <div class="section-header">
                      <h4>Workers ({planner.workers.length})</h4>
                      <button
                        type="button"
                        class="add-button small"
                        on:click={() => addPlannerWorker(pi)}
                        disabled={planner.workers.length >= 4}
                      >
                        + Add
                      </button>
                    </div>
                    {#each planner.workers as worker, wi (wi)}
                      <div class="worker-mini-card">
                        <div class="card-header">
                          <span class="card-title">Worker {wi + 1}</span>
                          <button
                            type="button"
                            class="remove-button small"
                            on:click={() => removePlannerWorker(pi, wi)}
                            disabled={planner.workers.length <= 1}
                          >
                            Remove
                          </button>
                        </div>
                        <AgentConfigEditor bind:config={worker} showLabel={false} />
                      </div>
                    {/each}
                  </div>
                </div>
              {/each}
            </div>
          </div>
        {/if}

        <div class="form-group">
          <label for="prompt">Initial Prompt (optional)</label>
          <textarea
            id="prompt"
            bind:value={prompt}
            placeholder="Enter a task for the session..."
            rows="3"
          ></textarea>
        </div>

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="dialog-actions">
          <button type="button" class="cancel-button" on:click={handleClose} disabled={launching}>
            Cancel
          </button>
          <button type="submit" class="submit-button" disabled={launching || !projectPath.trim()}>
            {launching ? 'Launching...' : 'Launch'}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .dialog-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .dialog {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    padding: 24px;
    width: 520px;
    max-width: 90vw;
    max-height: 85vh;
    overflow-y: auto;
  }

  .dialog h2 {
    margin: 0 0 16px 0;
    font-size: 18px;
    color: var(--color-text);
  }

  .mode-tabs {
    display: flex;
    gap: 4px;
    margin-bottom: 20px;
    background: var(--color-bg);
    padding: 4px;
    border-radius: 6px;
  }

  .mode-tab {
    flex: 1;
    padding: 8px 16px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--color-text-muted);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .mode-tab:hover {
    color: var(--color-text);
  }

  .mode-tab.active {
    background: var(--color-surface);
    color: var(--color-text);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  }

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label {
    display: block;
    margin-bottom: 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--color-text);
  }

  .form-group input,
  .form-group textarea {
    width: 100%;
    padding: 10px 12px;
    border: 1px solid var(--color-border);
    border-radius: 6px;
    background: var(--color-bg);
    color: var(--color-text);
    font-size: 14px;
    font-family: inherit;
  }

  .form-group input:focus,
  .form-group textarea:focus {
    outline: none;
    border-color: var(--color-accent, #8b5cf6);
  }

  .path-picker {
    display: flex;
    gap: 8px;
  }

  .path-picker input {
    flex: 1;
    cursor: pointer;
  }

  .path-picker input:read-only {
    background: var(--color-surface);
  }

  .browse-button {
    padding: 10px 16px;
    border: 1px solid var(--color-border);
    border-radius: 6px;
    background: var(--color-surface-hover, var(--color-surface));
    color: var(--color-text);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s ease;
  }

  .browse-button:hover {
    background: var(--color-border);
    border-color: var(--color-accent, #8b5cf6);
  }

  .form-section {
    margin-bottom: 20px;
    padding: 16px;
    background: var(--color-bg);
    border-radius: 6px;
  }

  .form-section h3 {
    margin: 0 0 12px 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--color-text);
  }

  .form-section h4 {
    margin: 0;
    font-size: 12px;
    font-weight: 500;
    color: var(--color-text-muted);
  }

  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .section-header h3,
  .section-header h4 {
    margin: 0;
  }

  .add-button {
    padding: 6px 12px;
    border: 1px dashed var(--color-border);
    border-radius: 4px;
    background: transparent;
    color: var(--color-text-muted);
    font-size: 12px;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .add-button:hover:not(:disabled) {
    border-color: var(--color-accent, #8b5cf6);
    color: var(--color-accent, #8b5cf6);
  }

  .add-button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .add-button.small {
    padding: 4px 8px;
    font-size: 11px;
  }

  .workers-list,
  .planners-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .worker-card,
  .planner-card {
    padding: 12px;
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 6px;
  }

  .worker-mini-card {
    padding: 10px;
    background: var(--color-bg);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    margin-top: 8px;
  }

  .card-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .card-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--color-text);
  }

  .remove-button {
    padding: 4px 10px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--color-error, #f7768e);
    font-size: 11px;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .remove-button:hover:not(:disabled) {
    background: rgba(247, 118, 142, 0.15);
  }

  .remove-button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .remove-button.small {
    padding: 2px 8px;
    font-size: 10px;
  }

  .planner-workers {
    margin-top: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--color-border);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 12px;
  }

  .field label {
    font-size: 12px;
    font-weight: 500;
    color: var(--color-text-muted);
  }

  .field input {
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    background: var(--color-bg);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    color: var(--color-text);
  }

  .field input:focus {
    outline: none;
    border-color: var(--color-accent, #8b5cf6);
  }

  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    margin-top: 24px;
  }

  .cancel-button,
  .submit-button {
    padding: 10px 20px;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-button {
    background: var(--color-surface-hover, var(--color-surface));
    color: var(--color-text);
  }

  .cancel-button:hover:not(:disabled) {
    background: var(--color-border);
  }

  .submit-button {
    background: var(--color-accent, #8b5cf6);
    color: var(--color-bg);
  }

  .submit-button:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .submit-button:disabled,
  .cancel-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .error-message {
    padding: 12px;
    margin-bottom: 16px;
    background: rgba(247, 118, 142, 0.15);
    border: 1px solid var(--color-error, #f7768e);
    border-radius: 6px;
    color: var(--color-error, #f7768e);
    font-size: 13px;
    word-break: break-word;
  }
</style>
