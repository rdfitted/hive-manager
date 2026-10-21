// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('$lib/stores/sessions', async () => {
  const { derived, writable } = await import('svelte/store');
  const activeSession = writable({ id: 'session-a', agents: [
    { id: 'worker-a', role: 'Queen', status: 'Running', config: { label: 'Worker A' } },
  ] });
  return { activeSession, activeAgents: derived(activeSession, (session) => session.agents), sessions: writable([]) };
});
vi.mock('$lib/stores/conversations', async () => {
  const { writable } = await import('svelte/store');
  const state = writable({ sessionId: 'session-a', selectedAgent: null as string | null, messages: [], loading: false, error: null });
  return {
    conversationStore: {
      subscribe: state.subscribe, setSessionId: vi.fn(), loadConversation: vi.fn(),
      selectAgent: (agent: string) => state.update((s) => ({ ...s, selectedAgent: agent })),
      sendMessage: vi.fn(), clearError: vi.fn(),
    },
    heartbeatStore: { subscribe: writable({ agents: {}, stalledAgents: new Set(), staleAgents: new Set() }).subscribe, loadHeartbeats: vi.fn() },
  };
});
const mounts = vi.hoisted(() => ({ logs: vi.fn(), timeline: vi.fn(), replay: vi.fn(), controls: vi.fn() }));
vi.mock('../CoordinationPanel.svelte', () => ({ default: mounts.logs }));
vi.mock('../timeline/TimelineView.svelte', () => ({ default: mounts.timeline }));
vi.mock('../replay/ReplayView.svelte', () => ({ default: mounts.replay }));
vi.mock('../replay/ReplayControls.svelte', () => ({ default: mounts.controls }));
import ActivityTab from './ActivityTab.svelte';

afterEach(() => { cleanup(); vi.clearAllMocks(); });

describe('Activity rail composition', () => {
  it('opens Chat with the real composer reachable in one click', async () => {
    const view = render(ActivityTab);
    expect(view.getByRole('button', { name: 'Chat' }).getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(view.getByRole('tab', { name: 'Queen' }));
    expect(view.getByRole('textbox', { name: 'Send message as operator… (@ to mention, / for commands)' })).toBeTruthy();
  });

  it('mounts both replay views in an initially collapsed disclosure, plus Logs and Timeline', async () => {
    const view = render(ActivityTab);
    expect(view.getByText('Replay').closest('details')?.open).toBe(false);
    expect(mounts.replay).toHaveBeenCalled();
    expect(mounts.controls).toHaveBeenCalled();
    await fireEvent.click(view.getByRole('button', { name: 'Logs' }));
    expect(mounts.logs).toHaveBeenCalled();
    await fireEvent.click(view.getByRole('button', { name: 'Timeline' }));
    expect(mounts.timeline).toHaveBeenCalled();
  });
});
