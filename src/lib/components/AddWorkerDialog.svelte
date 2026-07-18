<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { coordination, type WorkerRole, type AddWorkerRequest } from '$lib/stores/coordination';
  import {
    activeSession,
    activeAgents,
    serdeEnumVariantName,
    type AgentConfig,
  } from '$lib/stores/sessions';
  import { createSessionPrincipalConfig } from './hiveLaunch';
  import AgentConfigEditor, {
    fetchCliHealth,
    type CliHealthMap,
  } from './AgentConfigEditor.svelte';
  import Composer from './composer/Composer.svelte';

  export let open = false;

  const dispatch = createEventDispatcher<{
    close: void;
    added: { workerId: string };
  }>();

  // A role describes ownership. CLI/model/effort come from the session's
  // principal defaults and remain operator-selectable below.
  const predefinedRoles: { type: string; label: string; description: string; category: 'dev' | 'review' }[] = [
    // Development roles
    { type: 'backend', label: 'Backend', description: 'Server-side logic, APIs, databases', category: 'dev' },
    { type: 'frontend', label: 'Frontend', description: 'UI components, state management', category: 'dev' },
    { type: 'coherence', label: 'Coherence', description: 'Code consistency, API contracts', category: 'dev' },
    { type: 'simplify', label: 'Simplify', description: 'Code simplification, refactoring', category: 'dev' },
    // Review & QA roles
    { type: 'reviewer', label: 'Reviewer', description: 'Deep code review: security, edge cases, architecture', category: 'review' },
    { type: 'reviewer-quick', label: 'Quick Review', description: 'Fast review: obvious bugs, code style', category: 'review' },
    { type: 'resolver', label: 'Resolver', description: 'Address review findings, fix issues', category: 'review' },
    { type: 'tester', label: 'Tester', description: 'Run tests, fix failures, document issues', category: 'review' },
    { type: 'code-quality', label: 'Code Quality', description: 'Resolve PR comments, ensure standards', category: 'review' },
    // Custom
    { type: 'custom', label: 'Custom', description: 'Define your own role', category: 'dev' },
  ];

  let selectedRoleType = 'backend';
  let customRoleName = '';
  let workerConfig: AgentConfig = createSessionPrincipalConfig(null);
  let initializedSessionId: string | null = null;
  let workerName = '';
  let workerDescription = '';
  let initialTask = '';
  let selectedParent: string | null = null;
  let loading = false;
  let error: string | null = null;
  let cliHealth: CliHealthMap = {};
  let cliHealthLoading = false;
  let cliHealthError: string | null = null;
  let healthLoadedForOpen = false;

  async function loadCliHealth() {
    cliHealthLoading = true;
    cliHealthError = null;
    try {
      cliHealth = await fetchCliHealth();
    } catch (err) {
      cliHealthError = err instanceof Error ? err.message : String(err);
    } finally {
      cliHealthLoading = false;
    }
  }

  // Re-open with the active session's durable principal defaults. Once open,
  // the editor owns the values so an operator override is never reset.
  $: if (!open) initializedSessionId = null;
  $: if (!open) healthLoadedForOpen = false;
  $: if (open && !healthLoadedForOpen) {
    healthLoadedForOpen = true;
    void loadCliHealth();
  }
  $: if (open && $activeSession?.id && initializedSessionId !== $activeSession.id) {
    workerConfig = createSessionPrincipalConfig($activeSession);
    selectedParent = null;
    initializedSessionId = $activeSession.id;
  }

  // Get possible parents (Queen or Planners)
  $: parentOptions = $activeAgents.filter(
    (a) =>
      serdeEnumVariantName(a.role) === 'Queen' ||
      (typeof a.role === 'object' && a.role !== null && 'Planner' in a.role)
  );

  function close() {
    open = false;
    error = null;
    selectedParent = null;
    dispatch('close');
  }

  async function handleSubmit() {
    if (!$activeSession?.id) {
      error = 'No active session';
      return;
    }

    const roleType = selectedRoleType === 'custom' ? customRoleName.toLowerCase() : selectedRoleType;
    const roleLabel = selectedRoleType === 'custom' ? customRoleName : predefinedRoles.find((r) => r.type === selectedRoleType)?.label || selectedRoleType;

    if (selectedRoleType === 'custom' && !customRoleName.trim()) {
      error = 'Please enter a custom role name';
      return;
    }

    const explicitName = workerName.trim();
    const trimmedDescription = workerDescription.trim() || `${roleLabel} tasks`;

    const role: WorkerRole = {
      role_type: roleType,
      label: roleLabel,
      default_cli: workerConfig.cli,
      prompt_template: null,
    };

    const request: AddWorkerRequest = {
      session_id: $activeSession.id,
      ...(explicitName ? { name: explicitName } : {}),
      description: trimmedDescription,
      config: {
        ...workerConfig,
        ...(explicitName ? { name: explicitName } : {}),
        description: trimmedDescription,
        role,
        initial_prompt: initialTask || undefined,
      },
      role,
      parent_id: selectedParent || undefined,
    };

    loading = true;
    error = null;

    try {
      const agentInfo = await coordination.addWorker(request);
      dispatch('added', { workerId: (agentInfo as { id: string }).id });
      close();
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      close();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div class="dialog-overlay" on:click={close} role="presentation">
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div class="dialog" on:click|stopPropagation role="dialog" aria-modal="true" tabindex="-1">
      <div class="dialog-header">
        <h2>Add Managed Principal</h2>
        <button class="close-btn" on:click={close}>&times;</button>
      </div>

      <form on:submit|preventDefault={handleSubmit}>
        <div class="form-section">
          <span class="section-label" id="role-label">Role</span>

          <div class="role-category">
            <span class="category-label">Development</span>
            <div class="role-grid" role="group" aria-labelledby="role-label">
              {#each predefinedRoles.filter(r => r.category === 'dev') as role}
                <button
                  type="button"
                  class="role-option"
                  class:selected={selectedRoleType === role.type}
                  on:click={() => (selectedRoleType = role.type)}
                >
                  <span class="role-name">{role.label}</span>
                  <span class="role-desc">{role.description}</span>
                </button>
              {/each}
            </div>
          </div>

          <div class="role-category">
            <span class="category-label">Review & QA</span>
            <div class="role-grid" role="group" aria-labelledby="role-label">
              {#each predefinedRoles.filter(r => r.category === 'review') as role}
                <button
                  type="button"
                  class="role-option"
                  class:selected={selectedRoleType === role.type}
                  on:click={() => (selectedRoleType = role.type)}
                >
                  <span class="role-name">{role.label}</span>
                  <span class="role-desc">{role.description}</span>
                </button>
              {/each}
            </div>
          </div>

          {#if selectedRoleType === 'custom'}
            <input
              type="text"
              placeholder="Custom role name..."
              bind:value={customRoleName}
              class="custom-role-input"
            />
          {/if}
        </div>

        <div class="form-section">
          <span class="section-label">Principal Runtime</span>
          <AgentConfigEditor
            bind:config={workerConfig}
            showLabel={false}
            idPrefix="add-worker-principal"
            {cliHealth}
            {cliHealthLoading}
            {cliHealthError}
          />
        </div>

        {#if parentOptions.length > 1}
          <div class="form-section">
            <label class="section-label" for="parent-select">Parent (optional)</label>
            <select id="parent-select" bind:value={selectedParent} class="select-input">
              <option value={null}>Default (Queen)</option>
              {#each parentOptions as parent}
                <option value={parent.id}>
                  {parent.config.label || parent.id}
                </option>
              {/each}
            </select>
          </div>
        {/if}

        <div class="form-section">
          <label class="section-label" for="name-input">Principal Name</label>
          <input
            id="name-input"
            type="text"
            placeholder="Worker 2 (Frontend)"
            bind:value={workerName}
            class="custom-role-input"
          />
        </div>

        <div class="form-section">
          <label class="section-label" for="description-input">Description</label>
          <input
            id="description-input"
            type="text"
            placeholder="One-line task summary"
            bind:value={workerDescription}
            class="custom-role-input"
          />
        </div>

        <div class="form-section">
          <span class="section-label" id="task-label">Initial Task (optional)</span>
          <div class="composer-host" aria-labelledby="task-label">
            <Composer
              sessionId={$activeSession?.id ?? null}
              placeholder="Describe the initial task for this worker… (@ to mention, / for commands)"
              persistDraft={false}
              bind:value={initialTask}
            />
          </div>
        </div>

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="dialog-actions">
          <button type="button" class="btn-secondary" on:click={close}>Cancel</button>
          <button type="submit" class="btn-primary" disabled={loading}>
            {#if loading}
              Adding principal...
            {:else}
              Add Managed Principal
            {/if}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .dialog-overlay {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg-void) 60%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .dialog {
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    width: 480px;
    max-width: 90vw;
    max-height: 90vh;
    overflow-y: auto;
  }

  .dialog-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-structural);
  }

  .dialog-header h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-secondary);
    font-size: 24px;
    cursor: pointer;
    padding: 0;
    line-height: 1;
  }

  .close-btn:hover {
    color: var(--text-primary);
  }

  form {
    padding: 20px;
  }

  .form-section {
    margin-bottom: 20px;
  }

  .section-label {
    display: block;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--text-secondary);
    margin-bottom: 8px;
  }

  .role-category {
    margin-bottom: 16px;
  }

  .role-category:last-of-type {
    margin-bottom: 0;
  }

  .category-label {
    display: block;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-disabled);
    margin-bottom: 8px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .role-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }

  .role-option {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 12px;
    background: var(--bg-surface);
    border: 2px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
    text-align: left;
    transition: border-color 0.15s;
  }

  .role-option:hover {
    border-color: var(--border-structural);
  }

  .role-option.selected {
    border-color: var(--accent-cyan);
    background: color-mix(in srgb, var(--accent-cyan) 10%, transparent);
  }

  .role-name {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .role-desc {
    font-size: 11px;
    color: var(--text-secondary);
  }

  .custom-role-input,
  .select-input {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: 14px;
  }

  .custom-role-input:focus,
  .select-input:focus {
    outline: none;
    border-color: var(--accent-cyan);
  }

  .custom-role-input {
    margin-top: 12px;
  }

  .composer-host {
    display: flex;
  }

  .error-message {
    padding: 10px 12px;
    background: var(--bg-elevated);
    color: var(--status-error);
    border-radius: var(--radius-sm);
    font-size: 13px;
    margin-bottom: 16px;
  }

  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    padding-top: 8px;
  }

  .btn-primary,
  .btn-secondary {
    padding: 10px 20px;
    border-radius: var(--radius-sm);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    border: none;
    transition: opacity 0.15s;
  }

  .btn-primary {
    background: var(--accent-cyan);
    color: white;
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.9;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-secondary {
    background: var(--bg-surface);
    color: var(--text-primary);
    border: 1px solid var(--border-structural);
  }

  .btn-secondary:hover {
    background: var(--bg-elevated);
  }
</style>
