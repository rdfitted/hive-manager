import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { AgentEvent } from '$lib/stores/agentEvents';
import { CHAT_WINDOW_SIZE, passiveToolResult, protocolAttention, transcriptRows } from './transcript';

const fixture = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined), loadOlder: vi.fn(), setAgents: (_agents: import('$lib/stores/sessions').AgentInfo[]) => {}, setSource: (_source: import('$lib/stores/agentEvents').AgentFeed['capabilities']['event_source']) => {} }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('$lib/stores/agentEvents', async () => {
  const { writable } = await import('svelte/store');
  const state = writable({ sessionId: 's', histories: {}, feeds: {} });
  fixture.setSource = (event_source) => state.set({ sessionId: 's', histories: {}, feeds: { a: { capabilities: { event_source } } } });
  return { agentEvents: { subscribe: state.subscribe, loadOlder: fixture.loadOlder } };
});
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  const agents = writable<import('$lib/stores/sessions').AgentInfo[]>([]);
  fixture.setAgents = agents.set;
  return { sessions: writable({ sessions: [] }), activeAgents: agents };
});
import ChatPane from './ChatPane.svelte';
import PaneToggle from './PaneToggle.svelte';

export const event = (cursor: string, kind: AgentEvent['kind'] = 'assistant_message', payload: Record<string, unknown> = { text: cursor }, source_id: string | null = cursor): AgentEvent => ({
  agent_id: 'a', cursor, kind, payload, source_id, ts: '2026-01-01T00:00:00Z', truncated: false,
});
afterEach(() => { cleanup(); fixture.setSource('none'); fixture.setAgents([]); vi.clearAllMocks(); });

