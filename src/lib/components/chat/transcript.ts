import type { AgentEvent } from '$lib/stores/agentEvents';

export type TranscriptRow = {
  id: string; type: 'user' | 'assistant' | 'tool' | 'error' | 'decision'; ts: string; text: string;
  name?: string; input?: unknown; result?: unknown; complete?: boolean; failed?: boolean; truncated: boolean;
};

export const CHAT_WINDOW_SIZE = 80;

function failedTurn(event: AgentEvent): boolean {
  return event.kind === 'turn_end' && (event.payload.is_error === true || event.payload.status === 'failed'
    || event.payload.error != null);
}

function failureText(payload: Record<string, unknown>, fallback = 'Agent turn failed.'): string {
  const error = payload.error;
  const detail = error && typeof error === 'object' ? (error as Record<string, unknown>).message : error;
  return [payload.text, payload.message, detail, payload.result].find((value): value is string => typeof value === 'string' && value.length > 0)
    ?? fallback;
}

/** Current D/E adapters decline interactive requests and record that decision in the notice. */
function declinedDecision(event: AgentEvent): string | null {
  const p = event.payload;
  return event.kind === 'status' && p.status === 'needs_approval'
    && (p.decision === 'decline' || (typeof p.message === 'string' && /\bdeclined\b/i.test(p.message)))
    ? (typeof p.message === 'string' ? p.message : 'Approval was declined.') : null;
}

/** Normalize snapshots and deltas without exposing protocol approval widgets. */
export function transcriptRows(events: AgentEvent[]): TranscriptRow[] {
  const rows: TranscriptRow[] = [];
  const messages = new Map<string, TranscriptRow>();
  const tools = new Map<string, TranscriptRow>();
  let turn = 0;
  for (const event of events) {
    const p = event.payload;
    const failure = failedTurn(event);
    const decision = declinedDecision(event);
    if (event.kind === 'turn_start' || event.kind === 'turn_end') { turn++; if (!failure) continue; }
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
    if (event.kind === 'status' && !decision) continue;
    const type = event.kind === 'user_message' ? 'user' : event.kind === 'error' || failure ? 'error' : decision ? 'decision' : 'assistant';
    const key = `${event.agent_id}:${type}:${event.source_id ?? (event.kind === 'assistant_delta' ? `stream-${turn}` : event.cursor)}`;
    const text = decision ?? (type === 'error' ? failureText(p, event.kind === 'error' ? 'Agent reported an error.' : undefined)
      : typeof p.text === 'string' ? p.text : typeof p.message === 'string' ? p.message : '');
    let row = messages.get(key);
    // A boundary must not replace the richer explicit error from the same turn.
    if (failure && row) continue;
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
  let failure: string | null = null;
  let decision: string | null = null;
  let pending: string | null = null;
  for (const event of events) {
    if (event.kind === 'user_message') { failure = decision = pending = null; continue; }
    if (event.kind === 'error') { failure = failureText(event.payload, 'Agent reported an error.'); continue; }
    if (failedTurn(event)) { failure ??= failureText(event.payload); continue; }
    if (event.kind === 'turn_start' || event.kind === 'turn_end') { pending = null; continue; }
    if (event.kind !== 'status') continue;
    const status = event.payload.status;
    if (status === 'resolved') { failure = decision = pending = null; continue; }
    if (status === 'error_resolved') { failure = null; continue; }
    if (status === 'approval_resolved') { decision = pending = null; continue; }
    const declined = declinedDecision(event);
    if (declined) { decision = declined; continue; }
    if (status === 'needs_approval' || status === 'needs_input') {
      const label = status === 'needs_approval' ? 'Approval required.' : 'Agent needs input.';
      pending = typeof event.payload.message === 'string' ? `${label} ${event.payload.message}` : label;
    }
  }
  return [failure, decision, pending].filter(Boolean).join(' ') || null;
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
