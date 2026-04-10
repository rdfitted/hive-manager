<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import AgentConfigEditor from './AgentConfigEditor.svelte';
  import TemplatePicker from './templates/TemplatePicker.svelte';
  import type { AgentConfig, HiveLaunchConfig, SwarmLaunchConfig, FusionLaunchConfig, FusionVariantConfig, SoloLaunchConfig, PlannerConfig, WorkerRole, QaWorkerConfig } from '$lib/stores/sessions';
  import type { SessionTemplate } from '$lib/types/domain';
  import { templates, selectedTemplate } from '$lib/stores/templates';

  export let show: boolean = false;

  const dispatch = createEventDispatcher<{
    close: void;
    launchHive: HiveLaunchConfig;
    launchSwarm: SwarmLaunchConfig;
    launchFusion: FusionLaunchConfig;
    launchSolo: SoloLaunchConfig;
  }>();

  type SessionMode = 'templates' | 'hive' | 'fusion' | 'solo' | 'swarm';
  type LaunchWorkerConfig = AgentConfig & { selectedRole: string; promptTemplateOverride?: string | null };

  // ... (predefinedRoles same)
  // CLI defaults match backend default_roles in storage/mod.rs
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
      cli: 'gemini',
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
      cli: 'droid',
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
      cli: 'codex',
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
  let sessionName = '';
  let sessionColor = '';
  let prompt = '';
  let launching = false;
  let error = '';
  let showPreview = false;

  const COLORS = [
    { name: 'Blue', value: '#7aa2f7' },
    { name: 'Purple', value: '#bb9af7' },
    { name: 'Green', value: '#9ece6a' },
    { name: 'Yellow', value: '#e0af68' },
    { name: 'Cyan', value: '#7dcfff' },
    { name: 'Red', value: '#f7768e' },
    { name: 'Orange', value: '#ff9e64' },
    { name: 'Pink', value: '#f7b1d1' },
  ];

  // Solo config
  let soloConfig: AgentConfig = { cli: 'claude', flags: [], label: undefined };
  let soloTask = '';

  // Queen config (shared)
  let queenConfig: AgentConfig = {
    cli: 'claude',
    flags: [],
    label: undefined,
  };

  function createDefaultConfig(roleType: string = 'general'): LaunchWorkerConfig {
    const generalRole = predefinedRoles.find((r) => r.type === 'general')!;
    const role = predefinedRoles.find((r) => r.type === roleType) ?? generalRole;
    return {
      cli: role.cli,
      flags: [],
      label: undefined,
      selectedRole: roleType,
      promptTemplateOverride: role.promptTemplate || null,
    };
  }

  // Hive workers with roles - preset team of 6
  let hiveWorkers: LaunchWorkerConfig[] = [
    createDefaultConfig('backend'),
    createDefaultConfig('frontend'),
    createDefaultConfig('coherence'),
    createDefaultConfig('simplify'),
    createDefaultConfig('reviewer'),
    createDefaultConfig('resolver'),
  ];

  // Simplified Swarm config - same config for all planners
  let plannerCount = 2;
  let plannerConfig: AgentConfig = { cli: 'claude', flags: [], label: undefined };
  let workersPerPlanner: LaunchWorkerConfig[] = [
    createDefaultConfig('backend'),
    createDefaultConfig('frontend'),
    createDefaultConfig('coherence'),
    createDefaultConfig('simplify'),
    createDefaultConfig('reviewer'),
    createDefaultConfig('resolver'),
  ];

  // Fusion config
  let variantCount = 2;
  let fusionVariants: FusionVariantConfig[] = [
    { name: 'Variant A', cli: 'claude', flags: [] },
    { name: 'Variant B', cli: 'claude', flags: [] },
    { name: 'Variant C', cli: 'claude', flags: [] },
    { name: 'Variant D', cli: 'claude', flags: [] },
  ];
  let judgeConfig: { cli: string; model?: string; flags?: string[]; label?: string } = { cli: 'claude' };

  // AgentConfig wrappers for fusion variants (so AgentConfigEditor can be used)
  let variantAgentConfigs: AgentConfig[] = fusionVariants.map(v => ({
    cli: v.cli, model: v.model, flags: [], label: v.name,
  }));
  let judgeAgentConfig: AgentConfig = { cli: judgeConfig.cli, model: judgeConfig.model, flags: [], label: 'Fusion Judge' };

  function applyTemplate(template: SessionTemplate | null) {
    if (!template) return;
    
    sessionName = template.name;
    mode = template.mode as SessionMode;
    
    if (template.mode === 'hive') {
      hiveWorkers = template.cells.map((c) => ({
        ...createDefaultConfig(c.role),
        cli: c.cli,
        model: c.model,
        promptTemplateOverride: c.prompt_template,
      }));
    } else if (template.mode === 'fusion') {
      variantCount = template.cells.length;
      fusionVariants = template.cells.map((c, i: number) => ({
        name: `Variant ${String.fromCharCode(65 + i)}`,
        cli: c.cli,
        model: c.model,
        flags: [],
      }));
      variantAgentConfigs = fusionVariants.map(v => ({
        cli: v.cli, model: v.model, flags: [], label: v.name,
      }));
    }
    
    // Switch to the actual mode after applying
    // mode = template.mode as SessionMode; // already set above
  }

  $: if ($selectedTemplate) {
    applyTemplate($selectedTemplate);
    selectedTemplate.set(null); // Clear after applying
  }

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

  function buildWorkerRole(roleType: string, promptTemplateOverride?: string | null): WorkerRole {
    const role = predefinedRoles.find(r => r.type === roleType) || predefinedRoles.find(r => r.type === 'general')!;
    return {
      role_type: role.type,
      label: role.label,
      default_cli: role.cli,
      prompt_template: promptTemplateOverride ?? role.promptTemplate ?? null,
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

  let withPlanning = true;
  let withEvaluator = true;
  let evaluatorConfig: AgentConfig = {
    cli: 'claude',
    flags: [],
    label: 'Evaluator',
  };

  let qaWorkers: QaWorkerConfig[] = [
    { specialization: 'ui', cli: 'claude', flags: [] },
    { specialization: 'api', cli: 'claude', flags: [] },
    { specialization: 'a11y', cli: 'claude', flags: [] },
  ];

  function addQaWorker() {
    if (qaWorkers.length < 6) {
      qaWorkers = [...qaWorkers, { specialization: 'ui', cli: 'claude', flags: [] }];
    }
  }

  function removeQaWorker(index: number) {
    if (qaWorkers.length > 0) {
      qaWorkers = qaWorkers.filter((_, i) => i !== index);
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
          role: buildWorkerRole(w.selectedRole, w.promptTemplateOverride),
        }));

        const config: HiveLaunchConfig = {
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
          project_path: projectPath,
          queen_config: queenConfig,
          workers: workersWithRoles,
          prompt: prompt || undefined,
          with_planning: withPlanning,
          smoke_test: smokeTest,
          with_evaluator: withEvaluator,
          evaluator_config: withEvaluator ? evaluatorConfig : undefined,
          qa_workers: withEvaluator ? qaWorkers : undefined,
        };
        dispatch('launchHive', config);
      } else if (mode === 'swarm') {
        // Build workers config with roles
        const workersWithRoles: AgentConfig[] = workersPerPlanner.map((w) => ({
          cli: w.cli,
          flags: w.flags,
          label: w.label,
          role: buildWorkerRole(w.selectedRole, w.promptTemplateOverride),
        }));

        const config: SwarmLaunchConfig = {
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
          project_path: projectPath,
          queen_config: queenConfig,
          planner_count: plannerCount,
          planner_config: plannerConfig,
          workers_per_planner: workersWithRoles,
          prompt: prompt || undefined,
          with_planning: true, // Planning is always enabled
          smoke_test: smokeTest,
          with_evaluator: withEvaluator,
          evaluator_config: withEvaluator ? evaluatorConfig : undefined,
          qa_workers: withEvaluator ? qaWorkers : undefined,
        };
        dispatch('launchSwarm', config);
      } else if (mode === 'solo') {
        const config: SoloLaunchConfig = {
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
          projectPath,
          taskDescription: soloTask.trim() || undefined,
          cli: soloConfig.cli,
          model: soloConfig.model || undefined,
        };
        dispatch('launchSolo', config);
      } else {
        const config: FusionLaunchConfig = {
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
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
    <div
      class="dialog"
      on:click|stopPropagation
      role="dialog"
      aria-modal="true"
      tabindex="-1"
    >
      <h2>Launch New Session</h2>

      <div class="mode-tabs">
        <button
          class="mode-tab"
          class:active={mode === 'templates'}
          on:click={() => (mode = 'templates')}
          type="button"
        >
          Templates
        </button>
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
        <button
          class="mode-tab legacy"
          class:active={mode === 'swarm'}
          on:click={() => (mode = 'swarm')}
          type="button"
        >
          Swarm (Legacy)
        </button>
      </div>

      <form on:submit={(e) => { e.preventDefault(); handleSubmit(false); }}>
        {#if mode === 'templates'}
          <div class="form-section">
            <TemplatePicker />
          </div>
        {/if}

        <div class="form-row">
          <div class="form-group flex-2">
            <label for="sessionName">Session Name (optional)</label>
            <input
              id="sessionName"
              type="text"
              bind:value={sessionName}
              placeholder="e.g. Refactor API"
            />
          </div>
          <div class="form-group flex-1">
            <label>Session Color</label>
            <div class="color-picker-inline">
              {#each COLORS as color}
                <button
                  type="button"
                  class="color-circle"
                  style:background={color.value}
                  class:selected={sessionColor === color.value}
                  on:click={() => sessionColor = color.value}
                  title={color.name}
                >
                </button>
              {/each}
              <button
                type="button"
                class="color-circle clear"
                on:click={() => sessionColor = ''}
                title="Clear color"
              >×</button>
            </div>
          </div>
        </div>

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

          <div class="form-section">
            <h3>Orchestration Options</h3>
            <div class="checkbox-group">
              <label class="checkbox-label">
                <input type="checkbox" bind:checked={withPlanning} />
                <div class="checkbox-text">
                  <span class="checkbox-title">Enable Planning Phase</span>
                  <span class="checkbox-description">Master Planner analyzes the project and creates a task list before workers start.</span>
                </div>
              </label>
            </div>
            <div class="checkbox-group">
              <label class="checkbox-label">
                <input type="checkbox" bind:checked={withEvaluator} />
                <div class="checkbox-text">
                  <span class="checkbox-title">Enable Evaluator Peer</span>
                  <span class="checkbox-description">Independent agent that verifies milestone completion and manages QA workers.</span>
                </div>
              </label>
            </div>
            {#if withEvaluator}
              <div class="evaluator-config subsection">
                <h4>Evaluator Configuration</h4>
                <AgentConfigEditor bind:config={evaluatorConfig} showLabel={true} />
              </div>

              <div class="qa-workers-config subsection">
                <div class="section-header">
                  <h4>QA Workers ({qaWorkers.length})</h4>
                  <button type="button" class="add-button small" on:click={addQaWorker} disabled={qaWorkers.length >= 6}>
                    + Add
                  </button>
                </div>
                <div class="workers-list">
                  {#each qaWorkers as worker, i (i)}
                    <div class="worker-card qa-worker-card">
                      <div class="card-header">
                        <span class="card-title">QA Worker {i + 1}</span>
                        <button
                          type="button"
                          class="remove-button small"
                          on:click={() => removeQaWorker(i)}
                        >
                          Remove
                        </button>
                      </div>
                      <div class="role-selector small">
                        <label for="qa-spec-{i}">Specialization</label>
                        <select
                          id="qa-spec-{i}"
                          bind:value={worker.specialization}
                          class="role-select"
                        >
                          <option value="ui">UI Tester</option>
                          <option value="api">API Tester</option>
                          <option value="a11y">A11Y Tester</option>
                        </select>
                      </div>
                      <AgentConfigEditor bind:config={worker} showLabel={false} />
                    </div>
                  {/each}
                </div>
              </div>
            {/if}
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
        {:else if mode === 'swarm'}
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

            <AgentConfigEditor bind:config={soloConfig} showLabel={false} />

            <div class="form-group">
              <label for="solo-task">Task Description</label>
              <textarea
                id="solo-task"
                bind:value={soloTask}
                placeholder="What should the agent do? (Leave empty for interactive mode)"
                rows="5"
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

        <div class="launch-preview-section">
          <button type="button" class="preview-toggle" on:click={() => showPreview = !showPreview}>
            <span class="icon">{showPreview ? '▼' : '▶'}</span>
            {showPreview ? 'Hide' : 'Show'} Launch Preview & Topology
          </button>
          
          {#if showPreview}
            <div class="preview-content">
              <div class="topology-viz">
                <div class="node queen">
                  <span class="node-icon">♕</span>
                  <span class="node-label">Queen</span>
                  <span class="node-cli">{queenConfig.cli}</span>
                </div>
                
                <div class="connector"></div>
                
                <div class="worker-nodes">
                  {#if mode === 'hive'}
                    {#each hiveWorkers as worker}
                      <div class="node worker">
                        <span class="node-label">{worker.selectedRole}</span>
                        <span class="node-cli">{worker.cli}</span>
                      </div>
                    {/each}
                  {:else if mode === 'fusion'}
                    {#each activeFusionVariants as variant}
                      <div class="node fusion">
                        <span class="node-label">{variant.name}</span>
                        <span class="node-cli">{variant.cli}</span>
                      </div>
                    {/each}
                  {:else if mode === 'solo'}
                    <div class="node solo">
                      <span class="node-label">Solo</span>
                      <span class="node-cli">{soloConfig.cli}</span>
                    </div>
                  {/if}
                </div>
              </div>
            </div>
          {/if}
        </div>

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
    background: color-mix(in srgb, var(--bg-void) 60%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .dialog {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    padding: 24px;
    width: 520px;
    max-width: 90vw;
    max-height: 85vh;
    overflow-y: auto;
  }

  .dialog h2 {
    margin: 0 0 16px 0;
    font-size: 18px;
    color: var(--text-primary);
  }

  .mode-tabs {
    display: flex;
    gap: 4px;
    margin-bottom: 20px;
    background: var(--bg-void);
    padding: 4px;
    border-radius: var(--radius-sm);
  }

  .mode-tab {
    flex: 1;
    padding: 8px 16px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .mode-tab:hover {
    color: var(--text-primary);
  }

  .mode-tab.active {
    background: var(--bg-surface);
    color: var(--text-primary);
    box-shadow: 0 1px 3px color-mix(in srgb, var(--bg-void) 20%, transparent);
  }

  .mode-tab.legacy {
    opacity: 0.6;
    font-style: italic;
  }

  .mode-tab.legacy.active {
    opacity: 1;
  }

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label {
    display: block;
    margin-bottom: 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .form-row {
    display: flex;
    gap: 16px;
    margin-bottom: 16px;
  }

  .flex-1 { flex: 1; }
  .flex-2 { flex: 2; }

  .color-picker-inline {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
  }

  .color-circle {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
    transition: transform 0.15s ease;
    padding: 0;
  }

  .color-circle:hover {
    transform: scale(1.2);
  }

  .color-circle.selected {
    border-color: var(--text-primary);
    transform: scale(1.1);
  }

  .color-circle.clear {
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-elevated);
    color: var(--text-secondary);
    font-size: 14px;
    border: 1px solid var(--border-structural);
  }

  .form-group input,
  .form-group textarea {
    width: 100%;
    padding: 10px 12px;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    background: var(--bg-void);
    color: var(--text-primary);
    font-size: 14px;
    font-family: inherit;
  }

  .form-group input:focus,
  .form-group textarea:focus {
    outline: none;
    border-color: var(--accent-cyan);
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
    background: var(--bg-surface);
  }

  .browse-button {
    padding: 10px 16px;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    color: var(--text-primary);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s ease;
  }

  .browse-button:hover {
    background: var(--border-structural);
    border-color: var(--accent-cyan);
  }

  .form-section {
    margin-bottom: 20px;
    padding: 16px;
    background: var(--bg-void);
    border-radius: var(--radius-sm);
  }

  .form-section h3 {
    margin: 0 0 12px 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .form-section h4 {
    margin: 0;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  .section-description {
    margin: 0 0 12px 0;
    font-size: 12px;
    color: var(--text-secondary);
    line-height: 1.4;
  }

  .subsection {
    margin-top: 16px;
    padding-top: 12px;
    border-top: 1px solid var(--border-structural);
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
    border: 1px dashed var(--border-structural);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    font-size: 12px;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .add-button:hover:not(:disabled) {
    border-color: var(--accent-cyan);
    color: var(--accent-cyan);
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
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
  }

  .qa-worker-card {
    border-left: 3px solid var(--accent-cyan);
  }

  .worker-mini-card {
    padding: 10px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
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
    color: var(--text-primary);
  }

  .card-title-input {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary);
    background: var(--bg-elevated);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    padding: 4px 8px;
    width: 150px;
  }

  .card-title-input:focus {
    outline: none;
    border-color: var(--accent-cyan);
  }

  .remove-button {
    padding: 4px 10px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--status-error);
    font-size: 11px;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .remove-button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--status-error) 15%, transparent);
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
    border-top: 1px solid var(--border-structural);
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
    color: var(--text-secondary);
  }

  .field input {
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
  }

  .field input:focus {
    outline: none;
    border-color: var(--accent-cyan);
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
    border-radius: var(--radius-sm);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-button {
    background: var(--bg-elevated);
    color: var(--text-primary);
  }

  .cancel-button:hover:not(:disabled) {
    background: var(--border-structural);
  }

  .smoke-test-button {
    background: transparent;
    border: 1px dashed var(--status-warning);
    color: var(--status-warning);
  }

  .smoke-test-button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--status-warning) 10%, transparent);
    border-style: solid;
  }

  .submit-button {
    background: var(--accent-cyan);
    color: var(--bg-void);
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
    background: color-mix(in srgb, var(--status-error) 15%, transparent);
    border: 1px solid var(--status-error);
    border-radius: var(--radius-sm);
    color: var(--status-error);
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
    color: var(--text-secondary);
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
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    cursor: pointer;
  }

  .role-select:focus {
    outline: none;
    border-color: var(--accent-cyan);
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
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    transition: all 0.15s ease;
  }

  .checkbox-label:hover {
    border-color: var(--accent-cyan);
  }

  .checkbox-label input[type="checkbox"] {
    width: 18px;
    height: 18px;
    margin-top: 2px;
    accent-color: var(--accent-cyan);
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
    color: var(--text-primary);
  }

  .checkbox-description {
    font-size: 12px;
    color: var(--text-secondary);
    line-height: 1.4;
  }

  .launch-preview-section {
    margin-top: 20px;
    border-top: 1px solid var(--border-structural);
    padding-top: 16px;
  }

  .preview-toggle {
    background: transparent;
    border: none;
    color: var(--accent-cyan);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0;
  }

  .preview-content {
    margin-top: 16px;
    background: color-mix(in srgb, var(--bg-void) 20%, transparent);
    border-radius: var(--radius-sm);
    padding: 16px;
  }

  .topology-viz {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
  }

  .node {
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    display: flex;
    flex-direction: column;
    align-items: center;
    min-width: 100px;
  }

  .node.queen {
    border-color: var(--accent-cyan);
    background: color-mix(in srgb, var(--accent-cyan) 10%, transparent);
  }

  .node-icon {
    font-size: 16px;
    margin-bottom: 2px;
  }

  .node-label {
    font-size: 11px;
    font-weight: 700;
    color: var(--text-primary);
  }

  .node-cli {
    font-size: 9px;
    color: var(--text-secondary);
    font-family: var(--font-mono);
  }

  .connector {
    width: 2px;
    height: 16px;
    background: var(--border-structural);
  }

  .worker-nodes {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
  }

  .node.worker { border-color: var(--accent-cyan); }
  .node.fusion { border-color: var(--status-success); }
  .node.solo { border-color: var(--status-warning); }
</style>
