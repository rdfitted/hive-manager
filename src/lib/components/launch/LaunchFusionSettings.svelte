<script lang="ts">
  import type { AgentConfig, FusionVariantConfig } from '$lib/stores/sessions';
  import type { CliHealthMap } from '../AgentConfigEditor.svelte';
  import AgentConfigEditor from '../AgentConfigEditor.svelte';
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let judgeAgentConfig: AgentConfig;
  export let variantCount: number;
  export let cliHealthError: string | null;
  export let cliHealth: CliHealthMap;
  export let cliHealthLoading: boolean;
  export let fusionVariants: FusionVariantConfig[];
  export let activeFusionVariants: FusionVariantConfig[];
  export let variantAgentConfigs: AgentConfig[];
  export let handleVariantConfigChange: (index: number, detail: AgentConfig) => void;
  export let handleJudgeConfigChange: (detail: AgentConfig) => void;</script>

<div class="form-section">
            <h3>Fusion Configuration</h3>
            <p class="section-description">Run multiple agent variants in parallel to compare their outputs. A judge will evaluate and recommend the best result.</p>

            <div class="field">
              <label for="variant-count">Number of Variants</label>
              <select id="variant-count" bind:value={variantCount} class="role-select lattice-input">
                <option value={2}>2 Variants</option>
                <option value={3}>3 Variants</option>
                <option value={4}>4 Variants</option>
              </select>
            </div>

            <div class="subsection lattice-forced-colors-boundary">
              <h4>Variant Configurations</h4>
              <div class="workers-list">
                {#each activeFusionVariants as variant, i (i)}
                  <div class="worker-card lattice-panel">
                    <div class="card-header">
                      <input
                        type="text"
                        class="card-title-input lattice-input"
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

            <div class="subsection lattice-forced-colors-boundary">
              <h4>Judge Configuration</h4>
              <p class="section-description">Evaluates variant outputs and recommends a winner.</p>
              <div class="worker-card lattice-panel">
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
