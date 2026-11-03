import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { sessions, type Session } from './sessions';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

function savedSession(id: string): Session {
  return {
    id, session_type: { Hive: { worker_count: 1 } }, state: 'Running',
    project_path: 'C:/synthetic', created_at: '2026-11-03T12:00:00Z',
    agents: ['queen', 'worker-1'].map((suffix) => ({
      id: `${id}-${suffix}`, role: suffix === 'queen' ? 'Queen' : { Worker: { index: 1, parent: null } },
      status: 'Completed', config: { cli: 'claude', flags: [] }, parent_id: null,
    })),
  };
}

beforeEach(() => vi.clearAllMocks());

describe('G49 per-agent resume store', () => {
  it('requests only the selected agent and replaces the returned roster', async () => {
    const saved = savedSession('resume-store-success');
    vi.mocked(invoke).mockResolvedValueOnce(saved);
    await sessions.resumeSession(saved.id);
    vi.mocked(invoke).mockClear();
    const resumed: Session = { ...saved, agents: saved.agents.map((agent, index) => index === 1 ? { ...agent, status: 'Running' } : agent) };
    vi.mocked(invoke).mockResolvedValueOnce(resumed);
    expect(await sessions.resumeAgent(saved.id, saved.agents[1].id)).toEqual(resumed);
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith('resume_agent', { sessionId: saved.id, agentId: saved.agents[1].id });
    const state = get(sessions);
    expect(state.activeSessionId).toBe(saved.id);
    expect(state.sessions.find((session) => session.id === saved.id)?.agents.map((agent) => agent.status)).toEqual(['Completed', 'Running']);
  });

  it('keeps the saved roster when the backend refuses a missing worktree', async () => {
    const saved = savedSession('resume-store-refused');
    vi.mocked(invoke).mockResolvedValueOnce(saved);
    await sessions.resumeSession(saved.id);
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockRejectedValueOnce('Saved working directory no longer exists');
    await expect(sessions.resumeAgent(saved.id, saved.agents[1].id)).rejects.toContain('working directory');
    expect(invoke).toHaveBeenCalledOnce();
    const state = get(sessions);
    expect(state.sessions.find((session) => session.id === saved.id)?.agents).toEqual(saved.agents);
    expect(state.error).toContain('working directory');
  });
});
