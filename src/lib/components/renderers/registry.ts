/**
 * Pure-TypeScript tool-render registry.
 *
 * Maps a small result envelope (`{ renderer?, data }`) to a Svelte widget. The
 * resolution logic lives here as plain data + functions so it is unit-testable
 * in vitest's default `node` environment with NO DOM and NO Svelte runtime
 * import in the hot path beyond the `Component` *type* (erased at compile time).
 *
 * IMPORTANT (issue #127, criterion 4): this module imports NOTHING from the chat
 * core (no ConversationViewer, no conversations store). Registering a new
 * renderer is a single `registerToolRenderer()` call and requires zero edits to
 * chat-core files.
 *
 * Resolution priority (highest first):
 *   (a) exact built-in id === input.renderer
 *   (b) input.renderer hint matched against any registered renderer's id
 *   (c) custom renderers whose `match` is a string equal to toolName/renderer
 *       OR a predicate returning true, ordered by descending `priority`
 *   (d) null  -> the host renders the JSON/text fallback
 */

import type { Component } from 'svelte';

/** Detail payload for the Approval widget's approve/reject callbacks. */
export interface ApprovalActionDetail {
  actionId?: string;
}

/**
 * Props every renderer widget receives.
 *
 * `data` is the structured payload. `onapprove` / `onreject` are OPTIONAL
 * callback props (Svelte 5 runes idiom — typeable, unlike createEventDispatcher
 * events) that only the Approval widget invokes; other widgets ignore them. The
 * host wires these to forward up to the chat core.
 */
export interface ToolRendererProps {
  data: unknown;
  onapprove?: (detail: ApprovalActionDetail) => void;
  onreject?: (detail: ApprovalActionDetail) => void;
}

/** A dynamically-mountable tool-render widget. */
export type ToolRendererComponent = Component<ToolRendererProps>;

/** Input to {@link resolveToolRenderer}. */
export interface ResolveInput {
  /** Explicit renderer hint from the result envelope (a literal string). */
  renderer?: string;
  /** Originating tool / sender name (e.g. ConversationMessage.from). */
  toolName?: string;
  /** The data payload (available to predicate matchers). */
  data?: unknown;
}

/** A registered renderer. */
export interface ToolRenderer {
  /** Stable id (e.g. 'table', 'diff', 'approval', 'chart'). */
  id: string;
  /**
   * Optional custom match. A string is compared (exact) against the resolve
   * input's `toolName` then `renderer`. A predicate is called with the full
   * resolve input and returns true to claim the message.
   */
  match?: string | ((input: ResolveInput) => boolean);
  /** The Svelte component to mount. */
  component: ToolRendererComponent;
  /** Higher wins when multiple custom renderers match. Defaults to 0. */
  priority?: number;
}

/**
 * Built-in renderer ids. A hint exactly equal to one of these resolves directly
 * to the matching built-in (priority a/b), bypassing custom predicate scanning.
 */
export const BUILTIN_RENDERER_IDS = ['table', 'diff', 'approval', 'chart'] as const;
export type BuiltinRendererId = (typeof BUILTIN_RENDERER_IDS)[number];

// Module-level registry. `byId` is the fast lookup; `ordered` preserves
// registration order for stable, deterministic priority tie-breaking.
const byId = new Map<string, ToolRenderer>();
const ordered: ToolRenderer[] = [];
const builtinIds = new Set<string>(BUILTIN_RENDERER_IDS);

/**
 * Register (or replace) a renderer. Replacing an existing id updates in place,
 * preserving the original registration order so priority ties stay stable.
 */
export function registerToolRenderer(renderer: ToolRenderer): void {
  if (!renderer || typeof renderer.id !== 'string' || renderer.id.length === 0) {
    throw new Error('registerToolRenderer: renderer.id must be a non-empty string');
  }

  const existing = byId.get(renderer.id);
  byId.set(renderer.id, renderer);

  if (existing) {
    const idx = ordered.indexOf(existing);
    if (idx >= 0) {
      ordered[idx] = renderer;
      return;
    }
  }
  ordered.push(renderer);
}

/**
 * Resolve a renderer for the given input, or `null` to trigger the host's
 * formatted-JSON/text fallback. See module doc for the priority order.
 */
export function resolveToolRenderer(input: ResolveInput): ToolRenderer | null {
  const hint = input.renderer;

  // (a) + (b): an explicit hint matching a registered id wins outright. Because
  // built-ins register under their canonical ids, this covers both "built-in id
  // === hint" and "hint matched to a registered id".
  if (hint) {
    const direct = byId.get(hint);
    if (direct) {
      return direct;
    }
  }

  // (c): custom (non-built-in) renderers, scanned by descending priority. A
  // string `match` compares against toolName then the renderer hint; a
  // predicate gets the full input.
  const candidates = ordered
    .filter((r) => !builtinIds.has(r.id) && r.match !== undefined)
    .filter((r) => matches(r, input))
    .sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0));

  if (candidates.length > 0) {
    return candidates[0];
  }

  // (d): no match -> host renders the fallback.
  return null;
}

function matches(renderer: ToolRenderer, input: ResolveInput): boolean {
  const m = renderer.match;
  if (m === undefined) {
    return false;
  }
  if (typeof m === 'function') {
    try {
      return m(input) === true;
    } catch {
      return false;
    }
  }
  // String match: exact against toolName, then the renderer hint.
  return m === input.toolName || m === input.renderer;
}

/** True if `id` is currently registered. */
export function hasToolRenderer(id: string): boolean {
  return byId.has(id);
}

/** Number of currently-registered renderers (test/diagnostic helper). */
export function rendererCount(): number {
  return ordered.length;
}

/**
 * Test helper: remove all CUSTOM (non-built-in) renderers, leaving built-ins
 * intact. Keeps tests isolated without forcing a re-import of builtins.ts.
 */
export function clearCustomRenderers(): void {
  for (let i = ordered.length - 1; i >= 0; i -= 1) {
    const r = ordered[i];
    if (!builtinIds.has(r.id)) {
      ordered.splice(i, 1);
      byId.delete(r.id);
    }
  }
}

/**
 * Test helper: clear the ENTIRE registry including built-ins. Used to assert
 * idempotent built-in registration from a clean slate.
 */
export function clearAllRenderers(): void {
  ordered.length = 0;
  byId.clear();
}