describe('agent chat', () => {
  it('T15: shows known toolbar facts and names the target agent in the placeholder', async () => {
    fixture.setAgents([{ ...runningAgent(), config: { cli: 'codex', label: 'Frontend', model: 'fixture-model', flags: ['-c', 'model_reasoning_effort="high"'] } }]);
    fixture.setSource('transcript');
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [] } });
    const editor = view.getByRole('textbox', { name: 'Message Frontend' });
    editor.focus();
    expect(view.getByText('Full access')).toBeTruthy();
    expect(view.getByText('fixture-model')).toBeTruthy();
    expect(view.getByText('high')).toBeTruthy();
    editor.textContent = 'Fixture toolbar send'; await fireEvent.input(editor);
    await fireEvent.click(view.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', {
      request: { session_id: 's', target_agent_id: 'a', message: 'Fixture toolbar send' },
    }));
    expect(document.activeElement).toBe(editor);
    fixture.setAgents([]);
  });

  it('T15: omits unknown and protocol access facts', async () => {
    fixture.setAgents([runningAgent()]); fixture.setSource('protocol');
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [] } });
    expect(view.queryByText('Full access')).toBeNull();
    expect(view.container.querySelector('.model')).toBeNull();
    expect(view.container.querySelector('.effort')).toBeNull();
    fixture.setAgents([]); await tick();
    expect(view.getByRole('textbox', { name: 'Message this agent…' })).toBeTruthy();
  });
  it('T15: removes a portalled toolbar menu when chat is hidden without taking focus', async () => {
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [] } });
    const editor = view.getByRole('textbox'); editor.focus();
    await fireEvent.click(view.getByRole('button', { name: 'Add mention or command' }));
    expect(view.getByRole('menu', { name: 'Add to message' })).toBeTruthy();
    await view.rerender({ sessionId: 's', agentId: 'a', events: [], visible: false });
    expect(view.queryByRole('menu', { name: 'Add to message' })).toBeNull();
    expect(document.activeElement).toBe(editor);
  });
  it('renders replacement assistant snapshots, deltas and distinct user source identities', () => {
    const rows = transcriptRows([
      event('1', 'assistant_delta', { text: 'Hel' }, 'message'), event('2', 'assistant_delta', { text: 'lo' }, 'message'),
      event('3', 'assistant_message', { text: 'Hello world' }, 'message'), event('4', 'user_message', { text: 'Continue' }, 'message'),
      event('5', 'status', { status: 'needs_approval' }),
    ]);
    expect(rows.map((row) => row.text)).toEqual(['Hello world', 'Continue']);
    expect(new Set(rows.map((row) => row.id)).size).toBe(2);
  });

  it('joins tool starts/results, starts collapsed, and never presents transcript approval actions', async () => {
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [
      event('1', 'tool_start', { tool_id: 't', name: 'Fixture tool', input: { path: 'fixture.txt' } }),
      event('2', 'tool_result', { tool_id: 't', result: { renderer: 'approval', data: { actionId: 'unsafe' } } }),
      event('3', 'status', { status: 'needs_input', message: 'Protocol prompt' }),
    ] } });
    expect(view.container.querySelectorAll('details')).toHaveLength(1);
    const details = view.container.querySelector('details')!;
    expect(details.open).toBe(false);
    expect(details.querySelector('[aria-label="Tool input"]')).toBeNull();
    expect(details.querySelector('summary')?.textContent).toBe('Worked for 0s');
    details.open = true;
    await fireEvent(details, new Event('toggle'));
    expect(details.querySelector('[aria-label="Tool input"]')?.textContent).toContain('fixture.txt');
    expect(view.queryByRole('button', { name: /approve|reject/i })).toBeNull();
    expect(view.queryByText('Protocol prompt')).toBeNull();
    expect(passiveToolResult({ renderer: 'approval' })).toBeNull();
    expect(passiveToolResult({ renderer: 'table', data: [] })?.renderer).toBe('table');
  });

  it('windows five thousand messages, pages older/newer and returns to latest', async () => {
    const events = Array.from({ length: 5000 }, (_, index) => event(`fixture-${index}`));
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events } });
    expect(view.getAllByRole('article', { name: 'Assistant message' })).toHaveLength(CHAT_WINDOW_SIZE);
    expect(view.getByText('4921–5000 of 5000')).toBeTruthy();
    expect(view.queryByText('fixture-0')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Older' }));
    expect(view.getByText('4841–4920 of 5000')).toBeTruthy();
    expect(view.getAllByRole('article', { name: 'Assistant message' })).toHaveLength(CHAT_WINDOW_SIZE);
    await fireEvent.click(view.getByRole('button', { name: 'Newer' }));
    await fireEvent.click(view.getByRole('button', { name: 'Latest' }));
    expect(view.getByText('fixture-4999')).toBeTruthy();
  });

  it('reuses Composer and sends only through operator_inject to this agent', async () => {
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [] } });
    const editor = view.getByRole('textbox', { name: 'Message this agent…' });
    editor.textContent = 'Continue with fixture';
    await fireEvent.input(editor);
    await fireEvent.keyDown(editor, { key: 'Enter' });
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', {
      request: { session_id: 's', target_agent_id: 'a', message: 'Continue with fixture' },
    }));
    expect(fixture.invoke).toHaveBeenCalledTimes(1);
  });

  it('holds the current reading window when new live records arrive after scrolling up', async () => {
    const events = Array.from({ length: 200 }, (_, index) => event(`fixture-${index}`));
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events } });
    await tick();
    const transcript = view.container.querySelector<HTMLElement>('.transcript')!;
    Object.defineProperties(transcript, { scrollHeight: { value: 1000 }, clientHeight: { value: 100 } });
    transcript.scrollTop = 200;
    await fireEvent.scroll(transcript);
    expect(view.getByRole('button', { name: 'Latest' })).toBeTruthy();
    await view.rerender({ sessionId: 's', agentId: 'a', events: [...events, event('new-live')] });
    expect(view.getByText('fixture-120')).toBeTruthy();
    expect(view.queryByText('new-live')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Latest' }));
    expect(view.getByText('new-live')).toBeTruthy();
  });

  it('offers a raw log instead of a Terminal toggle when there is no terminal, with bounded raw records', async () => {
    const change = vi.fn();
    const toggle = render(PaneToggle, { hasTerminal: false, mode: 'chat', onchange: change });
    expect(toggle.queryByRole('button', { name: 'Terminal' })).toBeNull();
    await fireEvent.click(toggle.getByRole('button', { name: 'Raw event log' }));
    expect(change).toHaveBeenCalledWith('raw');
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', raw: true, events: Array.from({ length: 5000 }, (_, i) => event(`raw-${i}`, 'status', { status: 'needs_approval' })) } });
    expect(view.getByRole('region', { name: 'Raw agent event log' })).toBeTruthy();
    expect(view.getAllByRole('article', { name: 'status' })).toHaveLength(CHAT_WINDOW_SIZE);
  });

  it('keeps approval and input attention until a later protocol state resolves it', () => {
    expect(protocolAttention([event('1', 'status', { status: 'needs_approval', message: 'Check fixture' })])).toBe('Approval required. Check fixture');
    expect(protocolAttention([event('2', 'status', { status: 'needs_input' })])).toBe('Agent needs input.');
    expect(protocolAttention([event('1', 'status', { status: 'needs_input' }), event('2', 'turn_start')])).toBeNull();
  });

  it('F2: renders declined decisions and legacy failed turns as persistent labelled rows', async () => {
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [
      event('1', 'status', { status: 'needs_approval', message: 'Hive declined it; use PTY.', request_id: 7 }),
      event('2', 'turn_end', { status: 'failed', error: { message: 'Fixture failure' } }),
      event('3', 'status', { status: 'subagent_activity' }),
    ] } });
    expect(view.getByRole('article', { name: 'Approval decision' }).textContent).toContain('Hive declined it; use PTY.');
    expect(view.getByRole('article', { name: 'Agent error' }).textContent).toContain('Fixture failure');
    expect(view.queryByRole('button', { name: /approve|reject/i })).toBeNull();
  });
});

