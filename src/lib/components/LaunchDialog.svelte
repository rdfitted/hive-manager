<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import AgentConfigEditor from './AgentConfigEditor.svelte';
  import type { AgentConfig, HiveLaunchConfig, SwarmLaunchConfig, FusionLaunchConfig, SoloLaunchConfig, PlannerConfig, WorkerRole } from '$lib/stores/sessions';

  export let show: boolean = false;

  const dispatch = createEventDispatcher<{
    close: void;
    launchHive: HiveLaunchConfig;
    launchSwarm: SwarmLaunchConfig;
    launchFusion: FusionLaunchConfig;
    launchSolo: SoloLaunchConfig;
  }>();

  type SessionMode = 'hive' | 'swarm' | 'fusion' | 'solo';

  // Predefined roles with default CLIs, descriptions, and prompt templates
  const predefinedRoles = [
    {
      type: 'backend',
      label: 'Backend',
      cli: 'claude',
      description: 'Backend code, APIs, databases',
      promptTemplate: `You are the BACKEND specialist. Focus on:
- Server-side logic, APIs, and database operations
- Authentication, authorization, and security
- Performance optimization and caching
- Error handling and logging
- Data validation and sanitization
Do NOT work on frontend/UI code unless it directly interfaces with your backend work.`
    },
    {
      type: 'frontend',
      label: 'Frontend',
      cli: 'claude',
      description: 'UI components, styling, UX',
      promptTemplate: `You are the FRONTEND specialist. Focus on:
- UI components, layouts, and styling
- User interactions and state management
- Accessibility and responsive design
- Client-side validation and error display
- Performance optimization (lazy loading, code splitting)
Do NOT work on backend/server code unless it directly interfaces with your frontend work.`
    },
    {
      type: 'coherence',
      label: 'Coherence',
      cli: 'claude',
      description: 'Ensures code consistency',
      promptTemplate: `You are the COHERENCE specialist. Focus on:
- Ensuring consistent code patterns across the codebase
- Verifying naming conventions are followed
- Checking that similar problems are solved similarly
- Identifying and unifying duplicate logic
- Ensuring documentation matches implementation
Review changes made by other workers and flag inconsistencies.`
    },
    {
      type: 'simplify',
      label: 'Simplify',
      cli: 'claude',
      description: 'Refactors and simplifies code',
      promptTemplate: `You are the SIMPLIFY specialist. Focus on:
- Reducing code complexity and nesting
- Extracting reusable functions and components
- Removing dead code and unused imports
- Improving readability and maintainability
- Simplifying conditional logic
Review changes made by other workers and suggest simplifications.`
    },
    {
      type: 'reviewer',
      label: 'Reviewer',
      cli: 'claude',
      description: 'Reviews code for issues',
      promptTemplate: `You are the REVIEWER specialist. Focus on:
- Identifying bugs, edge cases, and potential issues
- Checking for security vulnerabilities
- Verifying error handling is comprehensive
- Ensuring tests cover critical paths
- Validating changes match requirements
Create detailed review comments for issues found. Do NOT fix issues yourself - that's the Resolver's job.`
    },
    {
      type: 'resolver',
      label: 'Resolver',
      cli: 'claude',
      description: 'Resolves reviewer issues',
      promptTemplate: `You are the RESOLVER specialist. Focus on:
- Addressing issues identified by the Reviewer
- Implementing fixes for bugs and edge cases
- Adding missing error handling
- Writing tests for uncovered paths
- Responding to review comments with fixes
Wait for Reviewer feedback before making changes. Your job is to resolve their concerns.`
    },
    {
      type: 'code-quality',
      label: 'Code Quality',
      cli: 'claude',
      description: 'PR comments, linting, tests',
      promptTemplate: `You are the CODE QUALITY specialist. Focus on:
- Running and fixing linter errors
- Ensuring test coverage meets standards
- Resolving PR review comments
- Fixing type errors and warnings
- Ensuring CI/CD checks pass
Use /resolveprcomments style workflow to systematically address quality issues.`
    },
    {
      type: 'general',
      label: 'General',
      cli: 'claude',
      description: 'General purpose worker',
      promptTemplate: null
    },
  ];

  let mode: SessionMode = 'hive';
  let projectPath = '';
  let prompt = '';
  let launching = false;
  let error = '';

  // CLI options for solo mode
  const cliOptions = [
    { value: 'claude', label: 'Claude Code', description: 'Anthropic Claude (Opus 4.6)' },
    { value: 'gemini', label: 'Gemini CLI', description: 'Google Gemini Pro' },
    { value: 'opencode', label: 'OpenCode', description: 'BigPickle, Grok, multi-model' },
    { value: 'codex', label: 'Codex', description: 'OpenAI GPT-5.3' },
    { value: 'cursor', label: 'Cursor', description: 'Cursor CLI via WSL (Opus 4.6)' },
    { value: 'droid', label: 'Droid', description: 'GLM 4.7 (Factory Droid CLI)' },
    { value: 'qwen', label: 'Qwen', description: 'Qwen Code CLI (Qwen3-Coder)' },
  ];

  // Solo config
  let soloCli = 'claude';
  let soloModel = '';
  let soloTask = '';

  // Queen config (shared)
  let queenConfig: AgentConfig = {
    cli: 'claude',
    flags: [],
    label: undefined,
  };

  // Hive workers with roles - preset team of 6
  let hiveWorkers: (AgentConfig & { selectedRole: string })[] = [
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'backend' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'frontend' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'coherence' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'simplify' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'reviewer' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'resolver' },
  ];

  // Simplified Swarm config - same config for all planners
  let plannerCount = 2;
  let plannerConfig: AgentConfig = { cli: 'claude', flags: [], label: undefined };
  let workersPerPlanner: (AgentConfig & { selectedRole: string })[] = [
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'backend' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'frontend' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'coherence' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'simplify' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'reviewer' },
    { cli: 'claude', flags: [], label: undefined, selectedRole: 'resolver' },
  ];

  // Fusion config
  let variantCount = 2;
  let fusionVariants: FusionVariantConfig[] = [
    { name: 'Variant A', cli: 'claude', flags: [] },
    { name: 'Variant B', cli: 'claude', flags: [] },
    { name: 'Variant C', cli: 'claude', flags: [] },
    { name: 'Variant D', cli: 'claude', flags: [] },
  ];
  let judgeConfig = { cli: 'claude', model: undefined };

  // AgentConfig wrappers for fusion variants (so AgentConfigEditor can be used)
  let variantAgentConfigs: AgentConfig[] = fusionVariants.map(v => ({
    cli: v.cli, model: v.model, flags: [], label: v.name,
  }));
  let judgeAgentConfig: AgentConfig = { cli: judgeConfig.cli, model: judgeConfig.model, flags: [], label: 'Fusion Judge' };

  function handleVariantConfigChange(index: number, detail: AgentConfig) {
    variantAgentConfigs[index] = detail;
    fusionVariants[index] = {
      ...fusionVariants[index],
      cli: detail.cli,
      model: detail.model || undefined,
      flags: detail.flags,
    };
  }

  function handleJudgeConfigChange(detail: AgentConfig) {
    judgeAgentConfig = detail;
    judgeConfig = { cli: detail.cli, model: detail.model || undefined, flags: detail.flags, label: detail.label };
  }


  $: activeFusionVariants = fusionVariants.slice(0, variantCount);

  function createDefaultConfig(roleType: string = 'general'): AgentConfig & { selectedRole: string } {
    const role = predefinedRoles.find(r => r.type === roleType) || predefinedRoles[4];
    return { cli: role.cli, flags: [], label: undefined, selectedRole: roleType };
  }

  function updateWorkerCli(workerIndex: number) {
    const worker = hiveWorkers[workerIndex];
    const role = predefinedRoles.find(r => r.type === worker.selectedRole);
    if (role) {
      hiveWorkers[workerIndex].cli = role.cli;
      hiveWorkers = [...hiveWorkers];
    }
  }

  function updateSwarmWorkerCli(workerIndex: number) {
    const worker = workersPerPlanner[workerIndex];
    const role = predefinedRoles.find(r => r.type === worker.selectedRole);
    if (role) {
      workersPerPlanner[workerIndex].cli = role.cli;
      workersPerPlanner = [...workersPerPlanner];
    }
  }

  function buildWorkerRole(roleType: string): WorkerRole {
    const role = predefinedRoles.find(r => r.type === roleType) || predefinedRoles.find(r => r.type === 'general')!;
    return {
      role_type: role.type,
      label: role.label,
      default_cli: role.cli,
      prompt_template: role.promptTemplate || null,
    };
  }

  function addHiveWorker() {
    if (hiveWorkers.length < 6) {
      hiveWorkers = [...hiveWorkers, createDefaultConfig('general')];
    }
  }

  function removeHiveWorker(index: number) {
    if (hiveWorkers.length > 1) {
      hiveWorkers = hiveWorkers.filter((_, i) => i !== index);
    }
  }

  function addSwarmWorker() {
    if (workersPerPlanner.length < 4) {
      workersPerPlanner = [...workersPerPlanner, createDefaultConfig('general')];
    }
  }

  function removeSwarmWorker(index: number) {
    if (workersPerPlanner.length > 1) {
      workersPerPlanner = workersPerPlanner.filter((_, i) => i !== index);
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

  async function handleSubmit(smokeTest: boolean = false) {
    if (!projectPath.trim()) return;

    launching = true;
    error = '';

    try {
      if (mode === 'hive') {
        // Build worker configs with roles
        const workersWithRoles: AgentConfig[] = hiveWorkers.map((w) => ({
          cli: w.cli,
          flags: w.flags,
          label: w.label,
          role: buildWorkerRole(w.selectedRole),
        }));

        const config: HiveLaunchConfig = {
          project_path: projectPath,
          queen_config: queenConfig,
          workers: workersWithRoles,
          prompt: prompt || undefined,
          with_planning: true, // Planning is always enabled
          smoke_test: smokeTest,
        };
        dispatch('launchHive', config);
      } else if (mode === 'swarm') {
        // Build workers config with roles
        const workersWithRoles: AgentConfig[] = workersPerPlanner.map((w) => ({
          cli: w.cli,
          flags: w.flags,
          label: w.label,
          role: buildWorkerRole(w.selectedRole),
        }));

        const config: SwarmLaunchConfig = {
          project_path: projectPath,
          queen_config: queenConfig,
          planner_count: plannerCount,
          planner_config: plannerConfig,
          workers_per_planner: workersWithRoles,
          prompt: prompt || undefined,
          with_planning: true, // Planning is always enabled
          smoke_test: smokeTest,
        };
        dispatch('launchSwarm', config);
      } else if (mode === 'solo') {
        if (!soloTask.trim()) {
          error = 'Task description is required for solo mode';
          launching = false;
          return;
        }

        const config: SoloLaunchConfig = {
          projectPath,
          taskDescription: soloTask,
          cli: soloCli,
          model: soloModel || undefined,
        };
        dispatch('launchSolo', config);
      } else {
        const config: FusionLaunchConfig = {
          project_path: projectPath,
          variants: activeFusionVariants,
          task_description: prompt,
          judge_config: judgeConfig,
          queen_config: queenConfig,
          with_planning: true,
        };
        dispatch('launchFusion', config);
      }
    } catch (err) {
      error = String(err);
      launching = false;
    }
  }

  function handleSmokeTest() {
    handleSubmit(true);
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
        <button
          class="mode-tab"
          class:active={mode === 'fusion'}
          on:click={() => (mode = 'fusion')}
          type="button"
        >
          Fusion
        </button>
        <button
          class="mode-tab"
          class:active={mode === 'solo'}
          on:click={() => (mode = 'solo')}
          type="button"
        >
          Solo
        </button>
      </div>

      <form on:submit|preventDefault={() => handleSubmit(false)}>
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

        {#if mode !== 'solo'}
          <div class="form-section">
            <h3>Queen Configuration</h3>
            <AgentConfigEditor bind:config={queenConfig} showLabel={true} />
          </div>
        {/if}

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
                  <div class="role-selector">
                    <label for="role-{i}">Role</label>
                    <select
                      id="role-{i}"
                      bind:value={worker.selectedRole}
                      on:change={() => updateWorkerCli(i)}
                      class="role-select"
                      title={predefinedRoles.find(r => r.type === worker.selectedRole)?.description}
                    >
                      {#each predefinedRoles as role}
                        <option value={role.type} title={role.description}>{role.label}</option>
                      {/each}
                    </select>
                  </div>
                  <AgentConfigEditor bind:config={worker} showLabel={true} />
                </div>
              {/each}
            </div>
          </div>
        {:else}
          <div class="form-section">
            <h3>Swarm Configuration</h3>
            <p class="section-description">All planners share the same configuration. Each planner gets its own set of workers.</p>

            <div class="field">
              <label for="planner-count">Number of Planners</label>
              <select id="planner-count" bind:value={plannerCount} class="role-select">
                <option value={1}>1 Planner</option>
                <option value={2}>2 Planners</option>
                <option value={3}>3 Planners</option>
                <option value={4}>4 Planners</option>
              </select>
            </div>

            <div class="subsection">
              <h4>Planner Config (shared by all)</h4>
              <AgentConfigEditor bind:config={plannerConfig} showLabel={true} />
            </div>

            <div class="subsection">
              <div class="section-header">
                <h4>Workers per Planner ({workersPerPlanner.length})</h4>
                <button type="button" class="add-button small" on:click={addSwarmWorker} disabled={workersPerPlanner.length >= 4}>
                  + Add
                </button>
              </div>
              <p class="section-description">Each planner will get this set of workers.</p>
              <div class="workers-list">
                {#each workersPerPlanner as worker, i (i)}
                  <div class="worker-card">
                    <div class="card-header">
                      <span class="card-title">Worker {i + 1}</span>
                      <button
                        type="button"
                        class="remove-button"
                        on:click={() => removeSwarmWorker(i)}
                        disabled={workersPerPlanner.length <= 1}
                      >
                        Remove
                      </button>
                    </div>
                    <div class="role-selector">
                      <label for="swarm-role-{i}">Role</label>
                      <select
                        id="swarm-role-{i}"
                        bind:value={worker.selectedRole}
                        on:change={() => updateSwarmWorkerCli(i)}
                        class="role-select"
                        title={predefinedRoles.find(r => r.type === worker.selectedRole)?.description}
                      >
                        {#each predefinedRoles as role}
                          <option value={role.type} title={role.description}>{role.label}</option>
                        {/each}
                      </select>
                    </div>
                    <AgentConfigEditor bind:config={worker} showLabel={true} />
                  </div>
                {/each}
              </div>
            </div>
          </div>
        {:else if mode === 'fusion'}
          <div class="form-section">
            <h3>Fusion Configuration</h3>
            <p class="section-description">Run multiple agent variants in parallel to compare their outputs. A judge will evaluate and recommend the best result.</p>

            <div class="field">
              <label for="variant-count">Number of Variants</label>
              <select id="variant-count" bind:value={variantCount} class="role-select">
                <option value={2}>2 Variants</option>
                <option value={3}>3 Variants</option>
                <option value={4}>4 Variants</option>
              </select>
            </div>

            <div class="subsection">
              <h4>Variant Configurations</h4>
              <div class="workers-list">
                {#each activeFusionVariants as variant, i (i)}
                  <div class="worker-card">
                    <div class="card-header">
                      <input
                        type="text"
                        class="card-title-input"
                        bind:value={fusionVariants[i].name}
                        placeholder="Variant {String.fromCharCode(65 + i)}"
                      />
                    </div>
                    <AgentConfigEditor
                      config={variantAgentConfigs[i]}
                      showLabel={false}
                      on:change={(e) => handleVariantConfigChange(i, e.detail)}
                    />
                  </div>
                {/each}
              </div>
            </div>

            <div class="subsection">
              <h4>Judge Configuration</h4>
              <p class="section-description">Evaluates variant outputs and recommends a winner.</p>
              <div class="worker-card">
                <AgentConfigEditor
                  config={judgeAgentConfig}
                  showLabel={false}
                  on:change={(e) => handleJudgeConfigChange(e.detail)}
                />
              </div>
            </div>
          </div>
        {:else if mode === 'solo'}
          <div class="form-section">
            <h3>Solo Configuration</h3>
            <p class="section-description">Run a single agent for a specific task without any orchestration overhead.</p>
            
            <div class="field">
              <label for="solo-cli">CLI</label>
              <select id="solo-cli" bind:value={soloCli} class="role-select">
                {#each cliOptions as cli}
                  <option value={cli.value} title={cli.description}>{cli.label}</option>
                {/each}
              </select>
            </div>

            <div class="field">
              <label for="solo-model">Model (optional)</label>
              <input
                id="solo-model"
                type="text"
                bind:value={soloModel}
                placeholder="e.g. opus, gemini-2.0-flash-exp"
              />
            </div>

            <div class="form-group">
              <label for="solo-task">Task Description</label>
              <textarea
                id="solo-task"
                bind:value={soloTask}
                placeholder="What should the agent do? (Required)"
                rows="5"
                required
              ></textarea>
            </div>
          </div>
        {/if}

        {#if mode !== 'solo'}
          <div class="form-group">
            <label for="prompt">Initial Prompt (optional)</label>
            <textarea
              id="prompt"
              bind:value={prompt}
              placeholder="Enter a task for the session..."
              rows="3"
            ></textarea>
          </div>
        {/if}

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="dialog-actions">
          <button type="button" class="cancel-button" on:click={handleClose} disabled={launching}>
            Cancel
          </button>
          <button
            type="button"
            class="smoke-test-button"
            on:click={handleSmokeTest}
            disabled={launching || !projectPath.trim()}
            title="Quick test to validate the entire flow: planning phase, task check-off, and agent spawning"
          >
            Smoke Test
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

  .section-description {
    margin: 0 0 12px 0;
    font-size: 12px;
    color: var(--color-text-muted);
    line-height: 1.4;
  }

  .subsection {
    margin-top: 16px;
    padding-top: 12px;
    border-top: 1px solid var(--color-border);
  }

  .subsection h4 {
    margin-bottom: 10px;
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

  .card-title-input {
    font-size: 12px;
    font-weight: 600;
    color: var(--color-text);
    background: var(--color-bg-secondary);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    padding: 4px 8px;
    width: 150px;
  }

  .card-title-input:focus {
    outline: none;
    border-color: var(--color-accent);
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
  .submit-button,
  .smoke-test-button {
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

  .smoke-test-button {
    background: transparent;
    border: 1px dashed var(--color-warning, #e0af68);
    color: var(--color-warning, #e0af68);
  }

  .smoke-test-button:hover:not(:disabled) {
    background: rgba(224, 175, 104, 0.1);
    border-style: solid;
  }

  .submit-button {
    background: var(--color-accent, #8b5cf6);
    color: var(--color-bg);
  }

  .submit-button:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .submit-button:disabled,
  .cancel-button:disabled,
  .smoke-test-button:disabled {
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

  .role-selector {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 12px;
  }

  .role-selector label {
    font-size: 12px;
    font-weight: 500;
    color: var(--color-text-muted);
  }

  .role-selector.small {
    margin-bottom: 8px;
  }

  .role-selector.small label {
    font-size: 11px;
  }

  .role-select {
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    background: var(--color-bg);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    color: var(--color-text);
    cursor: pointer;
  }

  .role-select:focus {
    outline: none;
    border-color: var(--color-accent, #8b5cf6);
  }

  .role-selector.small .role-select {
    padding: 6px 8px;
    font-size: 12px;
  }

  .checkbox-group {
    margin-top: 8px;
  }

  .checkbox-label {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    cursor: pointer;
    padding: 12px;
    background: var(--color-bg);
    border: 1px solid var(--color-border);
    border-radius: 6px;
    transition: all 0.15s ease;
  }

  .checkbox-label:hover {
    border-color: var(--color-accent, #8b5cf6);
  }

  .checkbox-label input[type="checkbox"] {
    width: 18px;
    height: 18px;
    margin-top: 2px;
    accent-color: var(--color-accent, #8b5cf6);
    cursor: pointer;
  }

  .checkbox-text {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .checkbox-title {
    font-size: 14px;
    font-weight: 500;
    color: var(--color-text);
  }

  .checkbox-description {
    font-size: 12px;
    color: var(--color-text-muted);
    line-height: 1.4;
  }
</style>
