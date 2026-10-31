<script lang="ts">
  import type { AgentConfig, DebateDebaterConfig, DelegationMode, FusionVariantConfig, QaWorkerConfig } from '$lib/stores/sessions';
  import type { CodingPrincipalFormConfig, TierRoutingPolicy } from '../hiveLaunch';
  import type { CliHealthMap } from '../AgentConfigEditor.svelte';
  import { COLORS, type SessionMode, type WorkGraphParameterRow, type TierLadderPreviewCell, type TierLadderOmission } from './launchForm';
  import AgentConfigEditor from '../AgentConfigEditor.svelte';
  import TemplatePicker from '../templates/TemplatePicker.svelte';
  import Composer from '../composer/Composer.svelte';
  import LaunchHiveSettings from './LaunchHiveSettings.svelte';
  import LaunchOrchestration from './LaunchOrchestration.svelte';
  import LaunchResearchSettings from './LaunchResearchSettings.svelte';
  import LaunchFusionSettings from './LaunchFusionSettings.svelte';
  import LaunchDebateSettings from './LaunchDebateSettings.svelte';
  import LaunchSoloSettings from './LaunchSoloSettings.svelte';
  import LaunchPreview from './LaunchPreview.svelte';
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let mode: SessionMode;
  export let projectPath: string;
  export let sessionName: string;
  export let sessionColor: string;
  export let prompt: string;
  export let soloTask: string;
  export let error: string;
  export let launchError: string;
  export let showPreview: boolean;
  export let showAdvanced: boolean;
  export let showHiveAdvanced: boolean;
  export let launching: boolean;
  export let withPlanning: boolean;
  export let withEvaluator: boolean;
  export let withSoloEvaluator: boolean;
  export let tierLadderLoading: boolean;
  export let presentation: 'modal' | 'inline';
  export let queenConfig: AgentConfig;
  export let soloConfig: AgentConfig;
  export let evaluatorConfig: AgentConfig;
  export let judgeAgentConfig: AgentConfig;
  export let debateJudgeAgentConfig: AgentConfig;
  export let codingPrincipals: CodingPrincipalFormConfig[];
  export let researchWorkers: CodingPrincipalFormConfig[];
  export let qaWorkers: QaWorkerConfig[];
  export let workspaceStrategy: 'shared_cell' | 'isolated_cell';
  export let queenDelegationMode: DelegationMode;
  export let principalDelegationMode: DelegationMode;
  export let queenMaxChildren: number;
  export let queenMaxDepth: number;
  export let principalMaxChildren: number;
  export let principalMaxDepth: number;
  export let variantCount: number;
  export let debaterCount: number;
  export let debateRounds: number;
  export let automaticAdversarialLanes: number;
  export let adversarialLaneTarget: number;
  export let workGraphArchetype: string;
  export let workGraphParameterRows: WorkGraphParameterRow[];
  export let tierPolicy: TierRoutingPolicy;
  export let tierLadderCells: TierLadderPreviewCell[];
  export let tierLadderOmissions: TierLadderOmission[];
  export let tierLadderError: string | null;
  export let cliHealthError: string | null;
  export let tierProviders: string[];
  export let cliHealth: CliHealthMap;
  export let cliHealthLoading: boolean;
  export let fusionVariants: FusionVariantConfig[];
  export let activeFusionVariants: FusionVariantConfig[];
  export let debateDebaters: DebateDebaterConfig[];
  export let activeDebaters: DebateDebaterConfig[];
  export let variantAgentConfigs: AgentConfig[];
  export let debaterAgentConfigs: AgentConfig[];
  export let setComposerMode: (next: string) => void;
  export let browseForFolder: () => void;
  export let addCodingPrincipal: () => void;
  export let addResearchWorker: () => void;
  export let addQaWorker: () => void;
  export let addWorkGraphParameter: () => void;
  export let retryTierLadder: () => void;
  export let handleSmokeTest: () => void;
  export let handleClose: () => void;
  export let removeCodingPrincipal: (index: number) => void;
  export let removeResearchWorker: (index: number) => void;
  export let removeQaWorker: (index: number) => void;
  export let removeWorkGraphParameter: (index: number) => void;
  export let handleSubmit: (smokeTest?: boolean) => void;
  export let handleTierRoutingToggle: (event: Event) => void;
  export let handleVariantConfigChange: (index: number, detail: AgentConfig) => void;
  export let handleDebaterConfigChange: (index: number, detail: AgentConfig) => void;
  export let handleJudgeConfigChange: (detail: AgentConfig) => void;
  export let handleDebateJudgeConfigChange: (detail: AgentConfig) => void;</script>

