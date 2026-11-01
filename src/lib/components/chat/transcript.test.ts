import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '$lib/stores/agentEvents';
import { protocolAttention, transcriptRows } from './transcript';

const event = (kind: AgentEvent['kind'], payload: Record<string, unknown>, source_id = 'held:turn'): AgentEvent => ({
  agent_id: 'fixture-agent', cursor: `${kind}-${source_id}`, ts: '2026-01-01T00:00:00Z', kind, source_id, truncated: false, payload,
});
// Normalized D/E envelopes, including the older journal shapes without an explicit error.
const failures = [
  ['Codex', { status: 'failed', error: { message: 'Fixture turn failed', additionalDetails: null } }, 'Fixture turn failed'],
  ['Claude', { turn_id: 'fixture-turn', is_error: true }, 'Agent turn failed.'],
] as const;
const boundaries = [event('turn_end', { status: 'completed', error: null }), event('turn_start', { status: 'started' }),
  event('status', { status: 'subagent_activity', message: 'Sub-agent activity', parent_thread_id: 'held' })];

describe('failure and decision transcript attention', () => {
  it.each(failures)('F2: legacy %s failed turn_end remains a visible error and attention across unrelated boundaries/status', (_provider, payload, text) => {
    const events = [event('turn_end', payload), ...boundaries];
    expect(transcriptRows(events).map((row) => [row.type, row.text])).toEqual([['error', text]]);
    expect(protocolAttention(events), 'failed turn attention must survive boundaries and subagent activity').toBe(text);
  });

  it.each(failures)('F2: explicit %s error survives its completion without losing useful detail or duplicating the row', (_provider, payload) => {
    const events = [event('error', { message: 'Useful fixture failure' }), event('turn_end', payload), ...boundaries];
    expect(protocolAttention(events), 'explicit error must survive turn_end').toBe('Useful fixture failure');
    expect(transcriptRows(events).map((row) => row.text)).toEqual(['Useful fixture failure']);
    expect(protocolAttention([...events, event('user_message', { text: 'Retry' })])).toBeNull();
    expect(protocolAttention([...events, event('status', { status: 'error_resolved' })])).toBeNull();
    expect(transcriptRows([...events, event('user_message', { text: 'Retry' })])[0].text).toBe('Useful fixture failure');
  });

  it.each([
    ['Codex', 'Codex requested approval or input. Hive declined it; use PTY for interactive approval.', 7],
    ['Claude', 'Claude requested an interactive decision. It was declined; select PTY to approve.', 'approval-1'],
  ])('F2: %s declined approval stays in the transcript and attention until user action or explicit resolution', (_provider, message, request_id) => {
    const events = [event('status', { status: 'needs_approval', message, request_id }, String(request_id)), ...boundaries];
    expect(protocolAttention(events), 'declined decision must survive boundaries and unrelated status').toBe(message);
    expect(transcriptRows(events).map((row) => [row.type, row.text])).toEqual([['decision', message]]);
    expect(protocolAttention([...events, event('status', { status: 'approval_resolved' })])).toBeNull();
    expect(protocolAttention([...events, event('user_message', { text: 'Continue' })])).toBeNull();
    expect(transcriptRows([...events, event('user_message', { text: 'Continue' })])[0].text).toBe(message);
  });

  it('keeps unresolved failures when an approval alone resolves, and pending requests across unrelated status', () => {
    const error = event('error', { message: 'Failure' });
    expect(protocolAttention([error, event('status', { status: 'approval_resolved' })])).toBe('Failure');
    expect(protocolAttention([event('status', { status: 'needs_input' }), boundaries[2]])).toBe('Agent needs input.');
    expect(protocolAttention([error, event('status', { status: 'resolved' })])).toBeNull();
  });
});

// Scrubbed fixtures exercise only normalized public event shapes.
import { liveActivity } from './transcript';
import { activityLabel } from './activityLabel';
const activity = (id: string, kind: AgentEvent['kind'], payload: Record<string, unknown> = {}, seconds = 0): AgentEvent => ({
  agent_id: 'fixture-agent', cursor: id, source_id: id, ts: new Date(Date.UTC(2026, 0, 1, 0, 0, seconds)).toISOString(), kind, payload, truncated: false,
});

