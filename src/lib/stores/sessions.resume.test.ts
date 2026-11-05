import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { resumeBlockedReason, sessions, type Session } from './sessions';

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
  it('retains the returned unavailable-agent report until dismissed, then clears it for the next load', async () => {
    const saved = { ...savedSession('resume-store-report'), name: 'Synthetic report',
      agent_resume_report: ['Synthetic principal: no saved conversation ID'] };
    vi.mocked(invoke).mockResolvedValueOnce(saved);
    await sessions.resumeSession(saved.id);
    expect(get(sessions).resumeNotice).toEqual({ sessionName: saved.name,
      notices: [{ agent_id: null, reason: 'legacy', message: saved.agent_resume_report[0] }] });
    sessions.dismissResumeNotice();
    expect(get(sessions).resumeNotice).toBeNull();
    vi.mocked(invoke).mockResolvedValueOnce(savedSession('resume-store-no-report'));
    await sessions.resumeSession('resume-store-no-report');
    expect(get(sessions).resumeNotice).toBeNull();
  });

  it('keeps structured blockers per agent and leaves terminal manual Resume available', async () => {
    const saved = savedSession('resume-store-structured');
    saved.state = 'Closed';
    saved.agent_resume_report = [
      { agent_id: saved.agents[0].id, reason: 'invalid_working_directory',
        message: 'Synthetic coordinator: saved working directory no longer exists; Resume is unavailable.' },
      { agent_id: saved.agents[1].id, reason: 'manual_resume_required',
        message: 'Synthetic principal: saved session has ended; use per-agent Resume.' },
    ];
    vi.mocked(invoke).mockResolvedValueOnce(saved);
    await expect(sessions.resumeSession(saved.id)).resolves.toEqual(saved);
    expect(get(sessions).resumeNotice).toEqual({ sessionName: 'synthetic', notices: saved.agent_resume_report });
    expect(get(sessions).error).toBeNull();
    expect(get(sessions).activeSessionId).toBe(saved.id);
    expect(resumeBlockedReason(saved, saved.agents[0].id)).toContain('working directory');
    expect(resumeBlockedReason(saved, saved.agents[1].id)).toBeNull();
    expect(resumeBlockedReason(saved, 'unrelated-agent')).toBeNull();
  });

  it('does not infer a blocked agent from legacy text or a missing report', () => {
    const saved = savedSession('resume-store-legacy-lookup');
    saved.agent_resume_report = [`${saved.agents[0].id}: no saved working directory`];
    expect(resumeBlockedReason(saved, saved.agents[0].id)).toBeNull();
    expect(resumeBlockedReason(savedSession('resume-store-empty'), 'queen')).toBeNull();
    expect(resumeBlockedReason(null, 'queen')).toBeNull();
    expect(resumeBlockedReason(undefined, 'queen')).toBeNull();
  });

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
