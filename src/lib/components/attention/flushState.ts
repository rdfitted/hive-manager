import { get, writable } from 'svelte/store';
import { flushEnter } from './flush';

export interface FlushStateEntry {
  pending: boolean;
  receipt?: string;
  error?: string;
}

export const flushKey = (sessionId: string, agentId: string): string => JSON.stringify([sessionId, agentId]);

/** The grid owns this state above its Focus/Grid presentation branches. */
export function createFlushState() {
  const state = writable<Record<string, FlushStateEntry>>({});
  return {
    subscribe: state.subscribe,
    async flush(sessionId: string, agentId: string) {
      const key = flushKey(sessionId, agentId);
      if (get(state)[key]?.pending) return;
      const write = (entry: FlushStateEntry) => state.update((entries) => ({ ...entries, [key]: entry }));
      write({ pending: true });
      try {
        write({ pending: false, receipt: await flushEnter(sessionId, agentId) });
      } catch (cause) {
        write({ pending: false, error: String(cause) });
      }
    },
    dismiss(sessionId: string, agentId: string) {
      const key = flushKey(sessionId, agentId);
      state.update((entries) => {
        const { [key]: dismissed, ...remaining } = entries;
        return remaining;
      });
    },
  };
}

export type FlushState = ReturnType<typeof createFlushState>;
