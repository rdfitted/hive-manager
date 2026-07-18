<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import AgentConfigEditor, {
    fetchCliHealth,
    type CliHealthMap,
  } from './AgentConfigEditor.svelte';
  import {
    automaticAdversarialLaneCount,
    buildHiveLaunchConfig,
    createDefaultCodingPrincipal,
    createDefaultHiveFormState,
    nextCodingPrincipalIndex,
    type CodingPrincipalFormConfig,
  } from './hiveLaunch';
  import TemplatePicker from './templates/TemplatePicker.svelte';
  import { routeFusionTemplateCells } from './templates/templateLaunch';
  import Composer from './composer/Composer.svelte';
  import { Crown, CaretDown, CaretRight } from 'phosphor-svelte';
  import type { AgentConfig, DebateDebaterConfig, DebateLaunchConfig, DelegationMode, FusionLaunchConfig, FusionVariantConfig, HiveLaunchConfig, QaWorkerConfig, ResearchLaunchConfig, SoloLaunchConfig, WorkerRole } from '$lib/stores/sessions';
  import type { SessionTemplate } from '$lib/types/domain';
  import { templates, selectedTemplate } from '$lib/stores/templates';
  import { defaultRoles } from '$lib/config/clis';

  export let show: boolean = false;
  export let launching: boolean = false;
  export let launchError: string = '';

  const dispatch = createEventDispatcher<{
    close: void;
    launchHive: HiveLaunchConfig;
    launchResearch: ResearchLaunchConfig;
    launchFusion: FusionLaunchConfig;
    launchSolo: SoloLaunchConfig;
    launchDebate: DebateLaunchConfig;
  }>();

  type SessionMode = 'templates' | 'hive' | 'fusion' | 'solo' | 'research' | 'debate';
  type LaunchWorkerConfig = CodingPrincipalFormConfig;
  let cliHealth: CliHealthMap = {};
  let cliHealthLoading = false;
  let cliHealthError: string | null = null;
  let healthLoadedForOpen = false;

  async function loadCliHealth() {
    cliHealthLoading = true;
    cliHealthError = null;
    try {
      cliHealth = await fetchCliHealth();
    } catch (err) {
      cliHealthError = err instanceof Error ? err.message : String(err);
    } finally {
      cliHealthLoading = false;
    }
  }

  // ... (predefinedRoles same)
  // CLI defaults match backend default_roles in storage/mod.rs
  const predefinedRoles = [
    {
      type: 'principal',
      label: 'Coding Principal',
      cli: 'codex',
      description: 'Owns an implementation lane and delegates bounded subtasks',
      promptTemplate: null,
    },
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
      cli: 'codex',
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

  const defaultHiveForm = createDefaultHiveFormState();
  let queenConfig: AgentConfig = defaultHiveForm.queenConfig;

  function createDefaultConfig(roleType: string = 'general'): LaunchWorkerConfig {
    const generalRole = predefinedRoles.find((r) => r.type === 'general')!;
    const role = predefinedRoles.find((r) => r.type === roleType) ?? generalRole;
    const defaults = defaultRoles[role.type] ?? defaultRoles.general;
    return {
      cli: defaults.cli,
      model: defaults.model,
      flags: [],
      label: undefined,
      selectedRole: roleType,
      promptTemplateOverride: role.promptTemplate || null,
    };
  }

  // A Hive starts with one visible coding principal. Native delegation policy,
  // not a fixed specialist roster, controls how the team grows at runtime.
  let codingPrincipals: LaunchWorkerConfig[] = defaultHiveForm.codingPrincipals;
  let workspaceStrategy = defaultHiveForm.workspaceStrategy;
  let queenDelegationMode: DelegationMode = defaultHiveForm.queenDelegationMode;
  let principalDelegationMode: DelegationMode = defaultHiveForm.principalDelegationMode;
  let queenMaxChildren = defaultHiveForm.queenMaxChildren;
  let queenMaxDepth = defaultHiveForm.queenMaxDepth;
  let principalMaxChildren = defaultHiveForm.principalMaxChildren;
  let principalMaxDepth = defaultHiveForm.principalMaxDepth;
  let showHiveAdvanced = false;

  // Research workers (researchers) - each with its own selectable model
  let researchWorkers: LaunchWorkerConfig[] = [
    createDefaultConfig('general'),
    createDefaultConfig('general'),
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

  // Debate config
  let debaterCount = 2;
  let debateRounds = 3;
  let debateTopic = '';
  let debateDebaters: DebateDebaterConfig[] = [
    { name: 'Debater A', stance: 'Proponent', cli: 'claude', flags: [] },
    { name: 'Debater B', stance: 'Opponent', cli: 'claude', flags: [] },
    { name: 'Debater C', stance: 'Alternative A', cli: 'claude', flags: [] },
    { name: 'Debater D', stance: 'Alternative B', cli: 'claude', flags: [] },
  ];
  let debateJudgeAgentConfig: AgentConfig = { cli: 'claude', flags: [], label: 'Debate Judge' };
  let fusionTemplateError = '';

  let debaterAgentConfigs: AgentConfig[] = debateDebaters.map(d => ({
    cli: d.cli, model: d.model, flags: [], label: d.name,
  }));

  function applyTemplate(template: SessionTemplate | null) {
    if (!template) return;

    if (template.mode !== 'fusion') fusionTemplateError = '';

    sessionName = template.name;
    mode = template.mode as SessionMode;
    workspaceStrategy = template.workspace_strategy === 'isolated_cell' ? 'isolated_cell' : 'shared_cell';

    if (template.mode === 'hive') {
      const queenCell = template.cells.find((cell) => cell.role.trim().toLowerCase() === 'queen');
      if (queenCell) {
        queenConfig = {
          cli: queenCell.cli,
          model: queenCell.model,
          flags: [],
          label: 'Queen',
        };
      }

      const principalCells = template.cells.filter((cell) => cell !== queenCell);
      codingPrincipals = principalCells.length > 0
        ? principalCells.map((cell, index) => {
            const role = predefinedRoles.find((candidate) => candidate.type === cell.role);
            return {
              ...createDefaultConfig(cell.role),
              cli: cell.cli,
              model: cell.model,
              label: role?.label ?? `Coding Principal ${index + 1}`,
              promptTemplateOverride: cell.prompt_template,
            };
          })
        : [createDefaultCodingPrincipal(0)];
    } else if (template.mode === 'fusion') {
      const routing = routeFusionTemplateCells(template.cells);
      if (routing.variants.length === 0) {
        fusionTemplateError = 'This Fusion template has no candidate cells; add a candidate before launching.';
        error = fusionTemplateError;
        return;
      }
      fusionTemplateError = '';
      variantCount = routing.variants.length;
      fusionVariants = routing.variants;
      variantAgentConfigs = fusionVariants.map(v => ({
        cli: v.cli, model: v.model, flags: v.flags ?? [], label: v.name,
      }));
      if (routing.judgeConfig) {
        judgeConfig = routing.judgeConfig;
        judgeAgentConfig = routing.judgeConfig;
      }
    } else if (template.mode === 'debate') {
      debaterCount = template.cells.length;
      debateDebaters = template.cells.map((c, i: number) => ({
        name: `Debater ${String.fromCharCode(65 + i)}`,
        stance: `Stance ${String.fromCharCode(65 + i)}`,
        cli: c.cli,
        model: c.model,
        flags: [],
      }));
      debaterAgentConfigs = debateDebaters.map(d => ({
        cli: d.cli, model: d.model, flags: [], label: d.name,
      }));
    }
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

  function handleDebaterConfigChange(index: number, detail: AgentConfig) {
    debaterAgentConfigs[index] = detail;
    debateDebaters[index] = {
      ...debateDebaters[index],
      cli: detail.cli,
      model: detail.model || undefined,
      flags: detail.flags,
    };
  }

  function handleDebateJudgeConfigChange(detail: AgentConfig) {
    debateJudgeAgentConfig = detail;
  }


  $: activeFusionVariants = fusionVariants.slice(0, variantCount);
  $: activeDebaters = debateDebaters.slice(0, debaterCount);

  function buildWorkerRole(roleType: string, defaultCli: string, promptTemplateOverride?: string | null): WorkerRole {
    const role = predefinedRoles.find(r => r.type === roleType);
    const customLabel = roleType
      .split(/[-_]/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ') || 'Custom';
    return {
      role_type: role?.type ?? roleType,
      label: role?.label ?? customLabel,
      default_cli: defaultCli,
      prompt_template: promptTemplateOverride ?? role?.promptTemplate ?? null,
    };
  }

  function addCodingPrincipal() {
    if (codingPrincipals.length < 3) {
      codingPrincipals = [
        ...codingPrincipals,
        createDefaultCodingPrincipal(nextCodingPrincipalIndex(codingPrincipals)),
      ];
    }
  }

  function removeCodingPrincipal(index: number) {
    if (codingPrincipals.length > 1) {
      codingPrincipals = codingPrincipals.filter((_, i) => i !== index);
    }
  }

  function addResearchWorker() {
    if (researchWorkers.length < 6) {
      researchWorkers = [...researchWorkers, createDefaultConfig('general')];
    }
  }

  function removeResearchWorker(index: number) {
    if (researchWorkers.length > 1) {
      researchWorkers = researchWorkers.filter((_, i) => i !== index);
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
  let withSoloEvaluator = false;
  let evaluatorConfig: AgentConfig = {
    cli: defaultRoles.evaluator.cli,
    model: defaultRoles.evaluator.model,
    flags: [],
    label: 'Evaluator',
  };

  function createDefaultQaWorker(specialization: QaWorkerConfig['specialization'] = 'ui'): QaWorkerConfig {
    return {
      specialization,
      cli: defaultRoles['qa-worker'].cli,
      model: defaultRoles['qa-worker'].model,
      flags: [],
    };
  }

  let qaWorkers: QaWorkerConfig[] = [
    createDefaultQaWorker('ui'),
    createDefaultQaWorker('api'),
    createDefaultQaWorker('a11y'),
  ];
  $: automaticAdversarialLanes = automaticAdversarialLaneCount(
    codingPrincipals.length,
    qaWorkers,
  );
  $: adversarialLaneTarget = Math.ceil(codingPrincipals.length / 2);

  function addQaWorker() {
    if (qaWorkers.length < 6) {
      qaWorkers = [...qaWorkers, createDefaultQaWorker('ui')];
    }
  }

  function removeQaWorker(index: number) {
    if (qaWorkers.length > 1) {
      qaWorkers = qaWorkers.filter((_, i) => i !== index);
    }
  }

  function delegationModeLabel(value: DelegationMode): string {
    if (value === 'disabled') return 'Disabled';
    if (value === 'encouraged') return 'Encouraged';
    return 'Automatic when supported';
  }

  async function handleSubmit(smokeTest: boolean = false) {
    if (!projectPath.trim()) return;
    if (mode === 'templates') {
      error = 'Choose a template before launching.';
      return;
    }

    error = '';

    try {
      if (mode === 'hive') {
        const principalsWithRoles: AgentConfig[] = codingPrincipals.map((principal) => ({
          cli: principal.cli,
          model: principal.model,
          flags: principal.flags,
          label: principal.label,
          role: buildWorkerRole(principal.selectedRole, principal.cli, principal.promptTemplateOverride),
        }));

        const config = buildHiveLaunchConfig({
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
          projectPath,
          queenConfig,
          principals: principalsWithRoles,
          workspaceStrategy,
          queenDelegationMode,
          principalDelegationMode,
          queenMaxChildren,
          queenMaxDepth,
          principalMaxChildren,
          principalMaxDepth,
          prompt: prompt || undefined,
          withPlanning,
          smokeTest,
          withEvaluator,
          evaluatorConfig,
          qaWorkers,
        });
        dispatch('launchHive', config);
      } else if (mode === 'research') {
        // Build researcher worker configs (each keeps its own cli + model)
        const researchers: AgentConfig[] = researchWorkers.map((w, i) => ({
          cli: w.cli,
          model: w.model,
          flags: w.flags,
          label: w.label ?? `Researcher ${i + 1}`,
          role: {
            role_type: 'researcher',
            label: 'Researcher',
            default_cli: w.cli,
            prompt_template: w.promptTemplateOverride ?? null,
          },
        }));

        const config: ResearchLaunchConfig = {
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
          project_path: projectPath,
          queen_config: queenConfig,
          workers: researchers,
          prompt: prompt || undefined,
          with_planning: false,
          with_evaluator: false,
          smoke_test: smokeTest,
        };
        dispatch('launchResearch', config);
      } else if (mode === 'solo') {
        const config: SoloLaunchConfig = {
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
          projectPath,
          taskDescription: soloTask.trim() || undefined,
          cli: soloConfig.cli,
          model: soloConfig.model || undefined,
          flags: soloConfig.flags,
          with_evaluator: withSoloEvaluator,
          evaluator_config: withSoloEvaluator ? evaluatorConfig : undefined,
          qa_workers: withSoloEvaluator ? qaWorkers : undefined,
        };
        dispatch('launchSolo', config);
      } else if (mode === 'debate') {
        const config: DebateLaunchConfig = {
          name: sessionName.trim() || undefined,
          color: sessionColor || undefined,
          project_path: projectPath,
          debaters: activeDebaters,
          topic: prompt,
          rounds: debateRounds,
          judge_config: debateJudgeAgentConfig,
          queen_config: queenConfig,
          with_planning: withPlanning,
          default_cli: 'claude',
          default_model: undefined,
        };
        dispatch('launchDebate', config);
      } else if (mode === 'fusion') {
        if (fusionTemplateError || activeFusionVariants.length === 0) {
          throw new Error(fusionTemplateError || 'Fusion requires at least one candidate variant.');
        }
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
      } else if (mode === 'templates') {
        throw new Error('Choose a template before launching.');
      } else {
        const exhaustiveMode: never = mode;
        throw new Error(`Unsupported launch mode: ${String(exhaustiveMode)}`);
      }
    } catch (err) {
      error = String(err);
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
  $: if (!show) healthLoadedForOpen = false;
  $: if (show && !healthLoadedForOpen) {
    healthLoadedForOpen = true;
    void loadCliHealth();
  }
  $: if (!show) {
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
      aria-labelledby="launch-dialog-title"
      tabindex="-1"
    >
      <h2 id="launch-dialog-title">Launch New Session</h2>

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
          on:click={() => {
            mode = 'fusion';
            fusionTemplateError = '';
            error = '';
          }}
          type="button"
        >
          Fusion
        </button>
        <button
          class="mode-tab"
          class:active={mode === 'debate'}
          on:click={() => (mode = 'debate')}
          type="button"
        >
          Debate
        </button>
        <button
          class="mode-tab"
          class:active={mode === 'research'}
          on:click={() => (mode = 'research')}
          type="button"
        >
          Research
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
            <span class="group-label">Session Color</span>
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

        {#if mode === 'hive' || mode === 'research' || mode === 'fusion' || mode === 'debate'}
          <div class="form-section queen-section">
            <div class="section-heading-copy">
              <h3>Queen</h3>
              <p>Sets direction, coordinates the visible principals, and owns the final integration.</p>
            </div>
            <AgentConfigEditor
              bind:config={queenConfig}
              showLabel={true}
              idPrefix="launch-queen"
              {cliHealth}
              {cliHealthLoading}
              {cliHealthError}
            />
          </div>
        {/if}

        {#if mode === 'hive'}
          <div class="form-section hive-policy-section">
            <div class="section-heading-copy">
              <h3>Hive Topology</h3>
              <p>Keep the team legible: a Queen, a small set of coding principals, and bounded native delegation.</p>
            </div>

            <fieldset class="policy-fieldset">
              <legend>Workspace strategy</legend>
              <div class="choice-grid">
                <label class="choice-card" class:selected={workspaceStrategy === 'shared_cell'}>
                  <input type="radio" name="workspace-strategy" bind:group={workspaceStrategy} value="shared_cell" />
                  <span class="choice-title">Shared cell</span>
                  <span class="choice-description">Queen and principals collaborate in one workspace.</span>
                </label>
                <label class="choice-card" class:selected={workspaceStrategy === 'isolated_cell'}>
                  <input type="radio" name="workspace-strategy" bind:group={workspaceStrategy} value="isolated_cell" />
                  <span class="choice-title">Per-principal worktrees</span>
                  <span class="choice-description">Queen and each visible principal get isolated managed worktrees.</span>
                </label>
              </div>
            </fieldset>

            <div class="delegation-grid">
              <div class="field">
                <label for="queen-delegation">Queen delegation</label>
                <select id="queen-delegation" bind:value={queenDelegationMode} class="role-select">
                  <option value="disabled">Disabled</option>
                  <option value="auto">Automatic when useful</option>
                  <option value="encouraged">Encouraged</option>
                </select>
                <span class="field-hint">Controls native subagents launched by the Queen.</span>
              </div>
              <div class="field">
                <label for="principal-delegation">Principal delegation</label>
                <select id="principal-delegation" bind:value={principalDelegationMode} class="role-select">
                  <option value="disabled">Disabled</option>
                  <option value="auto">Automatic when useful</option>
                  <option value="encouraged">Encouraged</option>
                </select>
                <span class="field-hint">Applies to each visible coding principal.</span>
              </div>
            </div>

            <div class="principals-heading section-header">
              <div>
                <h4>Coding Principals ({codingPrincipals.length})</h4>
                <p class="section-description">Each principal owns a coherent lane and may delegate according to policy.</p>
              </div>
              <button type="button" class="add-button" on:click={addCodingPrincipal} disabled={codingPrincipals.length >= 3}>
                + Add principal
              </button>
            </div>
            <div class="workers-list principal-list">
              {#each codingPrincipals as principal, i (i)}
                <div class="worker-card principal-card">
                  <div class="card-header">
                    <span class="card-title">{principal.label || `Coding Principal ${i + 1}`}</span>
                    <button
                      type="button"
                      class="remove-button"
                      on:click={() => removeCodingPrincipal(i)}
                      disabled={codingPrincipals.length <= 1}
                    >
                      Remove
                    </button>
                  </div>
                  <div class="field principal-role-field">
                    <label for={`launch-principal-${i}-role`}>Specialization</label>
                    <select
                      id={`launch-principal-${i}-role`}
                      bind:value={principal.selectedRole}
                      class="role-select"
                    >
                      {#each predefinedRoles as role}
                        <option value={role.type}>{role.label}</option>
                      {/each}
                      {#if !predefinedRoles.some((role) => role.type === principal.selectedRole)}
                        <option value={principal.selectedRole}>{principal.selectedRole} (custom)</option>
                      {/if}
                    </select>
                    <span class="field-hint">
                      {predefinedRoles.find((role) => role.type === principal.selectedRole)?.description || ''}
                    </span>
                  </div>
                  <AgentConfigEditor
                    bind:config={principal}
                    showLabel={true}
                    idPrefix={`launch-principal-${i}`}
                    {cliHealth}
                    {cliHealthLoading}
                    {cliHealthError}
                  />
                </div>
              {/each}
            </div>

            <button
              type="button"
              class="advanced-toggle"
              on:click={() => showHiveAdvanced = !showHiveAdvanced}
              aria-expanded={showHiveAdvanced}
              aria-controls="hive-delegation-limits"
            >
              {showHiveAdvanced ? 'Hide' : 'Show'} delegation guidance
            </button>
            {#if showHiveAdvanced}
              <div class="advanced-grid" id="hive-delegation-limits">
                <label>Queen target max children <input type="number" min="1" max="8" bind:value={queenMaxChildren} /></label>
                <label>Queen target max depth <input type="number" min="1" max="4" bind:value={queenMaxDepth} /></label>
                <label>Principal target max children <input type="number" min="1" max="8" bind:value={principalMaxChildren} /></label>
                <label>Principal target max depth <input type="number" min="1" max="4" bind:value={principalMaxDepth} /></label>
                <p class="policy-note">These values guide assignments; native harness settings own hard concurrency limits.</p>
              </div>
            {/if}
          </div>
        {/if}

        {#if mode === 'hive' || mode === 'debate' || mode === 'fusion' || mode === 'solo'}
          <div class="form-section">
            <h3>Orchestration Options</h3>
            {#if mode === 'hive' || mode === 'debate'}
              <div class="checkbox-group">
                <label class="checkbox-label">
                  <input type="checkbox" bind:checked={withPlanning} />
                  <div class="checkbox-text">
                    <span class="checkbox-title">Enable Planning Phase</span>
                    <span class="checkbox-description">Create a scoped plan before implementation begins.</span>
                  </div>
                </label>
              </div>
            {:else if mode === 'fusion'}
              <p class="policy-note">Fusion includes planning so candidate Hives start from the same objective.</p>
            {/if}

            {#if mode === 'hive' || mode === 'solo'}
              <div class="checkbox-group">
                <label class="checkbox-label">
                  {#if mode === 'solo'}
                    <input type="checkbox" bind:checked={withSoloEvaluator} />
                  {:else}
                    <input type="checkbox" bind:checked={withEvaluator} />
                  {/if}
                  <div class="checkbox-text">
                    <span class="checkbox-title">Enable Evaluator Peer</span>
                    <span class="checkbox-description">Independently verifies milestones and coordinates QA workers.</span>
                  </div>
                </label>
              </div>
              {#if (mode === 'hive' && withEvaluator) || (mode === 'solo' && withSoloEvaluator)}
                <div class="evaluator-config subsection">
                  <h4>Evaluator Configuration</h4>
                  <AgentConfigEditor
                    bind:config={evaluatorConfig}
                    showLabel={true}
                    idPrefix="launch-evaluator"
                    {cliHealth}
                    {cliHealthLoading}
                    {cliHealthError}
                  />
                </div>

                <div class="qa-workers-config subsection">
                  <div class="section-header">
                    <h4>QA Workers ({qaWorkers.length})</h4>
                    <button type="button" class="add-button small" on:click={addQaWorker} disabled={qaWorkers.length >= 6}>+ Add</button>
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
                            disabled={qaWorkers.length <= 1}
                          >Remove</button>
                        </div>
                        <div class="role-selector small">
                          <label for="qa-spec-{i}">Specialization</label>
                          <select id="qa-spec-{i}" bind:value={worker.specialization} class="role-select">
                            <option value="ui">UI Tester</option>
                            <option value="api">API Tester</option>
                            <option value="a11y">A11Y Tester</option>
                            <option value="adversarial">Adversarial</option>
                          </select>
                        </div>
                        <AgentConfigEditor
                          bind:config={worker}
                          showLabel={false}
                          idPrefix={`launch-qa-${i}`}
                          {cliHealth}
                          {cliHealthLoading}
                          {cliHealthError}
                        />
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}
            {/if}
          </div>
        {/if}

        {#if mode === 'research'}
          <div class="form-section">
            <div class="section-header">
              <h3>Researcher Roster ({researchWorkers.length})</h3>
              <button type="button" class="add-button" on:click={addResearchWorker} disabled={researchWorkers.length >= 6}>
                + Add
              </button>
            </div>
            <p class="section-description">A roster the Queen spawns from on demand — none launch up front. Each runs with its own selectable model; the Queen decides how many to spawn based on the objective.</p>
            <div class="workers-list">
              {#each researchWorkers as worker, i (i)}
                <div class="worker-card">
                  <div class="card-header">
                    <span class="card-title">Researcher {i + 1}</span>
                    <button
                      type="button"
                      class="remove-button"
                      on:click={() => removeResearchWorker(i)}
                      disabled={researchWorkers.length <= 1}
                    >
                      Remove
                    </button>
                  </div>
                  <AgentConfigEditor
                    bind:config={worker}
                    showLabel={true}
                    idPrefix={`launch-researcher-${i}`}
                    {cliHealth}
                    {cliHealthLoading}
                    {cliHealthError}
                  />
                </div>
              {/each}
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
                      idPrefix={`launch-fusion-${i}`}
                      {cliHealth}
                      {cliHealthLoading}
                      {cliHealthError}
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
                  idPrefix="launch-fusion-judge"
                  {cliHealth}
                  {cliHealthLoading}
                  {cliHealthError}
                  on:change={(e) => handleJudgeConfigChange(e.detail)}
                />
              </div>
            </div>
          </div>
        {:else if mode === 'debate'}
          <div class="form-section">
            <h3>Debate Configuration</h3>
            <p class="section-description">Run multiple debaters in parallel to argue different perspectives across multiple rounds. A judge evaluates and renders the verdict.</p>

            <div class="form-row">
              <div class="field">
                <label for="debater-count">Number of Debaters</label>
                <select id="debater-count" bind:value={debaterCount} class="role-select">
                  <option value={2}>2 Debaters</option>
                  <option value={3}>3 Debaters</option>
                  <option value={4}>4 Debaters</option>
                </select>
              </div>

              <div class="field">
                <label for="debate-rounds">Number of Rounds</label>
                <select id="debate-rounds" bind:value={debateRounds} class="role-select">
                  <option value={1}>1 Round</option>
                  <option value={2}>2 Rounds</option>
                  <option value={3}>3 Rounds</option>
                  <option value={4}>4 Rounds</option>
                  <option value={5}>5 Rounds</option>
                </select>
              </div>
            </div>

            <div class="subsection">
              <h4>Debater Configurations</h4>
              <div class="workers-list">
                {#each activeDebaters as debater, i (i)}
                  <div class="worker-card">
                    <div class="card-header" style="display: flex; gap: 8px; flex-direction: column; align-items: stretch; border: none; padding: 0; margin-bottom: 8px;">
                      <input
                        type="text"
                        class="card-title-input"
                        bind:value={debateDebaters[i].name}
                        placeholder="Debater {String.fromCharCode(65 + i)}"
                        style="width: 100%;"
                      />
                      <input
                        type="text"
                        class="card-title-input"
                        style="font-size: 12px; opacity: 0.8; font-weight: normal; width: 100%; border-bottom: 1px dashed color-mix(in srgb, var(--text-primary) 10%, transparent);"
                        bind:value={debateDebaters[i].stance}
                        placeholder="Stance (optional, e.g. Pro / Con)"
                      />
                    </div>
                    <AgentConfigEditor
                      config={debaterAgentConfigs[i]}
                      showLabel={false}
                      idPrefix={`launch-debater-${i}`}
                      {cliHealth}
                      {cliHealthLoading}
                      {cliHealthError}
                      on:change={(e) => handleDebaterConfigChange(i, e.detail)}
                    />
                  </div>
                {/each}
              </div>
            </div>

            <div class="subsection">
              <h4>Judge Configuration</h4>
              <p class="section-description">Evaluates the debate and renders the verdict.</p>
              <div class="worker-card">
                <AgentConfigEditor
                  config={debateJudgeAgentConfig}
                  showLabel={false}
                  idPrefix="launch-debate-judge"
                  {cliHealth}
                  {cliHealthLoading}
                  {cliHealthError}
                  on:change={(e) => handleDebateJudgeConfigChange(e.detail)}
                />
              </div>
            </div>
          </div>
        {:else if mode === 'solo'}
          <div class="form-section">
            <h3>Solo Configuration</h3>
            <p class="section-description">Run a single agent for a specific task without any orchestration overhead.</p>

            <AgentConfigEditor
              bind:config={soloConfig}
              showLabel={false}
              idPrefix="launch-solo"
              {cliHealth}
              {cliHealthLoading}
              {cliHealthError}
            />

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

        {#if mode === 'hive' || mode === 'research' || mode === 'fusion' || mode === 'debate'}
          <div class="form-group">
            <span class="composer-label" id="prompt-label">Initial Prompt (optional)</span>
            <div class="composer-host" aria-labelledby="prompt-label">
              <Composer
                sessionId={null}
                placeholder="Enter a task for the session… (@ to mention, / for commands)"
                persistDraft={false}
                bind:value={prompt}
              />
            </div>
          </div>
        {/if}

        {#if error || launchError}
          <div class="error-message" role="alert">{error || launchError}</div>
        {/if}

        {#if mode !== 'templates'}
        <div class="launch-preview-section">
          <button
            type="button"
            class="preview-toggle"
            on:click={() => showPreview = !showPreview}
            aria-expanded={showPreview}
            aria-controls="launch-preview-content"
          >
            {#if showPreview}
              <CaretDown size={12} weight="light" />
            {:else}
              <CaretRight size={12} weight="light" />
            {/if}
            {showPreview ? 'Hide' : 'Show'} Launch Preview & Topology
          </button>

          {#if showPreview}
            <div class="preview-content" id="launch-preview-content">
              <div class="topology-viz">
                {#if mode === 'solo'}
                  <div class="node solo">
                    <span class="node-label">Solo</span>
                    <span class="node-cli">{soloConfig.cli} · {soloConfig.model || 'default'}</span>
                  </div>
                  {#if withSoloEvaluator}
                    <div class="connector"></div>
                    <div class="worker-nodes">
                      <div class="node worker">
                        <span class="node-label">Evaluator + Prince</span>
                        <span class="node-cli">verification control plane and QA on demand</span>
                      </div>
                    </div>
                  {/if}
                {:else}
                  <div class="node queen">
                    <span class="node-icon">
                      <Crown size={16} weight="light" />
                    </span>
                    <span class="node-label">Queen</span>
                    <span class="node-cli">{queenConfig.cli} · {queenConfig.model || 'default'}</span>
                  </div>
                  {#if mode !== 'research'}
                    <div class="connector"></div>
                    <div class="worker-nodes">
                      {#if mode === 'hive'}
                        {#each codingPrincipals as principal}
                          <div class="node worker">
                            <span class="node-label">{principal.label || 'Coding Principal'}</span>
                            <span class="node-cli">{principal.cli} · {principal.model || 'default'}</span>
                          </div>
                        {/each}
                      {:else if mode === 'fusion'}
                        {#each activeFusionVariants as variant}
                          <div class="node fusion">
                            <span class="node-label">{variant.name}</span>
                            <span class="node-cli">{variant.cli}</span>
                          </div>
                        {/each}
                      {:else if mode === 'debate'}
                        {#each activeDebaters as debater}
                          <div class="node debate">
                            <span class="node-label">{debater.name}</span>
                            <span class="node-cli">{debater.cli}</span>
                          </div>
                        {/each}
                      {/if}
                    </div>
                  {/if}
                {/if}

                {#if mode === 'hive'}
                  <div class="topology-contract">
                    <div class="topology-layer">
                      <span class="topology-layer-title">Managed principals</span>
                      <span>1 Queen + {codingPrincipals.length} manager-launched coding principal{codingPrincipals.length === 1 ? '' : 's'}</span>
                      <span>Workspace: {workspaceStrategy === 'shared_cell' ? 'one shared cell' : 'one Queen worktree plus one per principal'}</span>
                    </div>
                    {#if withEvaluator}
                      <div class="topology-layer">
                        <span class="topology-layer-title">Verification control plane</span>
                        <span>Evaluator + Prince peers; {qaWorkers.length} configured QA role{qaWorkers.length === 1 ? '' : 's'}, plus {automaticAdversarialLanes} missing adversarial lane{automaticAdversarialLanes === 1 ? '' : 's'} added automatically to reach the {adversarialLaneTarget}-lane target.</span>
                      </div>
                    {/if}
                    <div class="topology-layer">
                      <span class="topology-layer-title">Potential native children</span>
                      <span>Queen: {delegationModeLabel(queenDelegationMode)} · Principals: {delegationModeLabel(principalDelegationMode)}</span>
                      <span>Harness-native children stay inside their parent’s assignment; they are not additional managed principals.</span>
                    </div>
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        </div>
        {/if}

        <div class="dialog-actions">
          <button type="button" class="cancel-button" on:click={handleClose} disabled={launching}>
            Cancel
          </button>
          {#if mode === 'hive' || mode === 'research'}
          <button
            type="button"
            class="smoke-test-button"
            on:click={handleSmokeTest}
            disabled={launching || !projectPath.trim()}
            title={mode === 'research'
              ? 'Quick smoke test: the Queen spawns one researcher with a trivial task and reports back — no wiki load or capture'
              : 'Quick test to validate the entire flow: planning phase, task check-off, and agent spawning'}
          >
            Smoke Test
          </button>
          {/if}
          <button type="submit" class="submit-button" disabled={launching || !projectPath.trim() || mode === 'templates'}>
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

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label,
  .form-group .group-label,
  .composer-label {
    display: block;
    margin-bottom: 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .composer-host {
    display: flex;
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

  .section-heading-copy {
    margin-bottom: 14px;
  }

  .section-heading-copy h3 {
    margin-bottom: 4px;
  }

  .section-heading-copy p,
  .policy-note,
  .field-hint {
    margin: 0;
    font-size: 11px;
    line-height: 1.45;
    color: var(--text-secondary);
  }

  .queen-section {
    border-left: 3px solid var(--accent-amber);
  }

  .hive-policy-section {
    border: 1px solid color-mix(in srgb, var(--accent-cyan) 26%, var(--border-structural));
    background: color-mix(in srgb, var(--accent-cyan) 4%, var(--bg-void));
  }

  .policy-fieldset {
    margin: 0 0 16px;
    padding: 0;
    border: 0;
  }

  .policy-fieldset legend {
    margin-bottom: 8px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-secondary);
  }

  .choice-grid,
  .delegation-grid,
  .advanced-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
  }

  .choice-card {
    position: relative;
    display: grid;
    gap: 4px;
    padding: 11px 12px 11px 34px;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    background: var(--bg-surface);
    cursor: pointer;
  }

  .choice-card.selected {
    border-color: var(--accent-cyan);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-cyan) 25%, transparent);
  }

  .choice-card input {
    position: absolute;
    top: 13px;
    left: 12px;
    accent-color: var(--accent-cyan);
  }

  .choice-title {
    font-size: 12px;
    font-weight: 650;
    color: var(--text-primary);
  }

  .choice-description {
    font-size: 10px;
    line-height: 1.4;
    color: var(--text-secondary);
  }

  .delegation-grid {
    margin-bottom: 18px;
  }

  .principals-heading {
    align-items: flex-start;
  }

  .principals-heading .section-description {
    margin: 4px 12px 0 0;
  }

  .principal-card {
    border-left: 3px solid var(--accent-cyan);
  }

  .advanced-toggle {
    margin-top: 12px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--accent-cyan);
    font-size: 11px;
    cursor: pointer;
  }

  .advanced-grid {
    margin-top: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--border-structural);
  }

  .advanced-grid label {
    display: grid;
    gap: 5px;
    font-size: 10px;
    color: var(--text-secondary);
  }

  .advanced-grid input {
    width: 100%;
    padding: 7px 9px;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    background: var(--bg-surface);
    color: var(--text-primary);
  }

  .advanced-grid .policy-note {
    grid-column: 1 / -1;
  }

  @media (max-width: 560px) {
    .choice-grid,
    .delegation-grid,
    .advanced-grid {
      grid-template-columns: 1fr;
    }
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

  .workers-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .worker-card {
    padding: 12px;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
  }

  .qa-worker-card {
    border-left: 3px solid var(--accent-cyan);
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
    border-color: var(--accent-amber);
    background: color-mix(in srgb, var(--accent-amber) 10%, transparent);
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

  .node.worker {
    border-color: var(--accent-cyan);
    background: color-mix(in srgb, var(--accent-cyan) 10%, transparent);
  }
  .node.fusion { border-color: var(--status-success); }
  .node.debate { border-color: var(--accent-purple, #bb9af7); }
  .node.solo { border-color: var(--status-warning); }

  .topology-contract {
    width: 100%;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
    padding-top: 12px;
    border-top: 1px solid var(--border-structural);
  }

  .topology-layer {
    display: grid;
    gap: 4px;
    padding: 10px;
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    font-size: 10px;
    line-height: 1.4;
  }

  .topology-layer-title {
    color: var(--text-primary);
    font-size: 11px;
    font-weight: 700;
  }

  @media (max-width: 560px) {
    .topology-contract {
      grid-template-columns: 1fr;
    }
  }
</style>
