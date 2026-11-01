<script lang="ts">
  import type { AgentConfig, DebateDebaterConfig } from '$lib/stores/sessions';
  import type { CliHealthMap } from '../AgentConfigEditor.svelte';
  import AgentConfigEditor from '../AgentConfigEditor.svelte';
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let debateJudgeAgentConfig: AgentConfig;
  export let debaterCount: number;
  export let debateRounds: number;
  export let cliHealthError: string | null;
  export let cliHealth: CliHealthMap;
  export let cliHealthLoading: boolean;
  export let debateDebaters: DebateDebaterConfig[];
  export let activeDebaters: DebateDebaterConfig[];
  export let debaterAgentConfigs: AgentConfig[];
  export let handleDebaterConfigChange: (index: number, detail: AgentConfig) => void;
  export let handleDebateJudgeConfigChange: (detail: AgentConfig) => void;</script>

<div data-launch-owned class="form-section">
            <h3 data-launch-owned>Debate Configuration</h3>
            <p data-launch-owned class="section-description">Run multiple debaters in parallel to argue different perspectives across multiple rounds. A judge evaluates and renders the verdict.</p>

            <div data-launch-owned class="form-row">
              <div data-launch-owned class="field">
                <label data-launch-owned for="debater-count">Number of Debaters</label>
                <select data-launch-owned id="debater-count" bind:value={debaterCount} class="role-select lattice-input">
                  <option data-launch-owned value={2}>2 Debaters</option>
                  <option data-launch-owned value={3}>3 Debaters</option>
                  <option data-launch-owned value={4}>4 Debaters</option>
                </select>
              </div>

              <div data-launch-owned class="field">
                <label data-launch-owned for="debate-rounds">Number of Rounds</label>
                <select data-launch-owned id="debate-rounds" bind:value={debateRounds} class="role-select lattice-input">
                  <option data-launch-owned value={1}>1 Round</option>
                  <option data-launch-owned value={2}>2 Rounds</option>
                  <option data-launch-owned value={3}>3 Rounds</option>
                  <option data-launch-owned value={4}>4 Rounds</option>
                  <option data-launch-owned value={5}>5 Rounds</option>
                </select>
              </div>
            </div>

            <div data-launch-owned class="subsection lattice-forced-colors-boundary">
              <h4 data-launch-owned>Debater Configurations</h4>
              <div data-launch-owned class="workers-list">
                {#each activeDebaters as debater, i (i)}
                  <div data-launch-owned class="worker-card lattice-panel">
                    <div data-launch-owned class="card-header" style="display: flex; gap: 8px; flex-direction: column; align-items: stretch; border: none; padding: 0; margin-bottom: 8px;">
                      <input data-launch-owned
                        type="text"
                        class="card-title-input lattice-input"
                        bind:value={debateDebaters[i].name}
                        placeholder="Debater {String.fromCharCode(65 + i)}"
                        style="width: 100%;"
                      />
                      <input data-launch-owned
                        type="text"
                        class="card-title-input lattice-input"
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

            <div data-launch-owned class="subsection lattice-forced-colors-boundary">
              <h4 data-launch-owned>Judge Configuration</h4>
              <p data-launch-owned class="section-description">Evaluates the debate and renders the verdict.</p>
              <div data-launch-owned class="worker-card lattice-panel">
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
