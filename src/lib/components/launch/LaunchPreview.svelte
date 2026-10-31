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
        <div data-launch-owned class="launch-preview-section lattice-forced-colors-boundary">
          <button data-launch-owned
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
            <div data-launch-owned class="preview-content" id="launch-preview-content">
              <div data-launch-owned class="topology-viz">
                {#if mode === 'solo'}
                  <div data-launch-owned class="node solo lattice-panel">
                    <span data-launch-owned class="node-label">Solo</span>
                    <span data-launch-owned class="node-cli">{soloConfig.cli} · {soloConfig.model || 'default'}</span>
                  </div>
                  {#if withSoloEvaluator}
                    <div data-launch-owned class="connector"></div>
                    <div data-launch-owned class="worker-nodes">
                      <div data-launch-owned class="node worker lattice-panel">
                        <span data-launch-owned class="node-label">Evaluator + Prince</span>
                        <span data-launch-owned class="node-cli">verification control plane and QA on demand</span>
                      </div>
                    </div>
                  {/if}
                {:else}
                  <div data-launch-owned class="node queen lattice-panel">
                    <span data-launch-owned class="node-icon">
                      <Crown size={16} weight="light" />
                    </span>
                    <span data-launch-owned class="node-label">Queen</span>
                    <span data-launch-owned class="node-cli">{queenConfig.cli} · {queenConfig.model || 'default'}</span>
                  </div>
                  {#if mode !== 'research'}
                    <div data-launch-owned class="connector"></div>
                    <div data-launch-owned class="worker-nodes">
                      {#if mode === 'hive'}
                        {#each codingPrincipals as principal}
                          <div data-launch-owned class="node worker lattice-panel">
                            <span data-launch-owned class="node-label">{principal.label || 'Coding Principal'}</span>
                            <span data-launch-owned class="node-cli">{principal.cli} · {principal.model || 'default'}</span>
                          </div>
                        {/each}
                      {:else if mode === 'fusion'}
                        {#each activeFusionVariants as variant}
                          <div data-launch-owned class="node fusion lattice-panel">
                            <span data-launch-owned class="node-label">{variant.name}</span>
                            <span data-launch-owned class="node-cli">{variant.cli}</span>
                          </div>
                        {/each}
                      {:else if mode === 'debate'}
                        {#each activeDebaters as debater}
                          <div data-launch-owned class="node debate lattice-panel">
                            <span data-launch-owned class="node-label">{debater.name}</span>
                            <span data-launch-owned class="node-cli">{debater.cli}</span>
                          </div>
                        {/each}
                      {/if}
                    </div>
                  {/if}
                {/if}

                {#if mode === 'hive'}
                  <div data-launch-owned class="topology-contract lattice-forced-colors-boundary">
                    <div data-launch-owned class="topology-layer lattice-forced-colors-boundary">
                      <span data-launch-owned class="topology-layer-title">Managed principals</span>
                      <span data-launch-owned>1 Queen + {codingPrincipals.length} manager-launched coding principal{codingPrincipals.length === 1 ? '' : 's'}</span>
                      <span data-launch-owned>Workspace: {workspaceStrategy === 'shared_cell' ? 'one shared cell' : 'one Queen worktree plus one per principal'}</span>
                    </div>
                    {#if withEvaluator}
                      <div data-launch-owned class="topology-layer lattice-forced-colors-boundary">
                        <span data-launch-owned class="topology-layer-title">Verification control plane</span>
                        <span data-launch-owned>Evaluator + Prince peers; {qaWorkers.length} configured QA role{qaWorkers.length === 1 ? '' : 's'}, plus {automaticAdversarialLanes} missing adversarial lane{automaticAdversarialLanes === 1 ? '' : 's'} added automatically to reach the {adversarialLaneTarget}-lane target.</span>
                      </div>
                    {/if}
                    <div data-launch-owned class="topology-layer lattice-forced-colors-boundary">
                      <span data-launch-owned class="topology-layer-title">Potential native children</span>
                      <span data-launch-owned>Queen: {delegationModeLabel(queenDelegationMode)} · Principals: {delegationModeLabel(principalDelegationMode)}</span>
                      <span data-launch-owned>Harness-native children stay inside their parent’s assignment; they are not additional managed principals.</span>
                    </div>
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        </div>
        {/if}
