import type { AgentEvent } from '$lib/stores/agentEvents';
import type { AgentStatus } from '$lib/stores/sessions';
import type { CoordinationMessage } from '$lib/stores/coordination';
import { activityLabel } from './activityLabel';

export type TranscriptRow = {
  id: string; type: 'user' | 'assistant' | 'tool' | 'activity' | 'error' | 'decision'; ts: string; text: string;
  name?: string; input?: unknown; result?: unknown; complete?: boolean; failed?: boolean; truncated: boolean;
  tools?: TranscriptRow[]; durationMs?: number; failedCount?: number; sender?: 'queen' | 'operator';
};

export type TranscriptContext = { agentId: string; isQueen: boolean; log: CoordinationMessage[] };

/** Backend format_agent_display aliases, matching opswall/ticker.ts coordinationName. */
function coordinationTarget({ agentId, isQueen }: TranscriptContext): string {
  if (isQueen) return 'QUEEN';
  if (agentId.endsWith('-evaluator')) return 'EVALUATOR';
  for (const [marker, prefix] of [['-qa-worker-', 'QA-WORKER-'], ['-worker-', 'WORKER-'], ['-planner-', 'PLANNER-']]) {
    const index = agentId.lastIndexOf(marker);
    if (index >= 0) {
      const suffix = agentId.slice(index + marker.length);
      return prefix + (marker === '-planner-' ? suffix.split('-')[0] : suffix);
    }
  }
  return agentId;
}

export const CHAT_WINDOW_SIZE = 80;

