<script lang="ts">
  /**
   * DataTable tool-render widget.
   *
   * data shape: { columns?: string[]; rows: Record<string, unknown>[] }
   * Columns auto-infer from the union of the first row's keys when absent.
   * Click a header to sort (cycles asc -> desc -> none). v1: no virtualization;
   * a notice is shown above the table past ROW_CAP rows.
   */
  import type { ToolRendererProps } from './registry';

  let { data }: ToolRendererProps = $props();

  const ROW_CAP = 500;

  interface TableData {
    columns?: string[];
    rows: Record<string, unknown>[];
  }

  function asTableData(value: unknown): TableData {
    if (value && typeof value === 'object' && Array.isArray((value as TableData).rows)) {
      return value as TableData;
    }
    return { rows: [] };
  }

  const table = $derived(asTableData(data));

  const columns = $derived.by<string[]>(() => {
    if (table.columns && table.columns.length > 0) {
      return table.columns;
    }
    const first = table.rows[0];
    return first ? Object.keys(first) : [];
  });

  let sortKey = $state<string | null>(null);
  let sortDir = $state<'asc' | 'desc'>('asc');

  function cycleSort(key: string) {
    if (sortKey !== key) {
      sortKey = key;
      sortDir = 'asc';
    } else if (sortDir === 'asc') {
      sortDir = 'desc';
    } else {
      sortKey = null;
      sortDir = 'asc';
    }
  }

  function compare(a: unknown, b: unknown): number {
    if (a === b) return 0;
    if (a === null || a === undefined) return -1;
    if (b === null || b === undefined) return 1;
    if (typeof a === 'number' && typeof b === 'number') return a - b;
    return String(a).localeCompare(String(b));
  }

  const sortedRows = $derived.by<Record<string, unknown>[]>(() => {
    const rows = table.rows.slice(0, ROW_CAP);
    if (!sortKey) return rows;
    const key = sortKey;
    const dir = sortDir === 'asc' ? 1 : -1;
    return rows.slice().sort((ra, rb) => compare(ra[key], rb[key]) * dir);
  });

  function cellText(value: unknown): string {
    if (value === null || value === undefined) return '';
    if (typeof value === 'object') return JSON.stringify(value);
    return String(value);
  }
</script>

<div class="data-table-widget">
  {#if table.rows.length > ROW_CAP}
    <div class="notice">Showing first {ROW_CAP} of {table.rows.length} rows.</div>
  {/if}
  {#if columns.length === 0}
    <div class="empty">No tabular data.</div>
  {:else}
    <div class="table-scroll">
      <table>
        <thead>
          <tr>
            {#each columns as col (col)}
              <th>
                <button class="th-btn" onclick={() => cycleSort(col)}>
                  <span>{col}</span>
                  {#if sortKey === col}
                    <span class="sort-arrow">{sortDir === 'asc' ? '▲' : '▼'}</span>
                  {/if}
                </button>
              </th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each sortedRows as row, i (i)}
            <tr>
              {#each columns as col (col)}
                <td>{cellText(row[col])}</td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .data-table-widget {
    background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .notice {
    padding: 4px 8px;
    font-size: 10px;
    color: var(--text-secondary);
    background: color-mix(in srgb, var(--accent-amber) 8%, transparent);
    border-bottom: 1px solid var(--border-structural);
  }

  .empty {
    padding: 12px;
    font-size: 11px;
    color: var(--text-secondary);
    font-style: italic;
  }

  .table-scroll {
    overflow-x: auto;
    max-height: 360px;
    overflow-y: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-family: var(--font-mono);
    font-size: 11px;
  }

  th {
    position: sticky;
    top: 0;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border-structural);
    text-align: left;
    padding: 0;
    z-index: 1;
  }

  .th-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    padding: 6px 8px;
    background: none;
    border: none;
    color: var(--accent-cyan);
    font-family: inherit;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    text-align: left;
  }

  .th-btn:hover {
    color: var(--text-primary);
  }

  .sort-arrow {
    font-size: 9px;
    color: var(--accent-cyan);
  }

  td {
    padding: 4px 8px;
    border-bottom: 1px solid color-mix(in srgb, var(--border-structural) 50%, transparent);
    color: var(--text-primary);
    word-break: break-word;
  }

  tbody tr:last-child td {
    border-bottom: none;
  }

  tbody tr:hover td {
    background: color-mix(in srgb, var(--accent-cyan) 5%, transparent);
  }
</style>
