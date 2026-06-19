/**
 * Mention (`@`) data providers for the Composer (#128).
 *
 * Three kinds of mention target: agents (from the active session), other sessions, and
 * files (a debounced Tauri fs read scoped to the active session's project/worktree path).
 * Every mention resolves to a stable `id` (agent.id / session.id / absolute file path as
 * received from Rust — NO `path.join`, per the Windows-path risk) plus a display `label`.
 *
 * `flattenMention()` is the canonical serialization used when the contenteditable model is
 * flattened to a plain string on submit: agents -> `@label`, sessions -> `#session`, files
 * -> the raw path. The backend `send_agent_input` only accepts a flat string, so there is
 * no persistable rich-document model.
 */

import { invoke } from '@tauri-apps/api/core';
import { serdeEnumVariantName } from '$lib/types/domain';
import type { AgentInfo, Session } from '$lib/stores/sessions';

export type MentionKind = 'agent' | 'session' | 'file';

export interface MentionItem {
  kind: MentionKind;
  /** Stable identifier (agent.id / session.id / absolute file path). */
  id: string;
  /** Display label (without the leading @). */
  label: string;
  /** Secondary line shown in the menu (role, project, etc.). */
  detail?: string;
}

/** Friendly role label for an agent, handling serde object-variants safely. */
function roleLabel(role: AgentInfo['role']): string {
  const variant = serdeEnumVariantName(role);
  if (variant === 'Worker' && typeof role === 'object' && role !== null && 'Worker' in role) {
    const idx = (role as { Worker: { index: number } }).Worker.index;
    return `Worker ${idx}`;
  }
  return variant ?? 'Agent';
}

/** Build agent mention items from the active session's agents. */
export function agentMentions(agents: AgentInfo[]): MentionItem[] {
  return agents.map((a) => ({
    kind: 'agent' as const,
    id: a.id,
    label: a.config?.label || roleLabel(a.role) || a.id.slice(0, 8),
    detail: roleLabel(a.role),
  }));
}

/** Build session mention items from the sessions list. */
export function sessionMentions(sessions: Session[]): MentionItem[] {
  return sessions.map((s) => ({
    kind: 'session' as const,
    id: s.id,
    label: s.name || s.id.slice(0, 8),
    detail: s.project_path,
  }));
}

/** Build file mention items from a list of absolute paths returned by Rust. */
export function fileMentions(paths: string[]): MentionItem[] {
  return paths.map((p) => {
    // Display just the basename; keep the full path as the stable id.
    const idx = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
    const base = idx >= 0 ? p.slice(idx + 1) : p;
    return { kind: 'file' as const, id: p, label: base, detail: p };
  });
}

/**
 * Debounced file source: lists files under a session's project/worktree path via a Tauri
 * command. Returns absolute paths exactly as Rust provides them (no client-side joining).
 * Results are capped and the call is debounced so large worktrees don't thrash.
 */
let fileDebounce: ReturnType<typeof setTimeout> | null = null;
const FILE_DEBOUNCE_MS = 200;
const FILE_RESULT_CAP = 50;

export function listSessionFiles(
  rootPath: string,
  query: string
): Promise<string[]> {
  return new Promise((resolve) => {
    if (fileDebounce) clearTimeout(fileDebounce);
    fileDebounce = setTimeout(async () => {
      fileDebounce = null;
      try {
        const files = await invoke<string[]>('list_session_files', {
          rootPath,
          query,
        });
        resolve(files.slice(0, FILE_RESULT_CAP));
      } catch {
        // No backing command (or fs error) — file mentions simply yield nothing.
        resolve([]);
      }
    }, FILE_DEBOUNCE_MS);
  });
}

/** Case-insensitive prefix/substring filter over mention labels. */
export function filterMentions(items: MentionItem[], query: string): MentionItem[] {
  const q = query.trim().toLowerCase();
  if (q === '') return items;
  return items.filter(
    (m) =>
      m.label.toLowerCase().includes(q) ||
      (m.detail ? m.detail.toLowerCase().includes(q) : false)
  );
}

/**
 * Flatten a mention to the plain-text token inserted into the outgoing prompt.
 * agents -> `@label`, sessions -> `#session`, files -> raw path.
 */
export function flattenMention(item: MentionItem): string {
  switch (item.kind) {
    case 'agent':
      return `@${item.label}`;
    case 'session':
      return `#${item.label}`;
    case 'file':
      return item.id;
    default:
      return item.label;
  }
}
