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
          <div data-launch-owned class="form-section hive-policy-section lattice-forced-colors-boundary">
            <div data-launch-owned class="section-heading-copy">
              <h3 data-launch-owned>Hive Topology</h3>
              <p data-launch-owned>Keep the team legible: a Queen, a small set of coding principals, and bounded native delegation.</p>
            </div>

            <fieldset data-launch-owned class="policy-fieldset">
              <legend data-launch-owned>Workspace strategy</legend>
              <div data-launch-owned class="choice-grid">
                <label data-launch-owned
                  class="choice-card lattice-panel lattice-selectable"
                  class:lattice-selectable--selected={workspaceStrategy === 'shared_cell'}
                >
                  <input data-launch-owned type="radio" name="workspace-strategy" bind:group={workspaceStrategy} value="shared_cell" />
                  <span data-launch-owned class="choice-title">Shared cell</span>
                  <span data-launch-owned class="choice-description">Queen and principals collaborate in one workspace.</span>
                </label>
                <label data-launch-owned
                  class="choice-card lattice-panel lattice-selectable"
                  class:lattice-selectable--selected={workspaceStrategy === 'isolated_cell'}
                >
                  <input data-launch-owned type="radio" name="workspace-strategy" bind:group={workspaceStrategy} value="isolated_cell" />
                  <span data-launch-owned class="choice-title">Per-principal worktrees</span>
                  <span data-launch-owned class="choice-description">Queen and each visible principal get isolated managed worktrees.</span>
                </label>
              </div>
            </fieldset>

            <fieldset data-launch-owned class="policy-fieldset">
              <legend data-launch-owned>Work-graph archetype (optional)</legend>
              <div data-launch-owned class="field">
                <label data-launch-owned for="work-graph-archetype">Archetype</label>
                <input data-launch-owned
                  id="work-graph-archetype"
                  class="lattice-input"
                  list="work-graph-archetypes"
                  bind:value={workGraphArchetype}
                  placeholder="None — preserve the planner's existing topology"
                />
                <datalist data-launch-owned id="work-graph-archetypes">
                  {#each WORK_GRAPH_ARCHETYPES as archetype}
                    <option data-launch-owned value={archetype.id}>{archetype.label}</option>
                  {/each}
                </datalist>
                <span data-launch-owned class="field-hint">Choose a built-in ID or enter an institutional catalog ID. Leaving this blank preserves legacy planning behavior.</span>
              </div>

              {#if workGraphArchetype.trim()}
                <div data-launch-owned class="parameter-editor">
                  <div data-launch-owned class="section-header">
                    <div data-launch-owned>
                      <h4 data-launch-owned>Parameter overrides</h4>
                      <p data-launch-owned class="section-description">Optional string substitutions applied when the skeleton is composed.</p>
                    </div>
                    <button data-launch-owned type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact lattice-btn--dashed" on:click={addWorkGraphParameter}>
                      + Add override
                    </button>
                  </div>
                  {#each workGraphParameterRows as parameter (parameter.id)}
                    <div data-launch-owned class="parameter-row">
                      <input data-launch-owned class="lattice-input" aria-label="Parameter name" bind:value={parameter.key} placeholder="component" />
                      <input data-launch-owned class="lattice-input" aria-label="Parameter value" bind:value={parameter.value} placeholder="authentication" />
                      <button data-launch-owned type="button" class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--compact" on:click={() => removeWorkGraphParameter(parameter.id)}>
                        Remove
                      </button>
                    </div>
                  {/each}
                </div>
              {/if}
            </fieldset>

            <fieldset data-launch-owned class="policy-fieldset tier-policy-fieldset">
              <legend data-launch-owned>Tier routing</legend>
              <label data-launch-owned class="checkbox-label lattice-selectable tier-routing-toggle">
                <input data-launch-owned
                  type="checkbox"
                  bind:checked={tierPolicy.enabled}
                  on:change={handleTierRoutingToggle}
                />
                <span data-launch-owned class="checkbox-text">
                  <span data-launch-owned class="checkbox-title">Enable tier-based model routing</span>
                  <span data-launch-owned class="checkbox-description">Route planned tasks through provider-native model and effort presets.</span>
                </span>
              </label>

              {#if tierPolicy.enabled}
                <div data-launch-owned class="tier-policy-controls">
                  <div data-launch-owned class="field">
                    <label data-launch-owned for="tier-routing-ceiling">Routing ceiling (%)</label>
                    <input data-launch-owned
                      id="tier-routing-ceiling"
                      class="lattice-input"
                      type="number"
                      min="1"
                      max="100"
                      step="1"
                      bind:value={tierPolicy.ceiling_percent}
                    />
                    <span data-launch-owned class="field-hint">Maximum share of planned tasks allowed above the review floor.</span>
                  </div>
                  <div data-launch-owned class="field">
                    <label data-launch-owned for="tier-review-floor">Review floor</label>
                    <select data-launch-owned id="tier-review-floor" class="role-select lattice-input" bind:value={tierPolicy.review_floor}>
                      {#each TASK_TIERS as tier}
                        <option data-launch-owned value={tier}>{providerLabel(tier)}</option>
                      {/each}
                    </select>
                    <span data-launch-owned class="field-hint">Tasks at or above this tier require the policy's review path.</span>
                  </div>
                </div>

                <div data-launch-owned class="tier-ladder-preview" aria-live="polite">
                  <div data-launch-owned class="tier-ladder-heading">
                    <div data-launch-owned>
                      <h4 data-launch-owned>Provider ladder preview</h4>
                      <p data-launch-owned class="section-description">Read-only launch preview for each distinct provider currently in use.</p>
                    </div>
                    {#if tierLadderError}
                      <button data-launch-owned type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact" on:click={retryTierLadder}>Retry</button>
                    {/if}
                  </div>

                  {#if !projectPath.trim()}
                    <p data-launch-owned class="policy-note">Choose a project folder to resolve its tier ladder.</p>
                  {:else if tierLadderLoading}
                    <p data-launch-owned class="policy-note">Resolving tier ladders…</p>
                  {:else if tierLadderError}
                    <p data-launch-owned class="tier-ladder-error" role="alert">{tierLadderError}</p>
                  {/if}

                  {#if projectPath.trim() && !tierLadderLoading && !tierLadderError}
                    <div data-launch-owned class="tier-ladder-grid">
                      {#each tierProviders as provider}
                        {@const providerCells = tierLadderCells.filter((cell) => cell.provider === provider)}
                        <section data-launch-owned class="tier-ladder-card lattice-panel" aria-label={`${providerLabel(provider)} tier ladder`}>
                          <h4 data-launch-owned>{providerLabel(provider)} ladder</h4>
                          {#if providerCells.length > 0}
                            <dl data-launch-owned>
                              {#each TASK_TIERS as tier}
                                {@const cell = providerCells.find((candidate) => candidate.tier === tier)}
                                {#if cell}
                                  <div data-launch-owned class="tier-ladder-row">
                                    <dt data-launch-owned>{providerLabel(tier)}</dt>
                                    <dd data-launch-owned>
                                      <span data-launch-owned>{cell.model}</span>
                                      {#if cell.flags.length > 0}
                                        <code data-launch-owned>{cell.flags.join(' ')}</code>
                                      {/if}
                                    </dd>
                                  </div>
                                {/if}
                              {/each}
                            </dl>
                          {:else}
                            <p data-launch-owned class="policy-note">No maintained ladder is available for this provider.</p>
                          {/if}
                        </section>
                      {/each}
                    </div>

                    {#if tierLadderOmissions.length > 0}
                      <div data-launch-owned class="tier-ladder-omissions">
                        <strong data-launch-owned>Resolution notes</strong>
                        <ul data-launch-owned>
                          {#each tierLadderOmissions as omission}
                            <li data-launch-owned>{omission.detail}</li>
                          {/each}
                        </ul>
                      </div>
                    {/if}
                  {/if}
                </div>
              {/if}
            </fieldset>

            <div data-launch-owned class="delegation-grid">
              <div data-launch-owned class="field">
                <label data-launch-owned for="queen-delegation">Queen delegation</label>
                <select data-launch-owned id="queen-delegation" bind:value={queenDelegationMode} class="role-select lattice-input">
                  <option data-launch-owned value="disabled">Disabled</option>
                  <option data-launch-owned value="auto">Automatic when useful</option>
                  <option data-launch-owned value="encouraged">Encouraged</option>
                </select>
                <span data-launch-owned class="field-hint">Controls native subagents launched by the Queen.</span>
              </div>
              <div data-launch-owned class="field">
                <label data-launch-owned for="principal-delegation">Principal delegation</label>
                <select data-launch-owned id="principal-delegation" bind:value={principalDelegationMode} class="role-select lattice-input">
                  <option data-launch-owned value="disabled">Disabled</option>
                  <option data-launch-owned value="auto">Automatic when useful</option>
                  <option data-launch-owned value="encouraged">Encouraged</option>
                </select>
                <span data-launch-owned class="field-hint">Applies to each visible coding principal.</span>
              </div>
            </div>

            <div data-launch-owned class="principals-heading section-header">
              <div data-launch-owned>
                <h4 data-launch-owned>Coding Principals ({codingPrincipals.length})</h4>
                <p data-launch-owned class="section-description">Each principal owns a coherent lane and may delegate according to policy.</p>
              </div>
              <button data-launch-owned type="button" class="lattice-btn lattice-btn--secondary lattice-btn--compact lattice-btn--dashed" on:click={addCodingPrincipal} disabled={codingPrincipals.length >= 3}>
                + Add principal
              </button>
            </div>
            <div data-launch-owned class="workers-list principal-list">
              {#each codingPrincipals as principal, i (i)}
                <div data-launch-owned class="worker-card principal-card lattice-panel">
                  <div data-launch-owned class="card-header">
                    <span data-launch-owned class="card-title">{principal.label || `Coding Principal ${i + 1}`}</span>
                    <button data-launch-owned
                      type="button"
                      class="lattice-btn lattice-btn--ghost lattice-btn--danger lattice-btn--compact"
                      on:click={() => removeCodingPrincipal(i)}
                      disabled={codingPrincipals.length <= 1}
                    >
                      Remove
                    </button>
                  </div>
                  <div data-launch-owned class="field principal-role-field">
                    <label data-launch-owned for={`launch-principal-${i}-role`}>Specialization</label>
                    <select data-launch-owned
                      id={`launch-principal-${i}-role`}
                      bind:value={principal.selectedRole}
                      class="role-select lattice-input"
                    >
                      {#each predefinedRoles as role}
                        <option data-launch-owned value={role.type}>{role.label}</option>
                      {/each}
                      {#if !predefinedRoles.some((role) => role.type === principal.selectedRole)}
                        <option data-launch-owned value={principal.selectedRole}>{principal.selectedRole} (custom)</option>
                      {/if}
                    </select>
                    <span data-launch-owned class="field-hint">
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

            <button data-launch-owned
              type="button"
              class="lattice-btn lattice-btn--ghost lattice-btn--link advanced-disclosure"
              on:click={() => showHiveAdvanced = !showHiveAdvanced}
              aria-expanded={showHiveAdvanced}
              aria-controls="hive-delegation-limits"
            >
              {showHiveAdvanced ? 'Hide' : 'Show'} delegation guidance
            </button>
            {#if showHiveAdvanced}
              <div data-launch-owned class="advanced-grid lattice-forced-colors-boundary" id="hive-delegation-limits">
                <label data-launch-owned>Queen target max children <input data-launch-owned class="lattice-input" type="number" min="1" max="8" bind:value={queenMaxChildren} /></label>
                <label data-launch-owned>Queen target max depth <input data-launch-owned class="lattice-input" type="number" min="1" max="4" bind:value={queenMaxDepth} /></label>
                <label data-launch-owned>Principal target max children <input data-launch-owned class="lattice-input" type="number" min="1" max="8" bind:value={principalMaxChildren} /></label>
                <label data-launch-owned>Principal target max depth <input data-launch-owned class="lattice-input" type="number" min="1" max="4" bind:value={principalMaxDepth} /></label>
                <p data-launch-owned class="policy-note">These values guide assignments; native harness settings own hard concurrency limits.</p>
              </div>
            {/if}
          </div>
        {/if}
