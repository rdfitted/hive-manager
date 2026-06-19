import { writable } from 'svelte/store';
import { apiUrl } from '$lib/config';

/**
 * One-shot operator-selection context (#128 Ctrl+I "select -> instruct").
 *
 * The operator selects text in a terminal / the page (or a session cell), presses Ctrl+I,
 * and the selection is captured here. The NEXT composer submit consumes it exactly once,
 * prepending a fenced `[Operator context]` block to the outgoing prompt, then it expires.
 *
 * Source of truth for the one-shot guarantee is #124's `application_state` table via the
 * atomic `take` endpoint (read-and-delete in a single transaction): a lagging/double submit
 * cannot re-send the same context because the row is gone after the first take. A thin
 * in-memory mirror is kept for synchronous UI affordances (e.g. showing "context armed"),
 * but `consume()` always goes through the backend take so concurrency is handled server-side.
 */

export type PendingContextKind = 'cell' | 'selection';

export interface PendingContext {
  sessionId: string;
  agentId: string | null;
  kind: PendingContextKind;
  /** Present for kind === 'cell'. */
  cellId?: string | null;
  /** Selected text (normalized) for kind === 'selection'. */
  text?: string | null;
  /** Epoch millis when captured. */
  capturedAt: number;
}

/** The key under which the one-shot context lives in `application_state`. */
export const PENDING_CONTEXT_KEY = 'pending_selection_context';

interface PendingContextState {
  /** The latest armed context, or null when nothing is pending. */
  current: PendingContext | null;
}

function createPendingContextStore() {
  const { subscribe, set } = writable<PendingContextState>({ current: null });

  /** In-memory mirror for synchronous reads (UI badge etc.). Backend is authoritative. */
  let mirror: PendingContext | null = null;

  /**
   * Render the operator context as a fenced block to prepend to a prompt.
   * Exported as a pure helper so the composer (and tests) format it identically.
   */
  function render(ctx: PendingContext): string {
    const body =
      ctx.kind === 'cell'
        ? `Selected session cell: ${ctx.cellId ?? '(unknown)'}`
        : (ctx.text ?? '').trim();
    return `[Operator context]\n${body}\n[/Operator context]`;
  }

  return {
    subscribe,

    /**
     * Arm a one-shot context. Writes to the backend `application_state` (key
     * `pending_selection_context`) so it survives until consumed, and overwrites any
     * prior pending context (capture-twice keeps only the latest).
     */
    async capture(ctx: PendingContext): Promise<void> {
      mirror = ctx;
      set({ current: ctx });
      try {
        await fetch(apiUrl(`/api/sessions/${ctx.sessionId}/application-state`), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ key: PENDING_CONTEXT_KEY, value: ctx }),
        });
      } catch {
        // Best-effort: the in-memory mirror still lets consume() fall back below.
      }
    },

    /**
     * Read-and-clear the pending context for a session, returning it once then null.
     * Goes through the atomic backend take endpoint so a double/lagging submit can never
     * re-consume. Falls back to the in-memory mirror only if the backend is unreachable.
     */
    async consume(sessionId: string): Promise<PendingContext | null> {
      let taken: PendingContext | null = null;
      try {
        const resp = await fetch(
          apiUrl(`/api/sessions/${sessionId}/application-state/take`),
          {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ key: PENDING_CONTEXT_KEY }),
          }
        );
        if (resp.ok) {
          const row = (await resp.json()) as { value: PendingContext } | null;
          taken = row?.value ?? null;
        } else {
          // Backend rejected — fall back to the mirror so the feature still works offline.
          taken = mirror;
        }
      } catch {
        taken = mirror;
      }
      // Clear the mirror + store regardless: exactly-one-turn.
      mirror = null;
      set({ current: null });
      return taken;
    },

    /** Synchronous peek at the in-memory mirror (does NOT consume). */
    peek(): PendingContext | null {
      return mirror;
    },

    /** Discard any pending context without sending (e.g. session switch). */
    clear() {
      mirror = null;
      set({ current: null });
    },

    render,
  };
}

export const pendingContext = createPendingContextStore();
