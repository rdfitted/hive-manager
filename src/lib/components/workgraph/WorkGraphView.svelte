<script lang="ts">
  import { onDestroy } from 'svelte';
  import { activeSession } from '$lib/stores/sessions';
  import { apiUrl } from '$lib/config';
  import SkelBar from '../SkelBar.svelte';

  type NodeStatus =
    | 'pending'
    | 'ready'
    | 'running'
    | 'completed'
    | 'failed'
    | 'blocked'
    | 'cancelled';

  type EdgeProvenance = 'planner' | 'codegraph' | 'knowledge' | 'runtime';

  interface BindingRef {
    kind: 'role' | 'zone';
    value: string;
  }

  interface WorkGraphNode {
    id: string;
    status: NodeStatus;
    lane: BindingRef;
    contract_summary: { input_count: number; output_count: number; acceptance_count: number };
  }

  interface WorkGraphEdge {
    source: string;
    target: string;
    kind: string;
    provenance: EdgeProvenance;
  }

  interface WorkGraphResponse {
    view: 'plan' | 'runtime' | 'divergence';
    source: 'live' | 'archive';
    nodes: WorkGraphNode[];
    edges: WorkGraphEdge[];
    waves: string[][];
    status_by_node: Record<string, NodeStatus>;
    lane_assignment: Record<string, BindingRef>;
    critical_path: string[];
    divergence: unknown | null;
  }

  const VIEWS = [
    { id: 'runtime', label: 'Runtime' },
    { id: 'plan', label: 'Plan' },
    { id: 'divergence', label: 'Divergence' }
  ] as const;

  type ViewId = (typeof VIEWS)[number]['id'];

  // Lane hues. Lanes are categorical, so colour is assigned by first-seen order
  // and kept stable for the lifetime of the payload.
  const LANE_HUES = [190, 265, 25, 330, 95, 45];

  const POLL_MS = 3000;
  const NODE_W = 62;
  const NODE_H = 26;
  const GAP_X = 12;
  const GAP_Y = 34;
  const PAD = 14;

  let graph = $state<WorkGraphResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let view = $state<ViewId>('runtime');
  let poll: ReturnType<typeof setInterval> | null = null;
  let lastKey = '';

  let sessionId = $derived($activeSession?.id ?? null);

  async function load(id: string, which: ViewId, showSpinner: boolean) {
    if (showSpinner) loading = true;
    try {
      const res = await fetch(apiUrl(`/api/sessions/${id}/work-graph?view=${which}`));
      if (!res.ok) {
        const raw = await res.text();
        let detail = raw;
        try {
          detail = (JSON.parse(raw) as { error?: string }).error ?? raw;
        } catch {
          // non-JSON body — surface it verbatim
        }
        throw new Error(detail || `HTTP ${res.status}`);
      }
      graph = (await res.json()) as WorkGraphResponse;
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function stopPoll() {
    if (poll) {
      clearInterval(poll);
      poll = null;
    }
  }

  $effect(() => {
    const id = sessionId;
    const which = view;
    stopPoll();
    if (!id) {
      graph = null;
      return;
    }
    const key = `${id}:${which}`;
    void load(id, which, key !== lastKey);
    lastKey = key;
    poll = setInterval(() => void load(id, which, false), POLL_MS);
  });

  onDestroy(stopPoll);

  const laneKey = (lane: BindingRef | undefined) =>
    lane ? `${lane.kind}:${lane.value}` : 'unassigned';

  let lanes = $derived.by(() => {
    const seen: string[] = [];
    for (const node of graph?.nodes ?? []) {
      const key = laneKey(node.lane);
      if (!seen.includes(key)) seen.push(key);
    }
    return seen;
  });

  function laneHue(key: string): number {
    const index = lanes.indexOf(key);
    return LANE_HUES[(index < 0 ? 0 : index) % LANE_HUES.length];
  }

  interface Placed {
    id: string;
    x: number;
    y: number;
    status: NodeStatus;
    laneK: string;
    onCritical: boolean;
  }

  let layout = $derived.by(() => {
    const placed = new Map<string, Placed>();
    const waves = graph?.waves ?? [];
    const statuses = graph?.status_by_node ?? {};
    const assignment = graph?.lane_assignment ?? {};
    const critical = new Set(graph?.critical_path ?? []);
    let width = 0;

    waves.forEach((wave, row) => {
      // Order within a wave by lane so lanes read as columns wherever the
      // dependency structure allows it.
      const ordered = [...wave].sort(
        (a, b) => lanes.indexOf(laneKey(assignment[a])) - lanes.indexOf(laneKey(assignment[b]))
      );
      ordered.forEach((id, col) => {
        const x = PAD + col * (NODE_W + GAP_X);
        placed.set(id, {
          id,
          x,
          y: PAD + row * (NODE_H + GAP_Y),
          status: statuses[id] ?? 'pending',
          laneK: laneKey(assignment[id]),
          onCritical: critical.has(id)
        });
        width = Math.max(width, x + NODE_W);
      });
    });

    const height = PAD + waves.length * (NODE_H + GAP_Y);
    return { placed, width: width + PAD, height: height + PAD - GAP_Y + NODE_H };
  });

  let links = $derived.by(() =>
    (graph?.edges ?? [])
      .map((edge) => {
        const from = layout.placed.get(edge.source);
        const to = layout.placed.get(edge.target);
        if (!from || !to) return null;
        return {
          key: `${edge.source}->${edge.target}:${edge.kind}`,
          x1: from.x + NODE_W / 2,
          y1: from.y + NODE_H,
          x2: to.x + NODE_W / 2,
          y2: to.y,
          provenance: edge.provenance,
          onCritical: from.onCritical && to.onCritical
        };
      })
      .filter((l): l is NonNullable<typeof l> => l !== null)
  );

  let nodes = $derived.by(() => [...layout.placed.values()]);
  let isEmpty = $derived(!loading && !error && graph !== null && graph.nodes.length === 0);
</script>

<div class="wg">
  <div class="wg-controls">
    {#each VIEWS as option (option.id)}
      <button
        type="button"
        class={`lattice-btn lattice-btn--xs ${view === option.id ? 'lattice-btn--primary' : 'lattice-btn--ghost'}`}
        onclick={() => (view = option.id)}
      >
        {option.label}
      </button>
    {/each}
    {#if graph}
      <span class="wg-source" title="Where this projection came from">{graph.source}</span>
    {/if}
  </div>

  <div class="wg-canvas">
    {#if loading && !graph}
      <div class="wg-pad">
        {#each ['46%', '72%', '58%', '80%'] as barWidth}
          <SkelBar width={barWidth} height="1.6rem" radius="md" />
        {/each}
      </div>
    {:else if error}
      <div class="wg-msg wg-msg--error">
        <p class="wg-msg-title">Could not load the work graph</p>
        <p class="wg-msg-body">{error}</p>
      </div>
    {:else if !sessionId}
      <div class="wg-msg">
        <p class="wg-msg-title">No active session</p>
      </div>
    {:else if isEmpty}
      <div class="wg-msg">
        <p class="wg-msg-title">No tasks in this work graph</p>
        <p class="wg-msg-body">The work graph endpoint returned no task nodes for this session.</p>
      </div>
    {:else if graph}
      <svg
        class="wg-svg"
        width={layout.width}
        height={layout.height}
        viewBox={`0 0 ${layout.width} ${layout.height}`}
        role="img"
        aria-label={`Work graph: ${graph.nodes.length} nodes across ${graph.waves.length} waves`}
      >
        <g class="wg-edges">
          {#each links as link (link.key)}
            <line
              x1={link.x1}
              y1={link.y1}
              x2={link.x2}
              y2={link.y2}
              class={`wg-edge wg-edge--${link.provenance}`}
              class:critical={link.onCritical}
            />
          {/each}
        </g>
        <g class="wg-nodes">
          {#each nodes as node (node.id)}
            <g class="wg-node" class:critical={node.onCritical}>
              <rect
                x={node.x}
                y={node.y}
                width={NODE_W}
                height={NODE_H}
                rx="5"
                class={`wg-box wg-box--${node.status}`}
                style={`--lane: hsl(${laneHue(node.laneK)} 70% 58%)`}
              >
                <title>{node.id} — {node.status} · {node.laneK}</title>
              </rect>
              <text
                x={node.x + NODE_W / 2}
                y={node.y + NODE_H / 2 + 4}
                text-anchor="middle"
                class="wg-label"
              >{node.id}</text>
            </g>
          {/each}
        </g>
      </svg>
    {/if}
  </div>

  {#if graph && graph.nodes.length > 0}
    <div class="wg-legend">
      <span class="wg-key"><i class="sw sw--completed"></i>done</span>
      <span class="wg-key"><i class="sw sw--running"></i>running</span>
      <span class="wg-key"><i class="sw sw--ready"></i>ready</span>
      <span class="wg-key"><i class="sw sw--blocked"></i>blocked</span>
      <span class="wg-key"><i class="sw sw--pending"></i>not started</span>
    </div>
  {/if}
</div>

<style>
  .wg {
    position: relative;
    display: flex;
    flex: 1;
    min-height: 0;
    flex-direction: column;
  }

  /* Floating control scrim over the canvas, not a structural toolbar row. */
  .wg-controls {
    position: absolute;
    top: 6px;
    right: 8px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 3px 4px;
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--bg-void) 82%, transparent);
    backdrop-filter: blur(4px);
  }

  .wg-source {
    padding-left: 2px;
    color: var(--text-disabled);
    font-size: 10px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .wg-canvas {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .wg-pad {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
  }

  .wg-svg {
    display: block;
  }

  .wg-msg {
    padding: 28px 16px;
    color: var(--text-secondary);
  }

  .wg-msg-title {
    margin: 0 0 6px;
    color: var(--text-primary);
    font-size: 13px;
  }

  .wg-msg-body {
    margin: 0;
    font-size: 12px;
    line-height: 1.6;
  }

  .wg-msg--error .wg-msg-title {
    color: var(--status-error);
  }

  .wg-edge {
    stroke-width: 1;
    opacity: 0.55;
  }

  .wg-edge--planner {
    stroke: var(--text-disabled);
  }

  .wg-edge--codegraph {
    stroke: var(--accent-cyan);
    stroke-dasharray: 3 3;
  }

  .wg-edge--knowledge {
    stroke: var(--status-warning);
    stroke-dasharray: 1 4;
  }

  .wg-edge--runtime {
    stroke: var(--status-success);
  }

  .wg-edge.critical {
    stroke-width: 2;
    opacity: 0.9;
  }

  /* Fill encodes status; the lane stripe encodes ownership. Blocked is a solid
     error fill and pending is a dashed hollow outline — the two states that read
     identically in a task list must never read identically here. */
  .wg-box {
    stroke: var(--lane);
    stroke-width: 2;
  }

  .wg-box--pending {
    fill: transparent;
    stroke-dasharray: 3 3;
    opacity: 0.65;
  }

  .wg-box--ready {
    fill: color-mix(in srgb, var(--accent-cyan) 18%, transparent);
  }

  .wg-box--running {
    fill: color-mix(in srgb, var(--accent-cyan) 45%, transparent);
  }

  .wg-box--completed {
    fill: color-mix(in srgb, var(--status-success) 40%, transparent);
  }

  .wg-box--blocked {
    fill: color-mix(in srgb, var(--status-error) 45%, transparent);
  }

  .wg-box--failed {
    fill: color-mix(in srgb, var(--status-error) 45%, transparent);
    stroke-dasharray: 2 2;
  }

  .wg-box--cancelled {
    fill: transparent;
    opacity: 0.4;
  }

  .wg-node.critical .wg-box {
    stroke-width: 3;
  }

  .wg-label {
    fill: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 10px;
    pointer-events: none;
  }

  .wg-legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    padding: 6px 10px;
    box-shadow: var(--edge-seam);
    color: var(--text-secondary);
    font-size: 10px;
  }

  .wg-key {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .sw {
    width: 9px;
    height: 9px;
    border: 1px solid var(--text-disabled);
    border-radius: 2px;
  }

  .sw--completed {
    background: color-mix(in srgb, var(--status-success) 40%, transparent);
  }

  .sw--running {
    background: color-mix(in srgb, var(--accent-cyan) 45%, transparent);
  }

  .sw--ready {
    background: color-mix(in srgb, var(--accent-cyan) 18%, transparent);
  }

  .sw--blocked {
    background: color-mix(in srgb, var(--status-error) 45%, transparent);
  }

  .sw--pending {
    background: transparent;
    border-style: dashed;
  }
</style>