// G19 component proof: collapsing keeps trouble visible and never steals focus.
import type { AgentInfo } from '$lib/stores/sessions';
function runningAgent(status: AgentInfo['status'] = 'Running'): AgentInfo {
  return { id: 'a', role: 'Queen', status, config: { cli: 'codex', flags: [] }, parent_id: null };
}

describe('G19 collapsed activity and message layout', () => {
  afterEach(() => { fixture.setAgents([]); vi.useRealTimers(); });

  it('G19: failed count remains visible while all tool details are collapsed', () => {
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [
      event('1', 'tool_start', { tool_id: 'a', name: 'exec', input: 'tools.exec_command({})' }),
      event('2', 'tool_result', { tool_id: 'a', result: 'fixture' }),
      event('3', 'tool_start', { tool_id: 'b', name: 'exec' }),
      event('4', 'status', { status: 'tool_failed', exit_code: 1 }),
      event('5', 'tool_result', { tool_id: 'b', result: 'fixture' }),
    ] } });
    expect(view.container.querySelectorAll('details')).toHaveLength(1);
    expect(view.container.querySelector('details')?.open).toBe(false);
    expect(view.container.querySelector('summary')?.textContent, 'collapsed activity must expose the failed count').toContain('1 failed');
    expect(view.queryByLabelText('Tool result')).toBeNull();
  });

  it('preserves expansion when another tool is appended, without moving editor focus', async () => {
    const events = [event('1', 'tool_start', { tool_id: 'a', name: 'Read', input: { path: 'fixture.txt' } })];
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events } });
    const editor = view.getByRole('textbox'); editor.focus();
    const details = view.container.querySelector('details')!;
    details.open = true; await fireEvent(details, new Event('toggle'));
    await view.rerender({ sessionId: 's', agentId: 'a', events: [...events, event('2', 'tool_start', { tool_id: 'b', name: 'Read' })] });
    expect(view.container.querySelector('details')).toBe(details);
    expect(details.open).toBe(true);
    expect(view.getByLabelText('Tool input').textContent).toContain('fixture.txt');
    expect(document.activeElement).toBe(editor);
  });

  it('shows live labels, quiet terminal guidance, and closes on turn end or agent exit', async () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));
    fixture.setAgents([runningAgent()]);
    const events = [event('1', 'tool_start', { tool_id: 'a', name: 'exec', input: 'tools.exec_command({})' })];
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events } });
    expect(view.getByRole('status').textContent).toBe('Running bash');
    await vi.advanceTimersByTimeAsync(90000); await tick();
    const quietStatus = 'Waiting without an event · Check the terminal';
    expect(view.getByRole('status').textContent, 'quiet live region has a stable sentence').toBe(quietStatus);
    for (const seconds of [90, 91, 92]) {
      if (seconds > 90) { await vi.advanceTimersByTimeAsync(1000); await tick(); }
      expect(view.getByRole('status').textContent, 'elapsed ticks do not change accessible status').toBe(quietStatus);
      const elapsed = view.getByText(`Waiting for ${seconds}s without an event · Check the terminal`);
      expect(elapsed.getAttribute('aria-hidden')).toBe('true');
      expect(elapsed.closest('[role="status"]'), 'elapsed counter is outside the live region').toBeNull();
    }
    const resumed = { ...event('resumed', 'tool_result', { tool_id: 'a' }), ts: new Date().toISOString() };
    await view.rerender({ sessionId: 's', agentId: 'a', events: [...events, resumed] });
    expect(view.getByRole('status').textContent).toBe('Thinking');
    fixture.setAgents([runningAgent('Completed')]); await tick();
    expect(view.queryByRole('status')).toBeNull();
    fixture.setAgents([runningAgent()]);
    await view.rerender({ sessionId: 's', agentId: 'a', events: [...events, event('end', 'turn_end', { status: 'aborted' })] });
    expect(view.queryByRole('status')).toBeNull();
  });

  it('clamps a long user bubble with Show more and gives plain assistant text a hover timestamp', async () => {
    const text = Array.from({ length: 9 }, (_, i) => `Fixture line ${i}`).join('\n');
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [event('u', 'user_message', { text }), event('a')] } });
    const user = view.getByRole('article', { name: 'User message' });
    expect(user.classList.contains('user-message')).toBe(true);
    expect(user.querySelector('p')?.classList.contains('clamped')).toBe(true);
    await fireEvent.click(view.getByRole('button', { name: 'Show more' }));
    expect(user.querySelector('p')?.classList.contains('clamped')).toBe(false);
    const assistant = view.getByRole('article', { name: 'Assistant message' });
    expect(assistant.querySelector('header')).toBeNull();
    expect(assistant.getAttribute('title')).toBeTruthy();
    expect(view.queryByRole('navigation', { name: 'Transcript history' })).toBeNull();
  });

  it('offers Show more for short prose that overflows six lines in a narrow pane', async () => {
    let measure: (() => void) | undefined;
    vi.stubGlobal('ResizeObserver', class {
      constructor(callback: () => void) { measure = callback; }
      observe() {} disconnect() {}
    });
    try {
      const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events: [event('u', 'user_message', { text: 'Fixture prose '.repeat(20) })] } });
      const p = view.getByRole('article', { name: 'User message' }).querySelector('p')!;
      Object.defineProperties(p, { scrollHeight: { value: 240 }, clientHeight: { value: 120 } });
      measure?.(); await tick();
      expect(view.getByRole('button', { name: 'Show more' })).toBeTruthy();
      await fireEvent.click(view.getByRole('button', { name: 'Show more' }));
      expect(p.classList.contains('clamped')).toBe(false);
    } finally { vi.unstubAllGlobals(); }
  });

  it('keeps a full group before windowing and isolates attention, error and decision boundaries', () => {
    const events = [...Array.from({ length: 120 }, (_, i) => event(`tool-${i}`, 'tool_start', { tool_id: String(i), name: 'Read' })),
      event('error', 'error', { message: 'Visible fixture error' }),
      event('approval', 'status', { status: 'needs_approval', decision: 'decline', message: 'Visible fixture decision' })];
    const view = render(ChatPane, { props: { sessionId: 's', agentId: 'a', events } });
    expect(view.container.querySelectorAll('details')).toHaveLength(1);
    expect(view.getByRole('article', { name: 'Agent error' }).closest('details')).toBeNull();
    expect(view.getByRole('article', { name: 'Approval decision' }).closest('details')).toBeNull();
    expect(view.queryByRole('navigation', { name: 'Transcript history' })).toBeNull();
  });
});
