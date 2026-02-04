<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { coordination, type WorkerRole, type AddWorkerRequest } from '$lib/stores/coordination';
  import { activeSession, activeAgents } from '$lib/stores/sessions';

  export let open = false;

  const dispatch = createEventDispatcher<{
    close: void;
    added: { workerId: string };
  }>();

  // Predefined roles (all default to claude for compatibility)
  const predefinedRoles: { type: string; label: string; cli: string; description: string; category: 'dev' | 'review' }[] = [
    // Development roles
    { type: 'backend', label: 'Backend', cli: 'claude', description: 'Server-side logic, APIs, databases', category: 'dev' },
    { type: 'frontend', label: 'Frontend', cli: 'claude', description: 'UI components, state management', category: 'dev' },
    { type: 'coherence', label: 'Coherence', cli: 'claude', description: 'Code consistency, API contracts', category: 'dev' },
    { type: 'simplify', label: 'Simplify', cli: 'claude', description: 'Code simplification, refactoring', category: 'dev' },
    // Review & QA roles
    { type: 'reviewer', label: 'Reviewer', cli: 'claude', description: 'Deep code review: security, edge cases, architecture', category: 'review' },
    { type: 'reviewer-quick', label: 'Quick Review', cli: 'claude', description: 'Fast review: obvious bugs, code style', category: 'review' },
    { type: 'resolver', label: 'Resolver', cli: 'claude', description: 'Address review findings, fix issues', category: 'review' },
    { type: 'tester', label: 'Tester', cli: 'claude', description: 'Run tests, fix failures, document issues', category: 'review' },
    { type: 'code-quality', label: 'Code Quality', cli: 'claude', description: 'Resolve PR comments, ensure standards', category: 'review' },
    // Custom
    { type: 'custom', label: 'Custom', cli: 'claude', description: 'Define your own role', category: 'dev' },
  ];

  // CLI options
  const cliOptions = [
    { value: 'claude', label: 'Claude Code', description: 'Anthropic Claude (Opus 4.5)' },
    { value: 'gemini', label: 'Gemini CLI', description: 'Google Gemini Pro' },
    { value: 'opencode', label: 'OpenCode', description: 'Grok, BigPickle, GLM' },
    { value: 'codex', label: 'Codex', description: 'OpenAI GPT-5.2' },
  ];

  let selectedRoleType = 'backend';
  let customRoleName = '';
  let selectedCli = 'claude';
  let initialTask = '';
  let selectedParent: string | null = null;
  let loading = false;
  let error: string | null = null;

  // Update CLI when role changes
  $: {
    const role = predefinedRoles.find((r) => r.type === selectedRoleType);
    if (role) {
      selectedCli = role.cli;
    }
  }

  // Get possible parents (Queen or Planners)
  $: parentOptions = $activeAgents.filter(
    (a) => a.role === 'Queen' || (typeof a.role === 'object' && 'Planner' in a.role)
  );

  function close() {
    open = false;
    error = null;
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

    const role: WorkerRole = {
      role_type: roleType,
      label: roleLabel,
      default_cli: selectedCli,
      prompt_template: null,
    };

    const request: AddWorkerRequest = {
      session_id: $activeSession.id,
      config: {
        cli: selectedCli,
        flags: [],
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
        <h2>Add Worker</h2>
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
          <label class="section-label" for="cli-select">CLI</label>
          <select
            id="cli-select"
            bind:value={selectedCli}
            class="select-input"
          >
            {#each cliOptions as cli}
              <option value={cli.value}>{cli.label}</option>
            {/each}
          </select>
          <span class="cli-description">
            {cliOptions.find(c => c.value === selectedCli)?.description || ''}
          </span>
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
          <label class="section-label" for="task-input">Initial Task (optional)</label>
          <textarea
            id="task-input"
            placeholder="Describe the initial task for this worker..."
            bind:value={initialTask}
            rows={3}
            class="textarea-input"
          ></textarea>
        </div>

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="dialog-actions">
          <button type="button" class="btn-secondary" on:click={close}>Cancel</button>
          <button type="submit" class="btn-primary" disabled={loading}>
            {#if loading}
              Adding...
            {:else}
              Add Worker
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
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .dialog {
    background: var(--bg-secondary, #1a1b26);
    border: 1px solid var(--border-color, #414868);
    border-radius: 12px;
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
    border-bottom: 1px solid var(--border-color, #414868);
  }

  .dialog-header h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--text-primary, #c0caf5);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-secondary, #565f89);
    font-size: 24px;
    cursor: pointer;
    padding: 0;
    line-height: 1;
  }

  .close-btn:hover {
    color: var(--text-primary, #c0caf5);
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
    color: var(--text-secondary, #565f89);
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
    color: var(--text-tertiary, #444b6a);
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
    background: var(--bg-tertiary, #24283b);
    border: 2px solid transparent;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition: border-color 0.15s;
  }

  .role-option:hover {
    border-color: var(--border-color, #414868);
  }

  .role-option.selected {
    border-color: var(--accent-color, #7aa2f7);
    background: rgba(122, 162, 247, 0.1);
  }

  .role-name {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary, #c0caf5);
    margin-bottom: 4px;
  }

  .role-desc {
    font-size: 11px;
    color: var(--text-secondary, #565f89);
  }

  .custom-role-input,
  .select-input,
  .textarea-input {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-tertiary, #24283b);
    border: 1px solid var(--border-color, #414868);
    border-radius: 6px;
    color: var(--text-primary, #c0caf5);
    font-size: 14px;
  }

  .custom-role-input:focus,
  .select-input:focus,
  .textarea-input:focus {
    outline: none;
    border-color: var(--accent-color, #7aa2f7);
  }

  .custom-role-input {
    margin-top: 12px;
  }

  .textarea-input {
    resize: vertical;
    min-height: 60px;
    font-family: inherit;
  }

  .cli-description {
    font-size: 11px;
    color: var(--text-secondary, #565f89);
    margin-top: 4px;
    display: block;
  }

  .error-message {
    padding: 10px 12px;
    background: var(--error-bg, #3b2030);
    color: var(--error-text, #f7768e);
    border-radius: 6px;
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
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    border: none;
    transition: opacity 0.15s;
  }

  .btn-primary {
    background: var(--accent-color, #7aa2f7);
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
    background: var(--bg-tertiary, #24283b);
    color: var(--text-primary, #c0caf5);
    border: 1px solid var(--border-color, #414868);
  }

  .btn-secondary:hover {
    background: var(--bg-hover, #292e42);
  }
</style>
