<script lang="ts" module>
  export interface ChartPoint {
    label: string;
    value: number;
  }

  interface ChartSeries {
    name?: string;
    points: ChartPoint[];
  }

  interface ChartPayload {
    title?: string;
    type?: string;
    labels?: unknown[];
    values?: unknown[];
    data?: unknown;
    points?: unknown;
    rows?: unknown;
    series?: unknown;
  }

  interface NormalizedChart {
    title?: string;
    type: 'bar' | 'line';
    series: ChartSeries[];
    points: ChartPoint[];
  }

  function objectValue(value: unknown, key: string): unknown {
    if (!value || typeof value !== 'object') return undefined;
    return (value as Record<string, unknown>)[key];
  }

  function finiteNumber(value: unknown): number | null {
    if (typeof value === 'number' && Number.isFinite(value)) return value;
    if (typeof value === 'string' && value.trim() !== '') {
      const n = Number(value);
      if (Number.isFinite(n)) return n;
    }
    return null;
  }

  function pointLabel(value: unknown, index: number): string {
    if (typeof value === 'string' && value.trim()) return value;
    if (typeof value === 'number' && Number.isFinite(value)) return String(value);
    return String(index + 1);
  }

  function pointFromObject(item: Record<string, unknown>, index: number): ChartPoint | null {
    const value =
      finiteNumber(item.value) ??
      finiteNumber(item.y) ??
      finiteNumber(item.count) ??
      finiteNumber(item.total);
    if (value === null) return null;

    const label = item.label ?? item.name ?? item.x ?? item.key ?? item.category;
    return { label: pointLabel(label, index), value };
  }

  function pointsFromArray(values: unknown[], labels?: unknown[]): ChartPoint[] {
    return values
      .map((item, index): ChartPoint | null => {
        if (Array.isArray(item)) {
          const value = finiteNumber(item[1]);
          return value === null ? null : { label: pointLabel(item[0], index), value };
        }
        if (typeof item === 'number' || typeof item === 'string') {
          const value = finiteNumber(item);
          return value === null ? null : { label: pointLabel(labels?.[index], index), value };
        }
        if (item && typeof item === 'object') {
          return pointFromObject(item as Record<string, unknown>, index);
        }
        return null;
      })
      .filter((point): point is ChartPoint => point !== null);
  }

  function pointsFromPayload(payload: ChartPayload): ChartPoint[] {
    if (Array.isArray(payload.labels) && Array.isArray(payload.values)) {
      return pointsFromArray(payload.values, payload.labels);
    }

    const candidates = [payload.points, payload.data, payload.rows];
    for (const candidate of candidates) {
      if (Array.isArray(candidate)) {
        const points = pointsFromArray(candidate);
        if (points.length > 0) return points;
      }
    }

    return [];
  }

  function seriesFromPayload(payload: ChartPayload): ChartSeries[] {
    if (!Array.isArray(payload.series)) return [];

    return payload.series
      .map((entry, index): ChartSeries | null => {
        if (!entry || typeof entry !== 'object') return null;
        const name = objectValue(entry, 'name');
        const labels = objectValue(entry, 'labels');
        const data =
          objectValue(entry, 'data') ??
          objectValue(entry, 'values') ??
          objectValue(entry, 'points');

        if (!Array.isArray(data)) return null;
        const points = pointsFromArray(data, Array.isArray(labels) ? labels : payload.labels);
        if (points.length === 0) return null;
        return {
          name: typeof name === 'string' ? name : `Series ${index + 1}`,
          points,
        };
      })
      .filter((series): series is ChartSeries => series !== null);
  }

  export function normalizeChart(value: unknown): NormalizedChart {
    if (Array.isArray(value)) {
      const points = pointsFromArray(value);
      return { type: 'bar', series: [{ points }], points };
    }

    if (!value || typeof value !== 'object') {
      return { type: 'bar', series: [], points: [] };
    }

    const payload = value as ChartPayload;
    const series = seriesFromPayload(payload);
    const points = series.length > 0 ? series.flatMap((s) => s.points) : pointsFromPayload(payload);
    const chartSeries = series.length > 0 ? series : points.length > 0 ? [{ points }] : [];
    const type = payload.type === 'line' || payload.type === 'area' ? 'line' : 'bar';

    return {
      title: typeof payload.title === 'string' ? payload.title : undefined,
      type,
      series: chartSeries,
      points,
    };
  }
</script>