function failedTurn(event: AgentEvent): boolean {
  return event.kind === 'turn_end' && event.payload.status !== 'aborted' && (event.payload.is_error === true || event.payload.status === 'failed'
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
export function transcriptRows(events: AgentEvent[], context?: TranscriptContext): TranscriptRow[] {
  const operatorTexts = new Set<string>(), queenTexts = new Set<string>();
  if (context) {
    const target = coordinationTarget(context);
    for (const entry of context.log) {
      if (entry.to !== context.agentId && entry.to !== target) continue;
      // Persisted coordination lines reload as Task, while live entries are System.
      if (entry.from === 'SYSTEM' && entry.content.startsWith('[OPERATOR] ')) operatorTexts.add(entry.content.slice(11).trim());
      if (entry.message_type === 'Task' && entry.from === 'QUEEN') queenTexts.add(entry.content.trim());
    }
  }
  const rows: TranscriptRow[] = [];
  const messages = new Map<string, TranscriptRow>();
  const tools = new Map<string, TranscriptRow>();
  let turn = 0;
  let segment = 0;
  let latestTool: TranscriptRow | undefined;
  let pendingFailures = 0;
  const segments = new Map<string, number>();
  const failureSignals = new Map<string, number>();
  const endedAt = new Map<string, string>();
  for (const event of events) {
    const p = event.payload;
    const failure = failedTurn(event);
    const decision = declinedDecision(event);
    if (event.kind === 'turn_start' || event.kind === 'turn_end') { turn++; segment++; latestTool = undefined; pendingFailures = 0; if (!failure) continue; }
    if (event.kind === 'status' && p.status === 'tool_failed') {
      if (latestTool) {
        failureSignals.set(latestTool.id, (failureSignals.get(latestTool.id) ?? 0) + 1);
        latestTool.failed = true; endedAt.set(latestTool.id, event.ts);
      }
      else pendingFailures++;
      continue;
    }
    if (event.kind === 'tool_start' || event.kind === 'tool_result') {
      const key = `${event.agent_id}:tool:${String(p.tool_id ?? event.source_id ?? event.cursor)}`;
      let row = tools.get(key);
      if (!row) {
        row = { id: key, type: 'tool', ts: event.ts, text: '', name: 'Tool', truncated: false };
        rows.push(row); tools.set(key, row);
        segments.set(key, segment);
      }
      latestTool = row;
      if (pendingFailures) { failureSignals.set(key, pendingFailures); row.failed = true; pendingFailures = 0; }
      endedAt.set(key, event.ts);
      if (typeof p.name === 'string') row.name = p.name;
      if (event.kind === 'tool_start') row.input = p.input;
      else { row.result = p.result; row.complete = true; row.failed ||= p.is_error === true; }
      row.truncated ||= event.truncated;
      continue;
    }
    if (event.kind === 'status' && !decision) {
      if (p.status === 'needs_input' || p.status === 'needs_approval') { segment++; latestTool = undefined; pendingFailures = 0; }
      continue;
    }
    segment++; latestTool = undefined; pendingFailures = 0;
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
    if (type === 'user' && context && event.agent_id === context.agentId) {
      const match = row.text.trim();
      row.sender = operatorTexts.has(match) ? 'operator' : queenTexts.has(match) ? 'queen' : undefined;
    }
    row.truncated ||= event.truncated;
  }
  // Collapse the full history before the UI applies its bounded row window.
  const grouped: TranscriptRow[] = [];
  let groupSegment: number | undefined;
  for (const row of rows) {
    if (row.type !== 'tool') { grouped.push(row); groupSegment = undefined; continue; }
    const previous = grouped.at(-1);
    const sameGroup = previous?.type === 'activity' && groupSegment === segments.get(row.id);
    const group: TranscriptRow = sameGroup ? previous : {
      id: row.id, type: 'activity', ts: row.ts, text: '', truncated: false, tools: [], durationMs: 0, failedCount: 0,
    };
    if (!sameGroup) grouped.push(group);
    groupSegment = segments.get(row.id);
    group.tools!.push(row);
    group.truncated ||= row.truncated;
    group.failedCount! += Math.max(row.failed ? 1 : 0, failureSignals.get(row.id) ?? 0);
    group.durationMs = Math.max(group.durationMs!, Date.parse(endedAt.get(row.id) ?? row.ts) - Date.parse(group.ts), 0);
  }
  return grouped;
}

export type LiveActivity = { label: string; status: string };

/** Explicit Codex turns survive steering; user items retire inferred Claude work. */
export function liveActivityState(events: AgentEvent[], status: AgentStatus | undefined, now = Date.now()): LiveActivity | null {
  if (status !== 'Running') return null;
  let open = false;
  let explicit = false;
  let last: AgentEvent | undefined;
  const running = new Map<string, AgentEvent>();
  let pending: string | null = null;
  for (const event of events) {
    const p = event.payload;
    if (event.kind === 'turn_end' || (event.kind === 'user_message' && !explicit)) {
      open = false; explicit = false; last = undefined; running.clear(); pending = null; continue;
    }
    if (event.kind === 'user_message') pending = null;
    if (event.kind === 'turn_start') { explicit = true; running.clear(); pending = null; }
    if (event.kind === 'turn_start' || event.kind === 'assistant_message' || event.kind === 'assistant_delta'
      || event.kind === 'tool_start' || event.kind === 'tool_result') { open = true; last = event; }
    if (event.kind === 'tool_start') running.set(String(p.tool_id ?? event.source_id ?? event.cursor), event);
    if (event.kind === 'tool_result') running.delete(String(p.tool_id ?? event.source_id ?? event.cursor));
    if (event.kind === 'status' && (p.status === 'needs_input' || p.status === 'needs_approval')) {
      pending = p.status === 'needs_input' ? 'Waiting for your answer · Check the terminal' : 'Approval required · Check the terminal'; last = event;
    }
    if (event.kind === 'status' && (p.status === 'resolved' || p.status === 'approval_resolved')) pending = null;
    if (open) last = event;
  }
  if (!open || !last) return null;
  const quietSeconds = Math.floor(Math.max(0, now - Date.parse(last.ts)) / 1000);
  if (quietSeconds >= 90) return {
    label: `Waiting for ${quietSeconds}s without an event · Check the terminal`,
    status: 'Waiting without an event · Check the terminal',
  };
  if (pending) return { label: pending, status: pending };
  const tool = [...running.values()].at(-1);
  const label = tool ? activityLabel(typeof tool.payload.name === 'string' ? tool.payload.name : 'Tool', tool.payload.input) : 'Thinking';
  return { label, status: label };
}

export function liveActivity(events: AgentEvent[], status: AgentStatus | undefined, now = Date.now()): string | null {
  return liveActivityState(events, status, now)?.label ?? null;
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
