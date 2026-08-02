<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { ResumeReport, RunJournalEntry, LedgerEntry } from '$lib/stores/sessions';

  /** Whether the modal is visible. */
  export let open = false;
  /** The session being resumed (for the title). */
  export let sessionName: string | null = null;
  /** The resume classification produced by the backend or preview store. */
  export let report: ResumeReport | null = null;
  export let loading = false;
  export let confirming = false;
  export let error: string | null = null;

  // Default-checked: skip already-completed write-steps so destructive git ops
  // (commits, branch/worktree creation, worker/evaluator spawns) are not re-run.
  let skipCompletedWriteSteps = true;
  let wasOpen = false;

  const dispatch = createEventDispatcher<{
    confirm: { skipCompletedWriteSteps: boolean };
    cancel: void;
  }>();

  $: skipped = report?.skipped ?? [];
  $: interrupted = report?.interrupted ?? [];
  $: uncertain = report?.uncertain ?? [];
  $: hasWarnings = interrupted.length > 0 || uncertain.length > 0;
  $: if (open && !wasOpen) {
    skipCompletedWriteSteps = true;
  }
  $: wasOpen = open;

  function kindLabel(entry: RunJournalEntry): string {
    return entry.kind.replace(/_/g, ' ');
  }

  function effectLabel(entry: LedgerEntry): string {
    const ref = entry.effect_ref ? ` (${entry.effect_ref.slice(0, 10)})` : '';
    return `${entry.effect_kind.replace(/_/g, ' ')}${ref}`;
  }

  function confirm() {
    dispatch('confirm', { skipCompletedWriteSteps });
  }

  function cancel() {
    dispatch('cancel');
  }
</script>

{#if open}
  <div
    class="modal-backdrop lattice-modal-backdrop"
    role="presentation"
    on:click={cancel}
    on:keydown={(e) => e.key === 'Escape' && cancel()}
  >
    <div
      class="modal lattice-modal"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label="Resume session"
      on:click|stopPropagation
      on:keydown|stopPropagation={(e) => e.key === 'Escape' && cancel()}
    >
      <header class="modal-header">
        <h2>Resume {sessionName ?? 'session'}</h2>
      </header>

      <div class="modal-body lattice-scroll-chrome">
        {#if loading}
          <p class="muted">Reading run journal...</p>
        {:else if error}
          <p class="error-text">{error}</p>
        {:else if !report}
          <p class="muted">No prior run journal - resuming a clean session.</p>
        {:else}
          {#if skipped.length > 0}
            <section>
              <h3>Completed steps ({skipped.length})</h3>
              <p class="muted">These write-steps already finished and will not be re-run.</p>
              <ul>
                {#each skipped as step (step.step_id)}
                  <li><span class="status-badge status-success resume-status">done</span> {kindLabel(step)}</li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if interrupted.length > 0}
            <section>
              <h3>Interrupted steps ({interrupted.length})</h3>
              <p class="muted">These were in-flight when the app stopped.</p>
              <ul>
                {#each interrupted as step (step.step_id)}
                  <li class="warn-row">
                    <span class="status-badge status-warning resume-status">interrupted</span> {kindLabel(step)}
                  </li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if uncertain.length > 0}
            <section>
              <h3>Unconfirmed side-effects ({uncertain.length})</h3>
              <p class="muted">
                These effects could not be verified - review before continuing.
              </p>
              <ul>
                {#each uncertain as effect (effect.step_id)}
                  <li class="warn-row">
                    <span class="status-badge status-warning resume-status">{effect.confidence}</span> {effectLabel(effect)}
                  </li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if skipped.length === 0 && !hasWarnings}
            <p class="muted">Nothing needs attention - safe to resume.</p>
          {/if}
        {/if}

        <label class="skip-toggle">
          <input type="checkbox" bind:checked={skipCompletedWriteSteps} />
          Skip completed write-steps (recommended)
        </label>
      </div>

      <footer class="modal-footer">
        <button type="button" class="lattice-btn lattice-btn--secondary" on:click={cancel} disabled={confirming}>
          Cancel
        </button>
        <button type="button" class="lattice-btn lattice-btn--primary lattice-btn--filled" on:click={confirm} disabled={loading || confirming} aria-busy={confirming}>
          {confirming ? 'Resuming...' : 'Resume'}
        </button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    color: var(--text-primary);
    border-radius: var(--radius-lg);
    width: min(560px, 92vw);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    padding: 1rem 1.25rem;
    box-shadow: var(--edge-seam);
  }

  .modal-header h2 {
    margin: 0;
    font-size: 1.05rem;
  }

  .modal-body {
    padding: 1rem 1.25rem;
    overflow-y: auto;
  }

  section {
    margin-bottom: 1rem;
  }

  section h3 {
    margin: 0 0 0.25rem;
    font-size: 0.9rem;
  }

  ul {
    list-style: none;
    padding: 0;
    margin: 0.25rem 0 0;
  }

  li {
    padding: 0.25rem 0;
    font-size: 0.85rem;
  }

  .warn-row {
    color: var(--text-warning, #e0af68);
  }

  .resume-status {
    margin-right: 0.4rem;
  }

  .muted {
    color: var(--text-secondary, #787c99);
    font-size: 0.8rem;
    margin: 0.25rem 0;
  }

  .error-text {
    color: var(--status-error, #f7768e);
    font-size: 0.82rem;
    margin: 0.25rem 0;
  }

  .skip-toggle {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.75rem;
    font-size: 0.85rem;
  }

  .modal-footer {
    padding: 0.85rem 1.25rem;
    box-shadow: var(--edge-seam-top);
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }

</style>
