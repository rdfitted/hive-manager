<script lang="ts">
  import type { AgentConfig } from '$lib/stores/sessions';
  import type { CliHealthMap } from '../AgentConfigEditor.svelte';
  import AgentConfigEditor from '../AgentConfigEditor.svelte';
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let soloTask: string;
  export let soloConfig: AgentConfig;
  export let cliHealthError: string | null;
  export let cliHealth: CliHealthMap;
  export let cliHealthLoading: boolean;</script>

<div data-launch-owned class="form-section">
            <h3 data-launch-owned>Solo Configuration</h3>
            <p data-launch-owned class="section-description">Run a single agent for a specific task without any orchestration overhead.</p>

            <AgentConfigEditor
              bind:config={soloConfig}
              showLabel={false}
              idPrefix="launch-solo"
              {cliHealth}
              {cliHealthLoading}
              {cliHealthError}
            />

            <div data-launch-owned class="form-group">
              <label data-launch-owned for="solo-task">Task Description</label>
            <textarea data-launch-owned
              id="solo-task"
              class="lattice-input"
                bind:value={soloTask}
                placeholder="What should the agent do? (Leave empty for interactive mode)"
                rows="5"
              ></textarea>
            </div>
          </div>
