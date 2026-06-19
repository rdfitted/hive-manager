<script lang="ts">
  /**
   * Chart tool-render widget — STUB.
   *
   * No charting library is a dependency yet, so this reserves the 'chart' id and
   * renders the JSON fallback plus a "not yet implemented" note. Deferring real
   * charting keeps #127 to a single session.
   */
  import type { ToolRendererProps } from './registry';

  let { data }: ToolRendererProps = $props();

  const json = $derived(JSON.stringify(data, null, 2));
</script>

<div class="chart-widget">
  <div class="chart-note">Chart renderer not yet implemented — showing raw data.</div>
  <pre class="chart-json">{json}</pre>
</div>

<style>
  .chart-widget {
    background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .chart-note {
    padding: 4px 8px;
    font-size: 10px;
    color: var(--text-secondary);
    background: color-mix(in srgb, var(--accent-amber) 8%, transparent);
    border-bottom: 1px solid var(--border-structural);
  }

  .chart-json {
    margin: 0;
    padding: 8px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-secondary);
    overflow-x: auto;
  }
</style>
