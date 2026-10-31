<script lang="ts">
  import type { DelegationMode } from '$lib/stores/sessions';
  import type { CodingPrincipalFormConfig, TierRoutingPolicy } from '../hiveLaunch';
  import type { CliHealthMap } from '../AgentConfigEditor.svelte';
  import { WORK_GRAPH_ARCHETYPES, TASK_TIERS, predefinedRoles, providerLabel, type SessionMode, type WorkGraphParameterRow, type TierLadderPreviewCell, type TierLadderOmission } from './launchForm';
  import AgentConfigEditor from '../AgentConfigEditor.svelte';
  // Values are bound to LaunchDialog; mounting this section never creates defaults.
  export let mode: SessionMode;
  export let projectPath: string;
  export let showHiveAdvanced: boolean;
  export let tierLadderLoading: boolean;
  export let codingPrincipals: CodingPrincipalFormConfig[];
  export let workspaceStrategy: 'shared_cell' | 'isolated_cell';
  export let queenDelegationMode: DelegationMode;
  export let principalDelegationMode: DelegationMode;
  export let queenMaxChildren: number;
  export let queenMaxDepth: number;
  export let principalMaxChildren: number;
  export let principalMaxDepth: number;
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
  export let addCodingPrincipal: () => void;
  export let addWorkGraphParameter: () => void;
  export let retryTierLadder: () => void;
  export let removeCodingPrincipal: (index: number) => void;
  export let removeWorkGraphParameter: (index: number) => void;
  export let handleTierRoutingToggle: (event: Event) => void;</script>

