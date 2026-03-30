<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { activeSession } from '$lib/stores/sessions';

  interface Contract {
    id: string;
    milestone_id: string;
    content: string;
    passed: boolean | null;
    grading_weights: Record<string, number>;
    threshold: number;
  }

  let contract = $state<Contract | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function loadContract(sessionId: string) {
    loading = true;
    error = null;
    try {
      const data = await invoke<Contract | null>('get_current_contract', { sessionId });
      contract = data;
    } catch (e) {
      console.error('Failed to load contract:', e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    if ($activeSession) {
      loadContract($activeSession.id);
    }
  });

  $effect(() => {
    if ($activeSession?.id) {
      loadContract($activeSession.id);
    } else {
      contract = null;
    }
  });
</script>

{#if contract}
  <div class="contract-viewer">
    <div class="contract-header">
      <div class="header-info">
        <h3>Sprint Contract</h3>
        <span class="milestone-id">Milestone: {contract.milestone_id}</span>
      </div>
      <div class="threshold-badge">
        Threshold: {contract.threshold}%
      </div>
    </div>

    <div class="contract-content-wrapper">
      {#if contract.passed !== null}
        <div class="status-overlay" class:passed={contract.passed}>
          <div class="overlay-icon">
            {contract.passed ? '✓' : '✗'}
          </div>
          <div class="overlay-text">
            {contract.passed ? 'PASSED' : 'FAILED'}
          </div>
        </div>
      {/if}
      
      <div class="content">
        <pre>{contract.content}</pre>
      </div>
    </div>

    {#if Object.keys(contract.grading_weights).length > 0}
      <div class="weights-section">
        <h4>Grading Weights</h4>
        <div class="weights-grid">
          {#each Object.entries(contract.grading_weights) as [key, weight]}
            <div class="weight-item">
              <span class="weight-key">{key}</span>
              <span class="weight-value">{weight}%</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .contract-viewer {
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .contract-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    background: var(--color-bg);
    border-bottom: 1px solid var(--color-border);
  }

  .header-info h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--color-text);
  }

  .milestone-id {
    font-size: 11px;
    color: var(--color-text-muted);
  }

  .threshold-badge {
    font-size: 11px;
    padding: 3px 8px;
    background: var(--color-border);
    border-radius: 4px;
    color: var(--color-text-muted);
  }

  .contract-content-wrapper {
    position: relative;
    flex: 1;
    min-height: 200px;
    max-height: 400px;
    overflow-y: auto;
  }

  .status-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    background: rgba(247, 118, 142, 0.1);
    pointer-events: none;
    z-index: 1;
  }

  .status-overlay.passed {
    background: rgba(158, 206, 106, 0.1);
  }

  .overlay-icon {
    font-size: 80px;
    font-weight: bold;
    opacity: 0.3;
  }

  .status-overlay:not(.passed) .overlay-icon {
    color: var(--color-error);
  }

  .status-overlay.passed .overlay-icon {
    color: var(--color-success);
  }

  .overlay-text {
    font-size: 24px;
    font-weight: 800;
    letter-spacing: 4px;
    opacity: 0.3;
    margin-top: -10px;
  }

  .status-overlay:not(.passed) .overlay-text {
    color: var(--color-error);
  }

  .status-overlay.passed .overlay-text {
    color: var(--color-success);
  }

  .content {
    padding: 16px;
  }

  pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: inherit;
    font-size: 13px;
    color: var(--color-text);
    line-height: 1.5;
  }

  .weights-section {
    padding: 12px 16px;
    background: var(--color-bg);
    border-top: 1px solid var(--color-border);
  }

  h4 {
    margin: 0 0 8px 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .weights-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 8px;
  }

  .weight-item {
    display: flex;
    justify-content: space-between;
    background: var(--color-surface);
    padding: 4px 8px;
    border-radius: 4px;
    border: 1px solid var(--color-border);
  }

  .weight-key {
    font-size: 11px;
    color: var(--color-text);
  }

  .weight-value {
    font-size: 11px;
    font-weight: 600;
    color: var(--color-accent);
  }
</style>