<script lang="ts">
  import type { ToolRendererProps } from './registry';

  let { data }: ToolRendererProps = $props();

  const VIEW_WIDTH = 640;
  const VIEW_HEIGHT = 240;
  const PADDING_LEFT = 42;
  const PADDING_RIGHT = 16;
  const PADDING_TOP = 18;
  const PADDING_BOTTOM = 34;

  const plotWidth = VIEW_WIDTH - PADDING_LEFT - PADDING_RIGHT;
  const plotHeight = VIEW_HEIGHT - PADDING_TOP - PADDING_BOTTOM;
  const palette = ['var(--accent-cyan)', 'var(--accent-amber)', 'var(--status-success)', 'var(--accent-chrome)'];

  const chart = $derived(normalizeChart(data));
  const values = $derived(chart.points.map((point) => point.value));
  const hasPoints = $derived(values.length > 0);
  const yMin = $derived(Math.min(0, ...values));
  const yMax = $derived(Math.max(1, ...values));
  const yMid = $derived((yMin + yMax) / 2);
  const yTicks = $derived([yMax, yMid, yMin]);
  const axisLabels = $derived(chart.points.slice(0, Math.min(chart.points.length, 8)));
  const baselineY = $derived(scaleY(0));

  function scaleY(value: number): number {
    const span = yMax - yMin || 1;
    return PADDING_TOP + (1 - (value - yMin) / span) * plotHeight;
  }

  function barWidth(): number {
    return Math.max(12, Math.min(46, plotWidth / Math.max(chart.points.length, 1) - 8));
  }

  function barX(index: number): number {
    const slot = plotWidth / Math.max(chart.points.length, 1);
    return PADDING_LEFT + index * slot + (slot - barWidth()) / 2;
  }

  function barY(value: number): number {
    return Math.min(scaleY(value), baselineY);
  }

  function barHeight(value: number): number {
    return Math.max(1, Math.abs(baselineY - scaleY(value)));
  }

  function lineX(index: number, count: number): number {
    if (count <= 1) return PADDING_LEFT + plotWidth / 2;
    return PADDING_LEFT + (index / (count - 1)) * plotWidth;
  }

  function linePoints(points: ChartPoint[]): string {
    return points.map((point, index) => `${lineX(index, points.length)},${scaleY(point.value)}`).join(' ');
  }

  function formatNumber(value: number): string {
    return Number.isInteger(value) ? String(value) : value.toFixed(1);
  }
</script>

<div class="chart-widget" data-testid="chart-widget">
  {#if chart.title}
    <div class="chart-title">{chart.title}</div>
  {/if}

  {#if hasPoints}
    <div class="chart-frame">
      <svg viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`} role="img" aria-label={chart.title ?? 'Chart'}>
        {#each yTicks as tick (tick)}
          <line
            class="grid-line"
            x1={PADDING_LEFT}
            x2={VIEW_WIDTH - PADDING_RIGHT}
            y1={scaleY(tick)}
            y2={scaleY(tick)}
          />
          <text class="tick-label" x={PADDING_LEFT - 8} y={scaleY(tick) + 4}>{formatNumber(tick)}</text>
        {/each}

        <line class="axis-line" x1={PADDING_LEFT} x2={VIEW_WIDTH - PADDING_RIGHT} y1={baselineY} y2={baselineY} />

        {#if chart.type === 'line'}
          {#each chart.series as series, seriesIndex (series.name ?? seriesIndex)}
            <polyline
              class="chart-line"
              points={linePoints(series.points)}
              stroke={palette[seriesIndex % palette.length]}
            />
            {#each series.points as point, pointIndex (`${seriesIndex}-${pointIndex}`)}
              <circle
                class="chart-point"
                cx={lineX(pointIndex, series.points.length)}
                cy={scaleY(point.value)}
                r="4"
                fill={palette[seriesIndex % palette.length]}
              >
                <title>{point.label}: {formatNumber(point.value)}</title>
              </circle>
            {/each}
          {/each}
        {:else}
          {#each chart.points as point, index (index)}
            <rect
              class="chart-bar"
              x={barX(index)}
              y={barY(point.value)}
              width={barWidth()}
              height={barHeight(point.value)}
              rx="2"
            >
              <title>{point.label}: {formatNumber(point.value)}</title>
            </rect>
          {/each}
        {/if}
      </svg>

      <div class="chart-axis-labels" aria-hidden="true">
        {#each axisLabels as point (point.label)}
          <span>{point.label}</span>
        {/each}
      </div>
    </div>
  {:else}
    <div class="chart-empty">No numeric chart data.</div>
  {/if}
</div>

<style>
  .chart-widget {
    background: color-mix(in srgb, var(--bg-void) 45%, var(--bg-surface));
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .chart-title {
    padding: 6px 10px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-primary);
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border-structural);
  }

  .chart-frame {
    padding: 8px 10px 10px;
  }

  svg {
    display: block;
    width: 100%;
    height: auto;
    max-height: 280px;
    overflow: visible;
  }

  .grid-line {
    stroke: color-mix(in srgb, var(--border-structural) 70%, transparent);
    stroke-width: 1;
  }

  .axis-line {
    stroke: var(--border-structural);
    stroke-width: 1.2;
  }

  .tick-label {
    fill: var(--text-secondary);
    font-family: var(--font-mono);
    font-size: 10px;
    text-anchor: end;
  }

  .chart-bar {
    fill: var(--accent-cyan);
    opacity: 0.78;
  }

  .chart-bar:hover,
  .chart-point:hover {
    opacity: 1;
  }

  .chart-line {
    fill: none;
    stroke-width: 2.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .chart-point {
    opacity: 0.9;
  }

  .chart-axis-labels {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(48px, 1fr));
    gap: 6px;
    margin-left: 42px;
    color: var(--text-secondary);
    font-family: var(--font-mono);
    font-size: 10px;
  }

  .chart-axis-labels span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chart-empty {
    padding: 12px;
    font-size: 11px;
    color: var(--text-secondary);
    font-style: italic;
  }
</style>
