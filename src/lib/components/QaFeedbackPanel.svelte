<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { activeSession, serdeEnumVariantName } from '$lib/stores/sessions';

  interface QaCriterion {
    id: string;
    label: string;
    passed: boolean;
    evidence: string | null;
  }

  interface QaVerdict {
    session_id: string;
    milestone_id: string;
    iteration: number;
    passed: boolean;
    criteria: QaCriterion[];
    summary: string;
    timestamp: string;
  }

  let verdict = $state<QaVerdict | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let collapsed = $state(false);

  async function loadVerdict(sessionId: string) {
    loading = true;
    error = null;
    try {
      // Use the new endpoint for QA verdict
      const data = await invoke<QaVerdict | null>('get_qa_verdict', { sessionId });
      verdict = data;
    } catch (e) {
      console.error('Failed to load QA verdict:', e);
      // It's okay if it doesn't exist yet
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    if ($activeSession) {
      loadVerdict($activeSession.id);
    }

    const unlisten = listen('qa-verdict-updated', (event) => {
      if ($activeSession && (event.payload as any).session_id === $activeSession.id) {
        loadVerdict($activeSession.id);
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  });

  $effect(() => {
    if ($activeSession?.id) {
      loadVerdict($activeSession.id);
    } else {
      verdict = null;
    }
  });

  const stateClass = $derived(verdict ? (verdict.passed ? 'passed' : 'failed') : '');
</script>

{#if verdict}
  <div class="qa-feedback-panel" class:collapsed>
    <button class="panel-header" onclick={() => collapsed = !collapsed}>
      <div class="header-left">
        <span class="status-icon {stateClass}">{verdict.passed ? '✓' : '✗'}</span>
        <h3>QA Feedback</h3>
        <span class="iteration-badge">Attempt {verdict.iteration}</span>
      </div>
      <span class="chevron">{collapsed ? '▼' : '▲'}</span>
    </button>

    {#if !collapsed}
      <div class="panel-content">
        <div class="summary">
          <p>{verdict.summary}</p>
        </div>

        <div class="criteria-list">
          {#each verdict.criteria as criterion}
            <div class="criterion-item" class:passed={criterion.passed}>
              <div class="criterion-header">
                <span class="criterion-icon">{criterion.passed ? '✓' : '✗'}</span>
                <span class="criterion-label">{criterion.label}</span>
              </div>
              {#if criterion.evidence}
                <p class="criterion-evidence">{criterion.evidence}</p>
              {/if}
            </div>
          {/each}
        </div>
        
        <div class="footer">
          <span class="timestamp">{new Date(verdict.timestamp).toLocaleString()}</span>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .qa-feedback-panel {
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    margin-bottom: 12px;
    overflow: hidden;
  }

  .panel-header {
    width: 100%;
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 14px;
    background: var(--bg-void);
    border: none;
    cursor: pointer;
    text-align: left;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .status-icon {
    font-size: 14px;
    font-weight: bold;
  }

  .status-icon.passed {
    color: var(--status-success);
  }

  .status-icon.failed {
    color: var(--status-error);
  }

  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .iteration-badge {
    font-size: 10px;
    padding: 2px 6px;
    background: var(--border-structural);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
  }

  .chevron {
    font-size: 10px;
    color: var(--text-secondary);
  }

  .panel-content {
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .summary {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-primary);
    padding-bottom: 12px;
    border-bottom: 1px solid var(--border-structural);
  }

  .summary p {
    margin: 0;
  }

  .criteria-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .criterion-item {
    padding: 10px;
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--status-error) 5%, transparent);
    border: 1px solid color-mix(in srgb, var(--status-error) 10%, transparent);
  }

  .criterion-item.passed {
    background: color-mix(in srgb, var(--status-success) 5%, transparent);
    border-color: color-mix(in srgb, var(--status-success) 10%, transparent);
  }

  .criterion-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }

  .criterion-icon {
    font-size: 12px;
    font-weight: bold;
  }

  .criterion-item:not(.passed) .criterion-icon {
    color: var(--status-error);
  }

  .criterion-item.passed .criterion-icon {
    color: var(--status-success);
  }

  .criterion-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .criterion-evidence {
    margin: 0;
    font-size: 11px;
    color: var(--text-secondary);
    line-height: 1.4;
    padding-left: 20px;
  }

  .footer {
    display: flex;
    justify-content: flex-end;
    margin-top: 4px;
  }

  .timestamp {
    font-size: 10px;
    color: var(--text-secondary);
  }
</style>
