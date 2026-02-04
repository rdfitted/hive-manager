<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { AgentConfig } from '$lib/stores/sessions';

  export let config: AgentConfig;
  export let showLabel: boolean = true;

  const dispatch = createEventDispatcher<{ change: AgentConfig }>();

  function handleCliChange(e: Event) {
    const target = e.target as HTMLInputElement;
    config = {
      ...config,
      cli: target.value,
    };
    dispatch('change', config);
  }

  function handleLabelChange(e: Event) {
    const target = e.target as HTMLInputElement;
    config = {
      ...config,
      label: target.value || undefined,
    };
    dispatch('change', config);
  }
</script>

<div class="config-editor">
  {#if showLabel}
    <div class="field">
      <label for="label">Label</label>
      <input
        id="label"
        type="text"
        placeholder="Optional display name"
        value={config.label || ''}
        on:input={handleLabelChange}
      />
    </div>
  {/if}

  <div class="field">
    <label for="cli">Command</label>
    <input
      id="cli"
      type="text"
      placeholder="claude, gemini, opencode..."
      value={config.cli}
      on:input={handleCliChange}
    />
  </div>
</div>

<style>
  .config-editor {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  label {
    font-size: 12px;
    font-weight: 500;
    color: var(--color-text-muted);
  }

  input {
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    background: var(--color-bg);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    color: var(--color-text);
  }

  input::placeholder {
    color: var(--color-text-muted);
    opacity: 0.6;
  }

  input:focus {
    outline: none;
    border-color: var(--color-primary, #8b5cf6);
  }
</style>