{#if mode === 'hive'}
          <div class="form-section hive-policy-section lattice-forced-colors-boundary">
            <div class="section-heading-copy">
              <h3>Hive Topology</h3>
              <p>Keep the team legible: a Queen, a small set of coding principals, and bounded native delegation.</p>
            </div>

            <fieldset class="policy-fieldset">
              <legend>Workspace strategy</legend>
              <div class="choice-grid">
                <label
                  class="choice-card lattice-panel lattice-selectable"
                  class:lattice-selectable--selected={workspaceStrategy === 'shared_cell'}
                >
                  <input type="radio" name="workspace-strategy" bind:group={workspaceStrategy} value="shared_cell" />
                  <span class="choice-title">Shared cell</span>
                  <span class="choice-description">Queen and principals collaborate in one workspace.</span>
                </label>
                <label
                  class="choice-card lattice-panel lattice-selectable"
                  class:lattice-selectable--selected={workspaceStrategy === 'isolated_cell'}
                >
                  <input type="radio" name="workspace-strategy" bind:group={workspaceStrategy} value="isolated_cell" />
                  <span class="choice-title">Per-principal worktrees</span>
                  <span class="choice-description">Queen and each visible principal get isolated managed worktrees.</span>
                </label>
              </div>
            </fieldset>

            <fieldset class="policy-fieldset">
              <legend>Work-graph archetype (optional)</legend>
              <div class="field">
                <label for="work-graph-archetype">Archetype</label>
                <input
                  id="work-graph-archetype"
                  class="lattice-input"
                  list="work-graph-archetypes"
                  bind:value={workGraphArchetype}
                  placeholder="None — preserve the planner's existing topology"
                />
                <datalist id="work-graph-archetypes">
                  {#each WORK_GRAPH_ARCHETYPES as archetype}
                    <option value={archetype.id}>{archetype.label}</option>
                  {/each}
                </datalist>
                <span class="field-hint">Choose a built-in ID or enter an institutional catalog ID. Leaving this blank preserves legacy planning behavior.</span>
              </div>

              {#if workGraphArchetype.trim()}
                <div class="parameter-editor">
                  <div class="section-header">
                    <div>
                      <h4>Parameter overrides</h4>
                      <p class="section-description">Optional string substitutions applied when the skeleton is composed.</p>
                    </div>
                    <button type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact lattice-btn--dashed" on:click={addWorkGraphParameter}>
                      + Add override
                    </button>
                  </div>
                  {#each workGraphParameterRows as parameter (parameter.id)}
                    <div class="parameter-row">
                      <input class="lattice-input" aria-label="Parameter name" bind:value={parameter.key} placeholder="component" />
                      <input class="lattice-input" aria-label="Parameter value" bind:value={parameter.value} placeholder="authentication" />
                      <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--compact" on:click={() => removeWorkGraphParameter(parameter.id)}>
                        Remove
                      </button>
                    </div>
                  {/each}
                </div>
              {/if}
            </fieldset>

            <fieldset class="policy-fieldset tier-policy-fieldset">
              <legend>Tier routing</legend>
              <label class="checkbox-label lattice-selectable tier-routing-toggle">
                <input
                  type="checkbox"
                  bind:checked={tierPolicy.enabled}
                  on:change={handleTierRoutingToggle}
                />
                <span class="checkbox-text">
                  <span class="checkbox-title">Enable tier-based model routing</span>
                  <span class="checkbox-description">Route planned tasks through provider-native model and effort presets.</span>
                </span>
              </label>

              {#if tierPolicy.enabled}
                <div class="tier-policy-controls">
                  <div class="field">
                    <label for="tier-routing-ceiling">Routing ceiling (%)</label>
                    <input
                      id="tier-routing-ceiling"
                      class="lattice-input"
                      type="number"
                      min="1"
                      max="100"
                      step="1"
                      bind:value={tierPolicy.ceiling_percent}
                    />
                    <span class="field-hint">Maximum share of planned tasks allowed above the review floor.</span>
                  </div>
                  <div class="field">
                    <label for="tier-review-floor">Review floor</label>
                    <select id="tier-review-floor" class="role-select lattice-input" bind:value={tierPolicy.review_floor}>
                      {#each TASK_TIERS as tier}
                        <option value={tier}>{providerLabel(tier)}</option>
                      {/each}
                    </select>
                    <span class="field-hint">Tasks at or above this tier require the policy's review path.</span>
                  </div>
                </div>

                <div class="tier-ladder-preview" aria-live="polite">
                  <div class="tier-ladder-heading">
                    <div>
                      <h4>Provider ladder preview</h4>
                      <p class="section-description">Read-only launch preview for each distinct provider currently in use.</p>
                    </div>
                    {#if tierLadderError}
                      <button type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact" on:click={retryTierLadder}>Retry</button>
                    {/if}
                  </div>

                  {#if !projectPath.trim()}
                    <p class="policy-note">Choose a project folder to resolve its tier ladder.</p>
                  {:else if tierLadderLoading}
                    <p class="policy-note">Resolving tier ladders…</p>
                  {:else if tierLadderError}
                    <p class="tier-ladder-error" role="alert">{tierLadderError}</p>
                  {/if}

                  {#if projectPath.trim() && !tierLadderLoading && !tierLadderError}
                    <div class="tier-ladder-grid">
                      {#each tierProviders as provider}
                        {@const providerCells = tierLadderCells.filter((cell) => cell.provider === provider)}
                        <section class="tier-ladder-card lattice-panel" aria-label={`${providerLabel(provider)} tier ladder`}>
                          <h4>{providerLabel(provider)} ladder</h4>
                          {#if providerCells.length > 0}
                            <dl>
                              {#each TASK_TIERS as tier}
                                {@const cell = providerCells.find((candidate) => candidate.tier === tier)}
                                {#if cell}
                                  <div class="tier-ladder-row">
                                    <dt>{providerLabel(tier)}</dt>
                                    <dd>
                                      <span>{cell.model}</span>
                                      {#if cell.flags.length > 0}
                                        <code>{cell.flags.join(' ')}</code>
                                      {/if}
                                    </dd>
                                  </div>
                                {/if}
                              {/each}
                            </dl>
                          {:else}
                            <p class="policy-note">No maintained ladder is available for this provider.</p>
                          {/if}
                        </section>
                      {/each}
                    </div>

                    {#if tierLadderOmissions.length > 0}
                      <div class="tier-ladder-omissions">
                        <strong>Resolution notes</strong>
                        <ul>
                          {#each tierLadderOmissions as omission}
                            <li>{omission.detail}</li>
                          {/each}
                        </ul>
                      </div>
                    {/if}
                  {/if}
                </div>
              {/if}
            </fieldset>

            <div class="delegation-grid">
              <div class="field">
                <label for="queen-delegation">Queen delegation</label>
                <select id="queen-delegation" bind:value={queenDelegationMode} class="role-select lattice-input">
                  <option value="disabled">Disabled</option>
                  <option value="auto">Automatic when useful</option>
                  <option value="encouraged">Encouraged</option>
                </select>
                <span class="field-hint">Controls native subagents launched by the Queen.</span>
              </div>
              <div class="field">
                <label for="principal-delegation">Principal delegation</label>
                <select id="principal-delegation" bind:value={principalDelegationMode} class="role-select lattice-input">
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
              <button type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact lattice-btn--dashed" on:click={addCodingPrincipal} disabled={codingPrincipals.length >= 3}>
                + Add principal
              </button>
            </div>
            <div class="workers-list principal-list">
              {#each codingPrincipals as principal, i (i)}
                <div class="worker-card principal-card lattice-panel">
                  <div class="card-header">
                    <span class="card-title">{principal.label || `Coding Principal ${i + 1}`}</span>
                    <button
                      type="button"
                      class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--compact"
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
                      class="role-select lattice-input"
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
              class="lattice-btn lattice-btn--ghost lattice-btn--link advanced-disclosure"
              on:click={() => showHiveAdvanced = !showHiveAdvanced}
              aria-expanded={showHiveAdvanced}
              aria-controls="hive-delegation-limits"
            >
              {showHiveAdvanced ? 'Hide' : 'Show'} delegation guidance
            </button>
            {#if showHiveAdvanced}
              <div class="advanced-grid lattice-forced-colors-boundary" id="hive-delegation-limits">
                <label>Queen target max children <input class="lattice-input" type="number" min="1" max="8" bind:value={queenMaxChildren} /></label>
                <label>Queen target max depth <input class="lattice-input" type="number" min="1" max="4" bind:value={queenMaxDepth} /></label>
                <label>Principal target max children <input class="lattice-input" type="number" min="1" max="8" bind:value={principalMaxChildren} /></label>
                <label>Principal target max depth <input class="lattice-input" type="number" min="1" max="4" bind:value={principalMaxDepth} /></label>
                <p class="policy-note">These values guide assignments; native harness settings own hard concurrency limits.</p>
              </div>
            {/if}
          </div>
        {/if}
