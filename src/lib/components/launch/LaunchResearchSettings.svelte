<script lang="ts">
  import type { CodingPrincipalFormConfig } from '../hiveLaunch';
  import type { CliHealthMap } from '../AgentConfigEditor.svelte';
  import AgentConfigEditor from '../AgentConfigEditor.svelte';
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let researchWorkers: CodingPrincipalFormConfig[];
  export let cliHealthError: string | null;
  export let cliHealth: CliHealthMap;
  export let cliHealthLoading: boolean;
  export let addResearchWorker: () => void;
  export let removeResearchWorker: (index: number) => void;</script>

<div class="form-section">
            <div class="section-header">
              <h3>Researcher Roster ({researchWorkers.length})</h3>
              <button type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact lattice-btn--dashed" on:click={addResearchWorker} disabled={researchWorkers.length >= 6}>
                + Add
              </button>
            </div>
            <p class="section-description">A roster the Queen spawns from on demand — none launch up front. Each runs with its own selectable model; the Queen decides how many to spawn based on the objective.</p>
            <div class="workers-list">
              {#each researchWorkers as worker, i (i)}
                <div class="worker-card lattice-panel">
                  <div class="card-header">
                    <span class="card-title">Researcher {i + 1}</span>
                    <button
                      type="button"
                      class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--compact"
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
