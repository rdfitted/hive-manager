<script lang="ts">
  import { Dialog } from '$lib/components/ui';
  import LaunchAdvanced from './launch/LaunchAdvanced.svelte';
  import { automaticAdversarialLaneCount } from './hiveLaunch';
  import { predefinedRoles, normalizeTierLadderPreview, type SessionMode, type WorkGraphParameterRow, type TierLadderPreviewCell, type TierLadderOmission } from './launch/launchForm';
  import './launch/launchForm.css';
  import { createEventDispatcher, tick } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import {
    fetchCliHealth,
    type CliHealthMap,
  } from './AgentConfigEditor.svelte';
  import {
    buildHiveLaunchConfig,
    createDefaultCodingPrincipal,
    createDefaultHiveFormState,
    nextCodingPrincipalIndex,
    type CodingPrincipalFormConfig,
    type TierRoutingPolicy,
  } from './hiveLaunch';
  import { routeFusionTemplateCells } from './templates/templateLaunch';
  import LaunchComposer from './launch/LaunchComposer.svelte';
  import type { AgentConfig, DebateDebaterConfig, DebateLaunchConfig, DelegationMode, FusionLaunchConfig, FusionVariantConfig, HiveLaunchConfig, QaWorkerConfig, ResearchLaunchConfig, SoloLaunchConfig, WorkerRole } from '$lib/stores/sessions';
  import type { SessionTemplate } from '$lib/types/domain';
  import { templates, selectedTemplate } from '$lib/stores/templates';
  import { defaultRoles } from '$lib/config/clis';
  import { apiUrl } from '$lib/config';

  export let show: boolean = false;
  export let launching: boolean = false;
  export let launchError: string = '';
  export let initialMode: SessionMode | undefined = undefined;
  export let presentation: 'modal' | 'inline' = 'modal';
  export let modeRequestId: number | undefined = undefined;
  export let defaultProjectPath: string = '';

  let handledModeRequestId: number | undefined;
  $: if (show && modeRequestId !== undefined && modeRequestId !== handledModeRequestId) {
    handledModeRequestId = modeRequestId;
    if (initialMode !== undefined) mode = initialMode;
    showAdvanced = false;
  }
  $: if (!projectPath && defaultProjectPath) projectPath = defaultProjectPath;
  $: if (show && presentation === 'inline' && !showAdvanced) {
    // Repeated requests focus the existing textarea, even when show stays true.
    modeRequestId;
    void tick().then(() => {
      if (show && presentation === 'inline' && !showAdvanced) {
        document.getElementById('launch-task')?.focus();
      }
    });
  }

  const dispatch = createEventDispatcher<{
    close: void;
    launchHive: HiveLaunchConfig;
    launchResearch: ResearchLaunchConfig;
    launchFusion: FusionLaunchConfig;
    launchSolo: SoloLaunchConfig;
    launchDebate: DebateLaunchConfig;
  }>();

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

  async function loadTierLadder(project: string) {
    if (!project || project === tierLadderLoadedProject) return;
    tierLadderLoadedProject = project;
    const requestId = ++tierLadderRequestId;
    tierLadderLoading = true;
    tierLadderError = null;
    tierLadderCells = [];
    tierLadderOmissions = [];

    try {
      const response = await fetch(
        apiUrl(`/api/tier-ladder?project_path=${encodeURIComponent(project)}`),
      );
      if (!response.ok) {
        throw new Error(`Tier ladder request failed (${response.status})`);
      }
      const preview = normalizeTierLadderPreview(await response.json());
      if (requestId !== tierLadderRequestId) return;
      tierLadderCells = preview.cells;
      tierLadderOmissions = preview.omissions;
    } catch (err) {
      if (requestId !== tierLadderRequestId) return;
      tierLadderError = err instanceof Error ? err.message : String(err);
    } finally {
      if (requestId === tierLadderRequestId) tierLadderLoading = false;
    }
  }

  function retryTierLadder() {
    tierLadderLoadedProject = '';
    void loadTierLadder(projectPath.trim());
  }

  let mode: SessionMode = 'hive';
  let projectPath = '';
  let sessionName = '';
  let sessionColor = '';
  let prompt = '';
  let error = '';
  let showPreview = false;
  let showAdvanced = false;
  let composerPreset = 'custom';
  $: composerWorkerCount = mode === 'hive' ? codingPrincipals.length
    : mode === 'research' ? researchWorkers.length
    : mode === 'fusion' ? variantCount
    : mode === 'debate' ? debaterCount : 1;
  $: composerMaxWorkers = mode === 'hive' ? Math.max(3, codingPrincipals.length)
    : mode === 'research' ? Math.max(6, researchWorkers.length)
    : mode === 'solo' || mode === 'templates' ? 1
    : Math.max(4, composerWorkerCount);

  function setComposerMode(next: string) {
    if (!['templates', 'hive', 'fusion', 'solo', 'research', 'debate'].includes(next)) return;
    // Solo's event contract uses a separate task field. Carry the edited task
    // across mode switches without changing that payload boundary.
    if (next === 'solo' && mode !== 'solo') soloTask = prompt;
    if (mode === 'solo' && next !== 'solo') prompt = soloTask;
    mode = next as SessionMode;
    composerPreset = 'custom';
    fusionTemplateError = '';
    error = '';
  }

  function setComposerWorkers(count: number) {
    if (!Number.isInteger(count) || count < 1 || count > composerMaxWorkers) return;
    if (mode === 'hive') {
      while (codingPrincipals.length < count) {
        codingPrincipals = [...codingPrincipals, createDefaultCodingPrincipal(nextCodingPrincipalIndex(codingPrincipals))];
      }
      codingPrincipals = codingPrincipals.slice(0, count);
    } else if (mode === 'research') {
      while (researchWorkers.length < count) researchWorkers = [...researchWorkers, createDefaultConfig('general')];
      researchWorkers = researchWorkers.slice(0, count);
    } else if (mode === 'fusion') {
      while (fusionVariants.length < count) {
        const name = `Variant ${String.fromCharCode(65 + fusionVariants.length)}`;
        fusionVariants = [...fusionVariants, { name, cli: 'claude', flags: [] }];
        variantAgentConfigs = [...variantAgentConfigs, { cli: 'claude', flags: [], label: name }];
      }
      variantCount = count;
    } else if (mode === 'debate') {
      while (debateDebaters.length < count) {
        const suffix = String.fromCharCode(65 + debateDebaters.length);
        debateDebaters = [...debateDebaters, { name: `Debater ${suffix}`, stance: `Stance ${suffix}`, cli: 'claude', flags: [] }];
        debaterAgentConfigs = [...debaterAgentConfigs, { cli: 'claude', flags: [], label: `Debater ${suffix}` }];
      }
      debaterCount = count;
    }
    composerPreset = 'custom';
  }

  function setComposerPreset(id: string) {
    if (id === 'custom') { composerPreset = id; return; }
    const template = $templates.templates.find((entry) => entry.id === id);
    if (template) applyTemplate(template);
  }

  // Solo config
  let soloConfig: AgentConfig = { cli: 'claude', flags: [], label: undefined };
  let soloTask = '';

  const defaultHiveForm = createDefaultHiveFormState();
  let queenConfig: AgentConfig = defaultHiveForm.queenConfig;
  let tierPolicy: TierRoutingPolicy = {
    ...defaultHiveForm.tierPolicy,
    ladder: {},
  };
  let tierLadderCells: TierLadderPreviewCell[] = [];
  let tierLadderOmissions: TierLadderOmission[] = [];
  let tierLadderLoading = false;
  let tierLadderError: string | null = null;
  let tierLadderLoadedProject = '';
  let tierLadderRequestId = 0;

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
  let workGraphArchetype = defaultHiveForm.workGraphArchetype ?? '';
  let nextWorkGraphParameterId = 0;
  let workGraphParameterRows: WorkGraphParameterRow[] = [];
  let showHiveAdvanced = false;
  $: tierProviders = Array.from(new Set([
    queenConfig.cli,
    ...codingPrincipals.map((principal) => principal.cli),
  ].filter(Boolean)));
  $: if (show && mode === 'hive' && tierPolicy.enabled && projectPath.trim()) {
    void loadTierLadder(projectPath.trim());
  }

  function parameterRowsFromRecord(parameters: Record<string, string>): WorkGraphParameterRow[] {
    return Object.entries(parameters).map(([key, value]) => ({
      id: nextWorkGraphParameterId++,
      key,
      value,
    }));
  }

  function workGraphParametersFromRows(): Record<string, string> {
    return Object.fromEntries(workGraphParameterRows.map(({ key, value }) => [key, value]));
  }

  function addWorkGraphParameter() {
    workGraphParameterRows = [
      ...workGraphParameterRows,
      { id: nextWorkGraphParameterId++, key: '', value: '' },
    ];
  }

  function removeWorkGraphParameter(id: number) {
    workGraphParameterRows = workGraphParameterRows.filter((parameter) => parameter.id !== id);
  }

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

    if (mode === 'solo') prompt = soloTask;
    composerPreset = template.id;
    if (!prompt.trim()) prompt = template.description;

    if (template.mode !== 'fusion') fusionTemplateError = '';

    sessionName = template.name;
    mode = template.mode as SessionMode;
    workspaceStrategy = template.workspace_strategy === 'isolated_cell' ? 'isolated_cell' : 'shared_cell';
    workGraphArchetype = template.mode === 'hive'
      ? template.work_graph_archetype?.trim() ?? ''
      : '';
    workGraphParameterRows = template.mode === 'hive'
      ? parameterRowsFromRecord(template.work_graph_parameters ?? {})
      : [];

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
  let withSoloEvaluator = true;
  $: if (workGraphArchetype.trim() && !withPlanning) withPlanning = true;
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

  function isValidTierRoutingCeiling(value: number): boolean {
    return Number.isInteger(value) && value >= 1 && value <= 100;
  }

  // The form is `novalidate` so the popup owns its own error reporting. That also
  // disables the browser's min/max enforcement on the delegation inputs, so those
  // bounds have to be checked here or they stop being enforced at all.
  function isValidDelegationLimit(value: number, max: number): boolean {
    return Number.isInteger(value) && value >= 1 && value <= max;
  }

  function handleTierRoutingToggle(event: Event) {
    const enabled = (event.currentTarget as HTMLInputElement).checked;
    if (!enabled && !isValidTierRoutingCeiling(tierPolicy.ceiling_percent)) {
      tierPolicy.ceiling_percent = 34;
    }
  }

  async function handleSubmit(smokeTest: boolean = false) {
    if (launching) return;
    if (!projectPath.trim()) return;
    if (mode === 'templates') {
      error = 'Choose a template before launching.';
      return;
    }

    error = '';

    if (
      mode === 'hive'
      && tierPolicy.enabled
      && !isValidTierRoutingCeiling(tierPolicy.ceiling_percent)
    ) {
      error = 'Tier-routing ceiling must be a whole number from 1 to 100.';
      return;
    }

    if (mode === 'hive') {
      const delegationLimits: Array<[string, number, number]> = [
        ['Queen target max children', queenMaxChildren, 8],
        ['Queen target max depth', queenMaxDepth, 4],
        ['Principal target max children', principalMaxChildren, 8],
        ['Principal target max depth', principalMaxDepth, 4],
      ];
      for (const [label, value, max] of delegationLimits) {
        if (!isValidDelegationLimit(value, max)) {
          error = `${label} must be a whole number from 1 to ${max}.`;
          return;
        }
      }
    }

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
          tierPolicy,
          workGraphArchetype: workGraphArchetype.trim() || null,
          workGraphParameters: workGraphParametersFromRows(),
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

  // Reset state when closed
  $: if (!show) healthLoadedForOpen = false;
  $: if (show && !healthLoadedForOpen) {
    healthLoadedForOpen = true;
    if (initialMode !== undefined) mode = initialMode;
    void loadCliHealth();
    void templates.fetchTemplates();
  }
  $: if (!show) {
    showAdvanced = false;
    error = '';
    tierLadderRequestId += 1;
    tierLadderLoadedProject = '';
    tierLadderCells = [];
    tierLadderOmissions = [];
    tierLadderLoading = false;
    tierLadderError = null;
  }
</script>

{#snippet content()}
  <div class="launch-content">
    {#if presentation === 'inline'}
      <p id="launch-dialog-title" class="launch-title">Hive Manager</p>
    {:else}
      <h2 id="launch-dialog-title">Launch New Session</h2>
    {/if}
    {#if showAdvanced}
<LaunchAdvanced
  {mode} bind:projectPath bind:sessionName
  bind:sessionColor bind:prompt bind:soloTask
  bind:error {launchError} bind:showPreview
  bind:showAdvanced bind:showHiveAdvanced {launching}
  bind:withPlanning bind:withEvaluator bind:withSoloEvaluator
  {tierLadderLoading} {presentation} bind:queenConfig
  bind:soloConfig bind:evaluatorConfig {judgeAgentConfig}
  {debateJudgeAgentConfig} bind:codingPrincipals bind:researchWorkers
  bind:qaWorkers bind:workspaceStrategy bind:queenDelegationMode
  bind:principalDelegationMode bind:queenMaxChildren bind:queenMaxDepth
  bind:principalMaxChildren bind:principalMaxDepth bind:variantCount
  bind:debaterCount bind:debateRounds {automaticAdversarialLanes}
  {adversarialLaneTarget} bind:workGraphArchetype bind:workGraphParameterRows
  bind:tierPolicy {tierLadderCells} {tierLadderOmissions}
  {tierLadderError} {cliHealthError} {tierProviders}
  {cliHealth} {cliHealthLoading} bind:fusionVariants
  {activeFusionVariants} bind:debateDebaters {activeDebaters}
  {variantAgentConfigs} {debaterAgentConfigs} {setComposerMode}
  {browseForFolder} {addCodingPrincipal} {addResearchWorker}
  {addQaWorker} {addWorkGraphParameter} {retryTierLadder}
  {handleSmokeTest} {handleClose} {removeCodingPrincipal}
  {removeResearchWorker} {removeQaWorker} {removeWorkGraphParameter}
  {handleSubmit} {handleTierRoutingToggle} {handleVariantConfigChange}
  {handleDebaterConfigChange} {handleJudgeConfigChange} {handleDebateJudgeConfigChange}
/>
    {:else}
        <LaunchComposer
          {mode}
          value={mode === 'solo' ? soloTask : prompt}
          onvalue={(value) => { if (mode === 'solo') soloTask = value; else prompt = value; }}
          {projectPath}
          workerCount={composerWorkerCount}
          maxWorkers={composerMaxWorkers}
          preset={composerPreset}
          templates={$templates.templates}
          templatesLoading={$templates.loading}
          templatesError={$templates.error}
          {launching}
          error={error || launchError}
          onmode={setComposerMode}
          onworkers={setComposerWorkers}
          onpreset={setComposerPreset}
          onbrowse={browseForFolder}
          onadvanced={() => showAdvanced = true}
          onlaunch={() => handleSubmit(false)}
          showQa={mode === 'hive' || mode === 'solo'}
          qaEnabled={mode === 'solo' ? withSoloEvaluator : withEvaluator}
          qaSummary={(mode === 'solo' ? withSoloEvaluator : withEvaluator) ? `QA: on · Evaluator + ${qaWorkers.length} (${qaWorkers.map((worker) => worker.specialization.toUpperCase()).join(', ')})` : 'QA: off'}
          qaCaveat="Gates milestones before push. UI and A11Y workers need a rendered browser; for desktop-only projects, select API or other terminal-checkable QA workers."
          onqa={(on) => { if (mode === 'solo') withSoloEvaluator = on; else withEvaluator = on; }}
          showSmoke={mode === 'hive' || mode === 'research'}
          onsmoke={handleSmokeTest}
          oncancel={presentation === 'inline' ? undefined : handleClose}
        />

    {/if}
  </div>
{/snippet}

{#if show}
  {#if presentation === 'inline'}
    <section class="launch-inline" aria-labelledby="launch-dialog-title">
      {@render content()}
    </section>
  {:else}
    <!-- Keep the modal in place to preserve the frozen launch-parity container contract. -->
    <Dialog open={show} onOpenChange={(open) => { if (!open) handleClose(); }} portal={false} dismissible={!launching} labelledby="launch-dialog-title" class="launch-dialog">
      {@render content()}
    </Dialog>
  {/if}
{/if}