<button type="button" class="lattice-btn lattice-btn--ghost" on:click={() => showAdvanced = false} disabled={launching}>Back to composer</button>
      <div class="mode-tabs">
        <button
          class="lattice-tab launch-mode-tab"
          class:lattice-tab--active={mode === 'templates'}
          aria-pressed={mode === 'templates'}
          on:click={() => setComposerMode('templates')}
          type="button"
        >
          Templates
        </button>
        <button
          class="lattice-tab launch-mode-tab"
          class:lattice-tab--active={mode === 'hive'}
          aria-pressed={mode === 'hive'}
          on:click={() => setComposerMode('hive')}
          type="button"
        >
          Hive
        </button>
        <button
          class="lattice-tab launch-mode-tab"
          class:lattice-tab--active={mode === 'fusion'}
          aria-pressed={mode === 'fusion'}
          on:click={() => setComposerMode('fusion')}
          type="button"
        >
          Fusion
        </button>
        <button
          class="lattice-tab launch-mode-tab"
          class:lattice-tab--active={mode === 'debate'}
          aria-pressed={mode === 'debate'}
          on:click={() => setComposerMode('debate')}
          type="button"
        >
          Debate
        </button>
        <button
          class="lattice-tab launch-mode-tab"
          class:lattice-tab--active={mode === 'research'}
          aria-pressed={mode === 'research'}
          on:click={() => setComposerMode('research')}
          type="button"
        >
          Research
        </button>
        <button
          class="lattice-tab launch-mode-tab"
          class:lattice-tab--active={mode === 'solo'}
          aria-pressed={mode === 'solo'}
          on:click={() => setComposerMode('solo')}
          type="button"
        >
          Solo
        </button>
      </div>

      <form novalidate on:submit={(e) => { e.preventDefault(); handleSubmit(false); }}>
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
              class="lattice-input"
              bind:value={sessionName}
              placeholder="e.g. Refactor API"
            />
          </div>
          <div class="form-group flex-1">
            <span class="group-label">Session Color</span>
            <div class="color-picker-inline lattice-forced-colors-boundary">
              {#each COLORS as color}
                <button
                  type="button"
                  class="lattice-btn lattice-btn--icon color-circle"
                  style:background={color.value}
                  aria-pressed={sessionColor === color.value}
                  on:click={() => sessionColor = color.value}
                  title={color.name}
                >
                </button>
              {/each}
              <button
                type="button"
                class="lattice-btn lattice-btn--icon lattice-btn--secondary color-circle clear"
                on:click={() => sessionColor = ''}
                title="Clear color"
                aria-label="Clear color"
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
              class="lattice-input"
              bind:value={projectPath}
              placeholder="Select a project folder..."
              readonly
              required
            />
            <button type="button" class="lattice-btn lattice-btn--secondary" on:click={browseForFolder}>
              Browse
            </button>
          </div>
        </div>

        {#if mode === 'hive' || mode === 'research' || mode === 'fusion' || mode === 'debate'}
          <div class="form-section queen-section lattice-forced-colors-boundary">
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

<LaunchHiveSettings
  {mode} {projectPath}
  bind:showHiveAdvanced {tierLadderLoading} bind:codingPrincipals
  bind:workspaceStrategy bind:queenDelegationMode bind:principalDelegationMode
  bind:queenMaxChildren bind:queenMaxDepth bind:principalMaxChildren
  bind:principalMaxDepth bind:workGraphArchetype bind:workGraphParameterRows
  bind:tierPolicy {tierLadderCells} {tierLadderOmissions}
  {tierLadderError} {cliHealthError} {tierProviders}
  {cliHealth} {cliHealthLoading} {addCodingPrincipal}
  {addWorkGraphParameter} {retryTierLadder} {removeCodingPrincipal}
  {removeWorkGraphParameter} {handleTierRoutingToggle}
/>

<LaunchOrchestration
  {mode} bind:withPlanning bind:withEvaluator
  bind:withSoloEvaluator bind:evaluatorConfig bind:qaWorkers
  bind:workGraphArchetype {cliHealthError} {cliHealth}
  {cliHealthLoading} {addQaWorker} {removeQaWorker}
/>

{#if mode === 'research'}
<LaunchResearchSettings
  bind:researchWorkers {cliHealthError} {cliHealth}
  {cliHealthLoading} {addResearchWorker} {removeResearchWorker}
/>
{:else if mode === 'fusion'}
<LaunchFusionSettings
  {judgeAgentConfig} bind:variantCount {cliHealthError}
  {cliHealth} {cliHealthLoading} bind:fusionVariants
  {activeFusionVariants} {variantAgentConfigs} {handleVariantConfigChange}
  {handleJudgeConfigChange}
/>
{:else if mode === 'debate'}
<LaunchDebateSettings
  {debateJudgeAgentConfig} bind:debaterCount bind:debateRounds
  {cliHealthError} {cliHealth} {cliHealthLoading}
  bind:debateDebaters {activeDebaters} {debaterAgentConfigs}
  {handleDebaterConfigChange} {handleDebateJudgeConfigChange}
/>
{:else if mode === 'solo'}
<LaunchSoloSettings
  bind:soloTask bind:soloConfig
  {cliHealthError} {cliHealth} {cliHealthLoading}
/>
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
          <div class="error-message lattice-forced-colors-boundary" role="alert">{error || launchError}</div>
        {/if}

<LaunchPreview
  {mode} bind:showPreview bind:withEvaluator
  bind:withSoloEvaluator bind:queenConfig bind:soloConfig
  bind:codingPrincipals bind:qaWorkers bind:workspaceStrategy
  bind:queenDelegationMode bind:principalDelegationMode {automaticAdversarialLanes}
  {adversarialLaneTarget} {activeFusionVariants} {activeDebaters}
/>

        <div class="dialog-actions">
          {#if presentation === 'modal'}
          <button type="button" class="lattice-btn lattice-btn--secondary" on:click={handleClose} disabled={launching}>
            Cancel
          </button>
          {/if}
          {#if mode === 'hive' || mode === 'research'}
          <button
            type="button"
            class="lattice-btn lattice-btn--warning lattice-btn--dashed"
            on:click={handleSmokeTest}
            disabled={launching || !projectPath.trim()}
            title={mode === 'research'
              ? 'Quick smoke test: the Queen spawns one researcher with a trivial task and reports back — no wiki load or capture'
              : 'Quick test to validate the entire flow: planning phase, task check-off, and agent spawning'}
          >
            Smoke Test
          </button>
          {/if}
          <button type="submit" class="lattice-btn lattice-btn--primary lattice-btn--filled" disabled={launching || !projectPath.trim() || mode === 'templates'} aria-busy={launching}>
            {launching ? 'Launching...' : 'Launch'}
          </button>
        </div>
      </form>
