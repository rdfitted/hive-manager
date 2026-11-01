<script lang="ts">
  import type { AgentConfig, QaWorkerConfig } from '$lib/stores/sessions';
  import type { CliHealthMap } from '../AgentConfigEditor.svelte';
  import { type SessionMode } from './launchForm';
  import AgentConfigEditor from '../AgentConfigEditor.svelte';
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let mode: SessionMode;
  export let withPlanning: boolean;
  export let withEvaluator: boolean;
  export let withSoloEvaluator: boolean;
  export let evaluatorConfig: AgentConfig;
  export let qaWorkers: QaWorkerConfig[];
  export let workGraphArchetype: string;
  export let cliHealthError: string | null;
  export let cliHealth: CliHealthMap;
  export let cliHealthLoading: boolean;
  export let addQaWorker: () => void;
  export let removeQaWorker: (index: number) => void;</script>

{#if mode === 'hive' || mode === 'debate' || mode === 'fusion' || mode === 'solo'}
          <div data-launch-owned class="form-section">
            <h3 data-launch-owned>Orchestration Options</h3>
            {#if mode === 'hive' || mode === 'debate'}
              <div data-launch-owned class="checkbox-group">
                <label data-launch-owned class="checkbox-label lattice-selectable">
                  <input data-launch-owned type="checkbox" bind:checked={withPlanning} disabled={Boolean(workGraphArchetype.trim())} />
                  <div data-launch-owned class="checkbox-text">
                    <span data-launch-owned class="checkbox-title">Enable Planning Phase</span>
                    <span data-launch-owned class="checkbox-description">
                      {workGraphArchetype.trim()
                        ? 'Required so the Master Planner starts from the selected work-graph skeleton.'
                        : 'Create a scoped plan before implementation begins.'}
                    </span>
                  </div>
                </label>
              </div>
            {:else if mode === 'fusion'}
              <p data-launch-owned class="policy-note">Fusion includes planning so candidate Hives start from the same objective.</p>
            {/if}

            {#if mode === 'hive' || mode === 'solo'}
              <div data-launch-owned class="checkbox-group">
                <label data-launch-owned class="checkbox-label lattice-selectable">
                  {#if mode === 'solo'}
                    <input data-launch-owned type="checkbox" bind:checked={withSoloEvaluator} />
                  {:else}
                    <input data-launch-owned type="checkbox" bind:checked={withEvaluator} />
                  {/if}
                  <div data-launch-owned class="checkbox-text">
                    <span data-launch-owned class="checkbox-title">QA: Evaluator + QA workers</span>
                    <span data-launch-owned class="checkbox-description">Gates milestones before push. UI and A11Y workers need a rendered browser; for desktop-only projects, select API or other terminal-checkable QA workers.</span>
                  </div>
                </label>
              </div>
              {#if (mode === 'hive' && withEvaluator) || (mode === 'solo' && withSoloEvaluator)}
                <div data-launch-owned class="evaluator-config subsection lattice-forced-colors-boundary">
                  <h4 data-launch-owned>Evaluator Configuration</h4>
                  <AgentConfigEditor
                    bind:config={evaluatorConfig}
                    showLabel={true}
                    idPrefix="launch-evaluator"
                    {cliHealth}
                    {cliHealthLoading}
                    {cliHealthError}
                  />
                </div>

                <div data-launch-owned class="qa-workers-config subsection lattice-forced-colors-boundary">
                  <div data-launch-owned class="section-header">
                    <h4 data-launch-owned>QA Workers ({qaWorkers.length})</h4>
                    <button data-launch-owned type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact lattice-btn--dashed" on:click={addQaWorker} disabled={qaWorkers.length >= 6}>+ Add</button>
                  </div>
                  <div data-launch-owned class="workers-list">
                    {#each qaWorkers as worker, i (i)}
                      <div data-launch-owned class="worker-card qa-worker-card lattice-panel">
                        <div data-launch-owned class="card-header">
                          <span data-launch-owned class="card-title">QA Worker {i + 1}</span>
                          <button data-launch-owned
                            type="button"
                            class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--compact"
                            on:click={() => removeQaWorker(i)}
                            disabled={qaWorkers.length <= 1}
                          >Remove</button>
                        </div>
                        <div data-launch-owned class="role-selector small">
                          <label data-launch-owned for="qa-spec-{i}">Specialization</label>
                          <select data-launch-owned id="qa-spec-{i}" bind:value={worker.specialization} class="role-select lattice-input">
                            <option data-launch-owned value="ui">UI Tester</option>
                            <option data-launch-owned value="api">API Tester</option>
                            <option data-launch-owned value="a11y">A11Y Tester</option>
                            <option data-launch-owned value="adversarial">Adversarial</option>
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
