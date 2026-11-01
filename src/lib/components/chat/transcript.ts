import type { AgentEvent } from '$lib/stores/agentEvents';

export type TranscriptRow = {
  id: string; type: 'user' | 'assistant' | 'tool' | 'error'; ts: string; text: string;
  name?: string; input?: unknown; result?: unknown; complete?: boolean; failed?: boolean; truncated: boolean;
};

export const CHAT_WINDOW_SIZE = 80;

/** Normalize snapshots and deltas without exposing protocol approval widgets. */
export function transcriptRows(events: AgentEvent[]): TranscriptRow[] {
  const rows: TranscriptRow[] = [];
  const messages = new Map<string, TranscriptRow>();
  const tools = new Map<string, TranscriptRow>();
  let turn = 0;
  for (const event of events) {
    const p = event.payload;
    if (event.kind === 'turn_start' || event.kind === 'turn_end') { turn++; continue; }
    if (event.kind === 'tool_start' || event.kind === 'tool_result') {
      const key = `${event.agent_id}:tool:${String(p.tool_id ?? event.source_id ?? event.cursor)}`;
      let row = tools.get(key);
      if (!row) {
        row = { id: key, type: 'tool', ts: event.ts, text: '', name: 'Tool', truncated: false };
        rows.push(row); tools.set(key, row);
      }
      if (typeof p.name === 'string') row.name = p.name;
      if (event.kind === 'tool_start') row.input = p.input;
      else { row.result = p.result; row.complete = true; row.failed = p.is_error === true; }
      row.truncated ||= event.truncated;
      continue;
    }
    if (event.kind === 'status') continue;
    const type = event.kind === 'user_message' ? 'user' : event.kind === 'error' ? 'error' : 'assistant';
    const key = `${event.agent_id}:${type}:${event.source_id ?? (event.kind === 'assistant_delta' ? `stream-${turn}` : event.cursor)}`;
    const text = typeof p.text === 'string' ? p.text : typeof p.message === 'string' ? p.message : '';
    let row = messages.get(key);
    if (!row) {
      row = { id: key, type, ts: event.ts, text: '', truncated: false };
      rows.push(row); messages.set(key, row);
    }
    row.text = event.kind === 'assistant_delta' ? row.text + text : text;
    row.truncated ||= event.truncated;
  }
  return rows;
}

export function protocolAttention(events: AgentEvent[]): string | null {
  for (let index = events.length - 1; index >= 0; index--) {
    const event = events[index];
    if (event.kind === 'turn_start' || event.kind === 'turn_end') return null;
    if (event.kind === 'error') return typeof event.payload.message === 'string' ? event.payload.message : 'Agent reported an error.';
    if (event.kind !== 'status') continue;
    const status = event.payload.status;
    if (status === 'needs_approval' || status === 'needs_input') {
      const label = status === 'needs_approval' ? 'Approval required.' : 'Agent needs input.';
      return typeof event.payload.message === 'string' ? `${label} ${event.payload.message}` : label;
    }
    return null;
  }
  return null;
}

/** Only explicit passive built-ins can reach the shared renderer registry. */
export function passiveToolResult(result: unknown): { renderer: string; data: unknown } | null {
  if (!result || typeof result !== 'object' || Array.isArray(result)) return null;
  const envelope = result as Record<string, unknown>;
  return typeof envelope.renderer === 'string' && ['table', 'diff', 'chart'].includes(envelope.renderer)
    ? { renderer: envelope.renderer, data: envelope.data } : null;
}

export function formatRaw(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value ?? null, null, 2);
}
