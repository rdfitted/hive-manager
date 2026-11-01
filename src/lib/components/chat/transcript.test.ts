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
