<script lang="ts">
  import type { AgentConfig, DebateDebaterConfig, DelegationMode, FusionVariantConfig, QaWorkerConfig } from '$lib/stores/sessions';
  import type { CodingPrincipalFormConfig } from '../hiveLaunch';
  import { type SessionMode } from './launchForm';
  import { Crown, CaretDown, CaretRight } from 'phosphor-svelte';
  function delegationModeLabel(value: DelegationMode): string {
    return value === 'disabled' ? 'Disabled' : value === 'encouraged' ? 'Encouraged' : 'Automatic when supported';
  }
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let mode: SessionMode;
  export let showPreview: boolean;
  export let withEvaluator: boolean;
  export let withSoloEvaluator: boolean;
  export let queenConfig: AgentConfig;
  export let soloConfig: AgentConfig;
  export let codingPrincipals: CodingPrincipalFormConfig[];
  export let qaWorkers: QaWorkerConfig[];
  export let workspaceStrategy: 'shared_cell' | 'isolated_cell';
  export let queenDelegationMode: DelegationMode;
  export let principalDelegationMode: DelegationMode;
  export let automaticAdversarialLanes: number;
  export let adversarialLaneTarget: number;
  export let activeFusionVariants: FusionVariantConfig[];
  export let activeDebaters: DebateDebaterConfig[];</script>

{#if mode !== 'templates'}
        <div class="launch-preview-section lattice-forced-colors-boundary">
          <button
            type="button"
            class="lattice-btn lattice-btn--ghost lattice-btn--link"
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
                  <div class="node solo lattice-panel">
                    <span class="node-label">Solo</span>
                    <span class="node-cli">{soloConfig.cli} · {soloConfig.model || 'default'}</span>
                  </div>
                  {#if withSoloEvaluator}
                    <div class="connector"></div>
                    <div class="worker-nodes">
                      <div class="node worker lattice-panel">
                        <span class="node-label">Evaluator + Prince</span>
                        <span class="node-cli">verification control plane and QA on demand</span>
                      </div>
                    </div>
                  {/if}
                {:else}
                  <div class="node queen lattice-panel">
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
                          <div class="node worker lattice-panel">
                            <span class="node-label">{principal.label || 'Coding Principal'}</span>
                            <span class="node-cli">{principal.cli} · {principal.model || 'default'}</span>
                          </div>
                        {/each}
                      {:else if mode === 'fusion'}
                        {#each activeFusionVariants as variant}
                          <div class="node fusion lattice-panel">
                            <span class="node-label">{variant.name}</span>
                            <span class="node-cli">{variant.cli}</span>
                          </div>
                        {/each}
                      {:else if mode === 'debate'}
                        {#each activeDebaters as debater}
                          <div class="node debate lattice-panel">
                            <span class="node-label">{debater.name}</span>
                            <span class="node-cli">{debater.cli}</span>
                          </div>
                        {/each}
                      {/if}
                    </div>
                  {/if}
                {/if}

                {#if mode === 'hive'}
                  <div class="topology-contract lattice-forced-colors-boundary">
                    <div class="topology-layer lattice-forced-colors-boundary">
                      <span class="topology-layer-title">Managed principals</span>
                      <span>1 Queen + {codingPrincipals.length} manager-launched coding principal{codingPrincipals.length === 1 ? '' : 's'}</span>
                      <span>Workspace: {workspaceStrategy === 'shared_cell' ? 'one shared cell' : 'one Queen worktree plus one per principal'}</span>
                    </div>
                    {#if withEvaluator}
                      <div class="topology-layer lattice-forced-colors-boundary">
                        <span class="topology-layer-title">Verification control plane</span>
                        <span>Evaluator + Prince peers; {qaWorkers.length} configured QA role{qaWorkers.length === 1 ? '' : 's'}, plus {automaticAdversarialLanes} missing adversarial lane{automaticAdversarialLanes === 1 ? '' : 's'} added automatically to reach the {adversarialLaneTarget}-lane target.</span>
                      </div>
                    {/if}
                    <div class="topology-layer lattice-forced-colors-boundary">
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