describe('G19 activity model', () => {
  it('keeps a failure when a partial history begins just before its result', () => {
    expect(transcriptRows([activity('f', 'status', { status: 'tool_failed' }),
      activity('r', 'tool_result', { tool_id: 'a', result: 'fixture' })])[0].failedCount).toBe(1);
  });

  it('clears resolved pending questions before showing subsequent activity', () => {
    const events = [activity('a', 'tool_start', { tool_id: 'a', name: 'Read' }),
      activity('q', 'status', { status: 'needs_input' })];
    const now = Date.parse(events[0].ts);
    expect(liveActivity(events, 'Running', now)).toContain('Waiting for your answer');
    for (const status of ['resolved', 'approval_resolved']) {
      expect(liveActivity([...events, activity('r', 'status', { status })], 'Running', now)).toBe('Reading a file');
    }
  });

  it('groups consecutive tools before windowing, keeps the first identity and counts Codex failure status', () => {
    const events = [activity('a', 'tool_start', { tool_id: 'a', name: 'exec' }),
      activity('ar', 'tool_result', { tool_id: 'a', result: 'fixture' }, 10),
      activity('b', 'tool_start', { tool_id: 'b' }, 12), activity('bf', 'status', { status: 'tool_failed', exit_code: 1 }, 20),
      activity('br', 'tool_result', { tool_id: 'b', result: 'fixture' }, 27)];
    const rows = transcriptRows(events);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ type: 'activity', durationMs: 27000, failedCount: 1 });
    expect(rows[0].tools).toHaveLength(2);
    expect(transcriptRows(events.slice(0, 2))[0].id).toBe(rows[0].id);
    const many = Array.from({ length: 100 }, (_, i) => activity(String(i), 'tool_start', { tool_id: String(i) }));
    expect(transcriptRows(many)).toHaveLength(1);
    expect(transcriptRows(many)[0].tools).toHaveLength(100);
  });

  it('keeps errors, decisions and attention outside groups, and aborted ends are not failures', () => {
    const aborted = activity('aborted', 'turn_end', { status: 'aborted', is_error: true });
    expect(transcriptRows([aborted])).toEqual([]);
    expect(protocolAttention([aborted])).toBeNull();
    const rows = transcriptRows([activity('a', 'tool_start', { tool_id: 'a' }),
      activity('e', 'error', { message: 'Fixture error' }), activity('b', 'tool_start', { tool_id: 'b' }),
      activity('d', 'status', { status: 'needs_approval', decision: 'decline' }),
      activity('c', 'tool_start', { tool_id: 'c' }), activity('q', 'status', { status: 'needs_input' }),
      activity('f', 'tool_start', { tool_id: 'f' }), activity('end', 'turn_end', { status: 'aborted' })]);
    expect(rows.map(row => row.type)).toEqual(['activity', 'error', 'activity', 'decision', 'activity', 'activity']);
    expect(rows.filter(row => row.type === 'activity').every(row => row.failedCount === 0)).toBe(true);
  });

  it('infers Claude turns, retires an old turn at user input and closes on end, abort or exit', () => {
    const events = [activity('a', 'assistant_message', { text: 'Fixture' })];
    const now = Date.parse(events[0].ts);
    expect(liveActivity(events, 'Running', now)).toBe('Thinking');
    expect(liveActivity(events, undefined, now)).toBeNull();
    expect(liveActivity(events, 'Completed', now)).toBeNull();
    expect(liveActivity(events, { Error: 'stopped' }, now)).toBeNull();
    for (const status of ['completed', 'aborted']) expect(liveActivity([...events, activity('end', 'turn_end', { status })], 'Running', now)).toBeNull();
    const user = [...events, activity('u', 'user_message', { text: 'Continue' }, 2)];
    expect(liveActivity(user, 'Running', now)).toBeNull();
    expect(liveActivity([...user, activity('b', 'tool_start', { tool_id: 'b', name: 'Read' }, 3)], 'Running', now + 3000)).toBe('Reading a file');
  });

  it('switches running tools to Thinking after their result, and directs quiet turns to the terminal', () => {
    const start = activity('a', 'tool_start', { tool_id: 'a', name: 'exec', input: 'await tools.exec_command({})' });
    const now = Date.parse(start.ts);
    expect(liveActivity([start], 'Running', now)).toBe('Running bash');
    expect(liveActivity([start, activity('r', 'tool_result', { tool_id: 'a' }, 2)], 'Running', now + 2000)).toBe('Thinking');
    expect(liveActivity([start], 'Running', now + 90000)).toBe('Waiting for 90s without an event · Check the terminal');
  });
});

describe('T10 activity labels', () => {
  it.each([
    ...['Bash', 'shell_command'].map(name => [name, undefined, 'Running bash']),
    ['Read', undefined, 'Reading a file'],
    ...['Edit', 'Write', 'apply_patch'].map(name => [name, undefined, 'Editing a file']),
    ...['Grep', 'Glob'].map(name => [name, undefined, 'Searching']),
    ...['Task', 'spawn_agent', 'followup_task', 'wait_agent', 'send_message', 'interrupt_agent', 'list_agents'].map(name => [name, undefined, 'Running a sub-agent']),
    ...['sleep', 'wait'].map(name => [name, undefined, 'Waiting']),
    ['request_user_input_async', undefined, 'Waiting for your answer'], ['unknown', undefined, 'Running unknown'],
    ['exec', 'await tools.exec_command({})', 'Running bash'], ['exec', 'tools.write_stdin({})', 'Running bash'],
    ['exec', 'tools.apply_patch("fixture")', 'Editing a file'], ['exec', 'tools.web__run({})', 'Searching the web'],
    ['exec', 'tools.view_image({})', 'Viewing an image'], ['exec', 'text(1)', 'Running code'],
    ['exec', {}, 'Running code'], ['exec', undefined, 'Running code'],
    ['exec', 'tools.exec_command({}); tools.web__run({})', 'Running bash'],
    ['exec', 'tools.unknown({})', 'Running unknown'], ['toString', undefined, 'Running toString'],
  ] as [string, unknown, string][])('%s with %s gives %s', (name, input, expected) => {
    expect(activityLabel(name as string, input)).toBe(expected);
  });
});
