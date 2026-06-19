<script lang="ts" module>
  /**
   * Unified-diff line model. Exported from the module context so the parser can
   * be unit-tested and reused.
   */
  export type DiffLineKind = 'add' | 'del' | 'ctx' | 'meta';

  export interface DiffLine {
    kind: DiffLineKind;
    text: string;
  }

  export interface DiffHunk {
    lines: DiffLine[];
  }

  export interface DiffFile {
    path: string;
    hunks?: DiffHunk[];
  }

  export interface DiffData {
    unified?: string;
    files?: DiffFile[];
  }

  /**
   * Parse a unified-diff string into add/del/ctx/meta lines.
   *  - '+++' / '---' / 'diff '/'index ' headers -> meta
   *  - '@@' hunk markers -> meta (also delimit context)
   *  - leading '+' -> add, leading '-' -> del, else -> ctx
   * Returns [] when the input yields nothing meaningful so the caller can fall
   * back to a raw <pre>.
   */
  export function parseUnifiedDiff(unified: string): DiffLine[] {
    if (!unified) return [];
    const out: DiffLine[] = [];
    for (const raw of unified.split('\n')) {
      if (
        raw.startsWith('+++') ||
        raw.startsWith('---') ||
        raw.startsWith('diff ') ||
        raw.startsWith('index ') ||
        raw.startsWith('@@')
      ) {
        out.push({ kind: 'meta', text: raw });
      } else if (raw.startsWith('+')) {
        out.push({ kind: 'add', text: raw.slice(1) });
      } else if (raw.startsWith('-')) {
        out.push({ kind: 'del', text: raw.slice(1) });
      } else {
        out.push({ kind: 'ctx', text: raw.startsWith(' ') ? raw.slice(1) : raw });
      }
    }
    return out;
  }

  const COUNTABLE: DiffLineKind[] = ['add', 'del', 'ctx'];

  /** True if the parsed result contains at least one real (non-meta) line. */
  function hasContent(lines: DiffLine[]): boolean {
    return lines.some((l) => COUNTABLE.includes(l.kind));
  }
</script>

<script lang="ts">
  import type { ToolRendererProps } from './registry';

  let { data }: ToolRendererProps = $props();

  function asDiffData(value: unknown): DiffData {
    if (value && typeof value === 'object') return value as DiffData;
    if (typeof value === 'string') return { unified: value };
    return {};
  }

  const diff = $derived(asDiffData(data));

  // Prefer structured `files` when present; otherwise parse `unified`.
  const fromUnified = $derived.by<DiffLine[]>(() =>
    diff.unified ? parseUnifiedDiff(diff.unified) : [],
  );

  const structuredLines = $derived.by<DiffLine[]>(() => {
    if (!diff.files || diff.files.length === 0) return [];
    const lines: DiffLine[] = [];
    for (const file of diff.files) {
      lines.push({ kind: 'meta', text: file.path });
      for (const hunk of file.hunks ?? []) {
        for (const ln of hunk.lines ?? []) {
          lines.push(ln);
        }
      }
    }
    return lines;
  });

  const lines = $derived(structuredLines.length > 0 ? structuredLines : fromUnified);
  const renderable = $derived(hasContent(lines));

  const rawText = $derived(
    diff.unified ?? (typeof data === 'string' ? data : JSON.stringify(data, null, 2)),
  );

  function prefix(kind: DiffLineKind): string {
    if (kind === 'add') return '+';
    if (kind === 'del') return '-';
    if (kind === 'meta') return '';
    return ' ';
  }
</script>

<div class="diff-widget">
  {#if renderable}
    <div class="diff-body">
      {#each lines as line, i (i)}
        <div class="diff-line {line.kind}">
          <span class="gutter">{prefix(line.kind)}</span>
          <span class="line-text">{line.text}</span>
        </div>
      {/each}
    </div>
  {:else}
    <pre class="diff-raw">{rawText}</pre>
  {/if}
</div>

<style>
  .diff-widget {
    background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .diff-body {
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.5;
    overflow-x: auto;
  }

  .diff-line {
    display: flex;
    gap: 6px;
    padding: 0 8px;
    white-space: pre;
  }

  .gutter {
    flex-shrink: 0;
    width: 10px;
    text-align: center;
    color: var(--text-secondary);
    user-select: none;
  }

  .line-text {
    flex: 1;
    word-break: break-word;
  }

  .diff-line.add {
    background: color-mix(in srgb, var(--status-success) 10%, transparent);
  }

  .diff-line.add .line-text,
  .diff-line.add .gutter {
    color: var(--status-success);
  }

  .diff-line.del {
    background: color-mix(in srgb, var(--status-error) 10%, transparent);
  }

  .diff-line.del .line-text,
  .diff-line.del .gutter {
    color: var(--status-error);
  }

  .diff-line.ctx .line-text {
    color: var(--text-secondary);
  }

  .diff-line.meta {
    background: var(--bg-surface);
  }

  .diff-line.meta .line-text {
    color: var(--accent-chrome);
    font-weight: 600;
  }

  .diff-raw {
    margin: 0;
    padding: 8px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-secondary);
    overflow-x: auto;
  }
</style>
