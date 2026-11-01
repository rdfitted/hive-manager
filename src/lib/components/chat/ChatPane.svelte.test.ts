import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { AgentEvent } from '$lib/stores/agentEvents';
import { CHAT_WINDOW_SIZE, passiveToolResult, protocolAttention, transcriptRows } from './transcript';

const fixture = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined), loadOlder: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('$lib/stores/agentEvents', async () => {
  const { writable } = await import('svelte/store');
  return { agentEvents: { subscribe: writable({ sessionId: 's', histories: {} }).subscribe, loadOlder: fixture.loadOlder } };
});
vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return { sessions: writable({ sessions: [] }), activeAgents: writable([]) };
});
import ChatPane from './ChatPane.svelte';
import PaneToggle from './PaneToggle.svelte';

export const event = (cursor: string, kind: AgentEvent['kind'] = 'assistant_message', payload: Record<string, unknown> = { text: cursor }, source_id: string | null = cursor): AgentEvent => ({
  agent_id: 'a', cursor, kind, payload, source_id, ts: '2026-01-01T00:00:00Z', truncated: false,
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });

describe('agent chat', () => {
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
    expect(details.querySelector('summary')?.textContent).toBe('Fixture tool · Complete');
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
