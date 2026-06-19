<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { ResumeReport, RunJournalEntry, LedgerEntry } from '$lib/stores/sessions';

  /** Whether the modal is visible. */
  export let open = false;
  /** The session being resumed (for the title). */
  export let sessionName: string | null = null;
  /** The resume classification produced by the backend on resume. */
  export let report: ResumeReport | null = null;

  // Default-checked: skip already-completed write-steps so destructive git ops
  // (commits, branch/worktree creation, worker/evaluator spawns) are not re-run.
  let skipCompletedWriteSteps = true;

  const dispatch = createEventDispatcher<{
    confirm: { skipCompletedWriteSteps: boolean };
    cancel: void;
  }>();

  $: skipped = report?.skipped ?? [];
  $: interrupted = report?.interrupted ?? [];
  $: uncertain = report?.uncertain ?? [];
  $: hasWarnings = interrupted.length > 0 || uncertain.length > 0;

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
    class="modal-backdrop"
    role="presentation"
    on:click={cancel}
    on:keydown={(e) => e.key === 'Escape' && cancel()}
  >
    <div
      class="modal"
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

      <div class="modal-body">
        {#if !report}
          <p class="muted">No prior run journal — resuming a clean session.</p>
        {:else}
          {#if skipped.length > 0}
            <section>
              <h3>Completed steps ({skipped.length})</h3>
              <p class="muted">These write-steps already finished and will not be re-run.</p>
              <ul>
                {#each skipped as step (step.step_id)}
                  <li><span class="badge done">done</span> {kindLabel(step)}</li>
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
                    <span class="badge warn">interrupted</span> {kindLabel(step)}
                  </li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if uncertain.length > 0}
            <section>
              <h3>Unconfirmed side-effects ({uncertain.length})</h3>
              <p class="muted">
                These effects could not be verified — review before continuing.
              </p>
              <ul>
                {#each uncertain as effect (effect.step_id)}
                  <li class="warn-row">
                    <span class="badge warn">{effect.confidence}</span> {effectLabel(effect)}
                  </li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if skipped.length === 0 && !hasWarnings}
            <p class="muted">Nothing needs attention — safe to resume.</p>
          {/if}
        {/if}

        <label class="skip-toggle">
          <input type="checkbox" bind:checked={skipCompletedWriteSteps} />
          Skip completed write-steps (recommended)
        </label>
      </div>

      <footer class="modal-footer">
        <button type="button" class="secondary" on:click={cancel}>Cancel</button>
        <button type="button" class="primary" on:click={confirm}>Resume</button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }
  .modal {
    background: var(--bg-elevated, #1a1b26);
    color: var(--text-primary, #c0caf5);
    border: 1px solid var(--border, #2a2e42);
    border-radius: 8px;
    width: min(560px, 92vw);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-header {
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--border, #2a2e42);
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
  .badge {
    display: inline-block;
    font-size: 0.7rem;
    padding: 0.05rem 0.4rem;
    border-radius: 4px;
    margin-right: 0.4rem;
    text-transform: uppercase;
  }
  .badge.done {
    background: rgba(158, 206, 106, 0.18);
    color: var(--text-success, #9ece6a);
  }
  .badge.warn {
    background: rgba(224, 175, 104, 0.18);
    color: var(--text-warning, #e0af68);
  }
  .muted {
    color: var(--text-secondary, #787c99);
    font-size: 0.8rem;
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
    border-top: 1px solid var(--border, #2a2e42);
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
  button {
    padding: 0.4rem 0.9rem;
    border-radius: 6px;
    border: 1px solid var(--border, #2a2e42);
    cursor: pointer;
    font-size: 0.85rem;
  }
  button.primary {
    background: var(--accent, #7aa2f7);
    color: #0b0c14;
    border-color: var(--accent, #7aa2f7);
  }
  button.secondary {
    background: transparent;
    color: var(--text-primary, #c0caf5);
  }
</style>
