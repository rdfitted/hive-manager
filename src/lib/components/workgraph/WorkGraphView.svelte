<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { activeSession } from '$lib/stores/sessions';
  import { apiUrl } from '$lib/config';
  import {
    calculateCriticalPathRemaining,
    calculateWaveStatistics,
    classifyHeartbeatStaleness,
    formatElapsedTime,
    getNodeAdjacency,
    selectActiveWave,
    truncateLabel
  } from '$lib/workgraph/graphUtils';
  import type {
    BindingRef,
    NodeStatus,
    WorkGraphNode,
    WorkGraphOmissionReason,
    WorkGraphResponse
  } from '$lib/workgraph/types';
  import SkelBar from '../SkelBar.svelte';
  import NodeInspector from './NodeInspector.svelte';
  import ProgressHeader from './ProgressHeader.svelte';

  const VIEWS = [
    { id: 'runtime', label: 'Runtime' },
    { id: 'plan', label: 'Plan' },
    { id: 'divergence', label: 'Divergence' }
  ] as const;

  type ViewId = (typeof VIEWS)[number]['id'];

  // Lanes are categorical, so token colours are assigned by first-seen order
  // and kept stable for the lifetime of the payload.
  const LANE_COLOURS = [
    'var(--accent-cyan)',
    'var(--accent-chrome)',
    'var(--status-warning)',
    'var(--status-error)',
    'var(--status-success)',
    'var(--text-secondary)'
  ];

  const OMISSION_LABELS: Record<WorkGraphOmissionReason, string> = {
    codegraph_unavailable: 'Code graph unavailable',
    project_knowledge_unavailable: 'Project knowledge unavailable',
    source_unreadable: 'Source unreadable',
    resolution_incomplete: 'Resolution incomplete',
    completion_unresolved: 'Completion unresolved'
  };

  const POLL_MS = 3000;
  const CLOCK_TICK_MS = 1000;
  const NODE_W = 62;
  const NODE_H = 26;
  const GAP_X = 12;
  const GAP_Y = 34;
  const PAD = 14;
  const WAVE_RAIL_W = 118;
  const INSPECTOR_GAP = 8;
  const INSPECTOR_INSET = 8;

  let graph = $state<WorkGraphResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let view = $state<ViewId>('runtime');
  let poll: ReturnType<typeof setInterval> | null = null;
  let lastKey = '';
  let rootElement: HTMLDivElement;
  let canvasElement: HTMLDivElement;
  let scrollerElement: HTMLDivElement;
  let inspectorElement = $state<HTMLDivElement | undefined>(undefined);
  const nodeElements = new Map<string, SVGGElement>();

  let hoveredNodeId = $state<string | null>(null);
  let focusedNodeId = $state<string | null>(null);
  let pinnedNodeId = $state<string | null>(null);
  let dismissedNodeId = $state<string | null>(null);
  let inspectorLeft = $state(INSPECTOR_INSET);
  let inspectorTop = $state(INSPECTOR_INSET);
  let inspectorHorizontal = $state<'left' | 'right'>('right');
  let inspectorVertical = $state<'above' | 'below'>('below');
  let nowMs = $state(Date.now());

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

  $effect(() => {
    nowMs = Date.now();
    const clock = setInterval(() => {
      nowMs = Date.now();
    }, CLOCK_TICK_MS);
    return () => clearInterval(clock);
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

  function laneColour(key: string): string {
    const index = lanes.indexOf(key);
    return LANE_COLOURS[(index < 0 ? 0 : index) % LANE_COLOURS.length];
  }

  function registerNode(element: SVGGElement, id: string) {
    nodeElements.set(id, element);
    return {
      destroy() {
        if (nodeElements.get(id) === element) nodeElements.delete(id);
      }
    };
  }

  let visibleNodeId = $derived(
    pinnedNodeId ??
      (focusedNodeId !== dismissedNodeId ? focusedNodeId : null) ??
      (hoveredNodeId !== dismissedNodeId ? hoveredNodeId : null)
  );

  let graphNodeById = $derived.by(
    () => new Map((graph?.nodes ?? []).map((node) => [node.id, node]))
  );

  let completedNodeIds = $derived.by(
    () =>
      new Set(
        Object.entries(graph?.status_by_node ?? {})
          .filter(([, status]) => status === 'completed')
          .map(([id]) => id)
      )
  );

  let waveStatistics = $derived(
    calculateWaveStatistics(graph?.waves ?? [], completedNodeIds)
  );
  let criticalPathRemaining = $derived(
    calculateCriticalPathRemaining(graph?.critical_path ?? [], completedNodeIds)
  );
  let activeWave = $derived(selectActiveWave(graph?.waves ?? [], completedNodeIds));
  let graphOmissions = $derived(graph?.omissions ?? []);
  let omittedItemCount = $derived(
    graphOmissions.reduce((total, omission) => total + omission.count, 0)
  );

  let inspectedNode = $derived(
    visibleNodeId ? graphNodeById.get(visibleNodeId) ?? null : null
  );

  let inspectorNode = $derived.by(() =>
    inspectedNode
      ? {
          id: inspectedNode.id,
          title: inspectedNode.title,
          kind: inspectedNode.kind,
          lane: laneKey(inspectedNode.lane),
          status: inspectedNode.status,
          contract: inspectedNode.contract
        }
      : null
  );

  let inspectorDependencies = $derived.by(() => {
    if (!inspectedNode || !graph) return [];
    return getNodeAdjacency(inspectedNode.id, graph.edges).dependencies
      .map((id) => graphNodeById.get(id))
      .filter((node): node is WorkGraphNode => node !== undefined)
      .map((node) => ({ id: node.id, title: node.title, kind: node.kind }));
  });

  async function positionInspector(id: string) {
    await tick();
    const nodeElement = nodeElements.get(id);
    if (!nodeElement || !canvasElement || !inspectorElement) return;

    const canvasRect = canvasElement.getBoundingClientRect();
    const nodeRect = nodeElement.getBoundingClientRect();
    const inspectorRect = inspectorElement.getBoundingClientRect();
    const cardWidth = Math.min(
      inspectorRect.width,
      Math.max(0, canvasRect.width - INSPECTOR_INSET * 2)
    );
    const cardHeight = Math.min(
      inspectorRect.height,
      Math.max(0, canvasRect.height - INSPECTOR_INSET * 2)
    );
    const nodeLeft = nodeRect.left - canvasRect.left;
    const nodeRight = nodeRect.right - canvasRect.left;
    const nodeTop = nodeRect.top - canvasRect.top;
    const nodeBottom = nodeRect.bottom - canvasRect.top;

    const fitsRight = nodeRight + INSPECTOR_GAP + cardWidth <= canvasRect.width - INSPECTOR_INSET;
    inspectorHorizontal = fitsRight ? 'right' : 'left';
    const desiredLeft = fitsRight
      ? nodeRight + INSPECTOR_GAP
      : nodeLeft - cardWidth - INSPECTOR_GAP;
    inspectorLeft = Math.min(
      Math.max(INSPECTOR_INSET, desiredLeft),
      Math.max(INSPECTOR_INSET, canvasRect.width - cardWidth - INSPECTOR_INSET)
    );

    const fitsBelow = nodeBottom + INSPECTOR_GAP + cardHeight <= canvasRect.height - INSPECTOR_INSET;
    inspectorVertical = fitsBelow ? 'below' : 'above';
    const desiredTop = fitsBelow
      ? nodeBottom + INSPECTOR_GAP
      : nodeTop - cardHeight - INSPECTOR_GAP;
    inspectorTop = Math.min(
      Math.max(INSPECTOR_INSET, desiredTop),
      Math.max(INSPECTOR_INSET, canvasRect.height - cardHeight - INSPECTOR_INSET)
    );
  }

  function handleNodeEnter(id: string) {
    dismissedNodeId = null;
    hoveredNodeId = id;
    void positionInspector(id);
  }

  function handleNodeLeave(id: string) {
    if (hoveredNodeId === id) hoveredNodeId = null;
  }

  function handleNodeFocus(id: string) {
    dismissedNodeId = null;
    focusedNodeId = id;
    void positionInspector(id);
  }

  function handleNodeBlur(id: string) {
    if (focusedNodeId === id) focusedNodeId = null;
    if (dismissedNodeId === id) dismissedNodeId = null;
  }

  function pinNode(id: string, element: SVGGElement) {
    dismissedNodeId = null;
    pinnedNodeId = id;
    element.focus();
    void positionInspector(id);
  }

  function dismissInspector() {
    const id = visibleNodeId;
    pinnedNodeId = null;
    hoveredNodeId = null;
    if (id) dismissedNodeId = id;
  }

  function handleNodeKeydown(event: KeyboardEvent, id: string) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      pinNode(id, event.currentTarget as SVGGElement);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      dismissInspector();
    }
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && visibleNodeId) {
      event.preventDefault();
      dismissInspector();
    }
  }

  function handleWindowClick(event: MouseEvent) {
    if (!pinnedNodeId) return;
    const target = event.target;
    if (
      target instanceof Element &&
      rootElement?.contains(target) &&
      (target.closest('.wg-node') || target.closest('.wg-inspector-overlay'))
    ) {
      return;
    }
    dismissInspector();
  }

  function repositionInspector() {
    if (visibleNodeId) void positionInspector(visibleNodeId);
  }

  $effect(() => {
    const id = visibleNodeId;
    if (id) void positionInspector(id);
  });

  interface Placed {
    id: string;
    x: number;
    y: number;
    status: NodeStatus;
    laneK: string;
    onCritical: boolean;
    title: string;
    kind: WorkGraphNode['kind'] | 'unknown';
    waveIndex: number;
    progressText: string | null;
    heartbeatState: 'unknown' | 'fresh' | 'stale';
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
        const payloadNode = graphNodeById.get(id);
        const status = statuses[id] ?? 'pending';
        const progress = payloadNode?.progress;
        const heartbeatState = progress
          ? classifyHeartbeatStaleness(progress.last_heartbeat_at, undefined, nowMs)
          : 'unknown';
        let progressText: string | null = null;
        if (status === 'running') {
          if (!progress) {
            progressText = 'No timing';
          } else {
            const elapsed = formatElapsedTime(progress.started_at, progress.finished_at, nowMs);
            const heartbeatLabel =
              heartbeatState === 'stale'
                ? 'Stale'
                : heartbeatState === 'fresh'
                  ? 'Live'
                  : 'Unknown';
            progressText =
              heartbeatState === 'stale'
                ? `${heartbeatLabel} · ${elapsed}`
                : `${elapsed} · ${heartbeatLabel}`;
          }
        } else if (progress?.started_at && progress.finished_at) {
          progressText = formatElapsedTime(progress.started_at, progress.finished_at, nowMs);
        } else if (progress) {
          progressText = 'Timing incomplete';
        }
        const x = WAVE_RAIL_W + PAD + col * (NODE_W + GAP_X);
        placed.set(id, {
          id,
          x,
          y: PAD + row * (NODE_H + GAP_Y),
          status,
          laneK: laneKey(assignment[id]),
          onCritical: critical.has(id),
          title: payloadNode?.title || id,
          kind: payloadNode?.kind ?? 'unknown',
          waveIndex: row,
          progressText,
          heartbeatState
        });
        width = Math.max(width, x + NODE_W);
      });
    });

    const height = PAD + waves.length * (NODE_H + GAP_Y);
    return {
      placed,
      width: Math.max(width + PAD, WAVE_RAIL_W + PAD * 2),
      height: height + PAD - GAP_Y + NODE_H
    };
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

<svelte:window
  onkeydown={handleWindowKeydown}
  onclick={handleWindowClick}
  onresize={repositionInspector}
/>

<div class="wg" bind:this={rootElement}>
  <div class="wg-toolbar">
    {#if graph}
      <span class="wg-source" title="Where this projection came from">
        <span class="wg-source-label">Source</span>
        {graph.source}
      </span>
    {/if}
    <div class="wg-controls" role="group" aria-label="Work graph view">
      {#each VIEWS as option (option.id)}
        <button
          type="button"
          aria-pressed={view === option.id}
          class={`lattice-btn lattice-btn--xs ${view === option.id ? 'lattice-btn--primary' : 'lattice-btn--ghost'}`}
          onclick={() => (view = option.id)}
        >
          {option.label}
        </button>
      {/each}
    </div>
  </div>

  {#if graphOmissions.length > 0}
    <section
      class="wg-omissions lattice-forced-colors-boundary"
      aria-label="Work graph omissions"
    >
      <header class="wg-omissions-header">
        <h2>Not everything is shown</h2>
        <span class="wg-omissions-total">
          {omittedItemCount} omitted {omittedItemCount === 1 ? 'item' : 'items'}
        </span>
      </header>
      <p class="wg-omissions-intro">
        Some work-graph evidence could not be represented in this projection.
      </p>
      <ul class="wg-omission-list">
        {#each graphOmissions as omission, index (`${omission.reason}:${index}`)}
          <li class="wg-omission">
            <div class="wg-omission-header">
              <strong>{OMISSION_LABELS[omission.reason]}</strong>
              <span>Count {omission.count}</span>
            </div>
            <p>{omission.detail}</p>
            {#if omission.examples.length > 0}
              <div class="wg-omission-examples">
                <span>Examples</span>
                <ul>
                  {#each omission.examples as example}
                    <li><code>{example}</code></li>
                  {/each}
                </ul>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if graph && graph.nodes.length > 0}
    <div class="wg-progress">
      <ProgressHeader
        nodesComplete={waveStatistics.nodesComplete}
        nodesTotal={waveStatistics.nodesTotal}
        wavesComplete={waveStatistics.wavesComplete}
        wavesTotal={waveStatistics.wavesTotal}
        {criticalPathRemaining}
      />
    </div>
  {/if}

  <div class="wg-canvas" bind:this={canvasElement}>
    <div class="wg-scroller" bind:this={scrollerElement} onscroll={repositionInspector}>
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
          role="group"
          aria-label={`Work graph: ${graph.nodes.length} nodes across ${graph.waves.length} waves`}
        >
        <g class="wg-wave-rail" aria-label="Wave progress">
          {#each graph.waves as _, row}
            <text
              x="8"
              y={PAD + row * (NODE_H + GAP_Y) + NODE_H / 2 + 4}
              class="wg-wave-label"
              class:active={activeWave === row}
              data-wave-index={row}
              aria-label={`Wave ${row + 1} of ${graph.waves.length}${activeWave === row ? ', Active' : ''}`}
            >
              Wave {row + 1} of {graph.waves.length}{#if activeWave === row}<tspan>{' Active'}</tspan>{/if}
            </text>
          {/each}
        </g>
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
            <g
              class="wg-node"
              class:critical={node.onCritical}
              class:wg-node--context={node.kind === 'context'}
              class:wg-node--stale={node.status === 'running' && node.heartbeatState === 'stale'}
              class:wg-node--fresh={node.status === 'running' && node.heartbeatState === 'fresh'}
              data-node-id={node.id}
              data-node-kind={node.kind}
              data-heartbeat-state={node.heartbeatState}
              use:registerNode={node.id}
              role="button"
              tabindex="0"
              aria-label={`${node.title} — ${node.id}, ${node.kind}, ${node.status}, ${node.laneK}`}
              aria-expanded={visibleNodeId === node.id}
              aria-controls={visibleNodeId === node.id ? 'wg-node-inspector' : undefined}
              onmouseenter={() => handleNodeEnter(node.id)}
              onmouseleave={() => handleNodeLeave(node.id)}
              onfocus={() => handleNodeFocus(node.id)}
              onblur={() => handleNodeBlur(node.id)}
              onclick={(event) => pinNode(node.id, event.currentTarget as SVGGElement)}
              onkeydown={(event) => handleNodeKeydown(event, node.id)}
              class:pinned={pinnedNodeId === node.id}
            >
              <rect
                x={node.x}
                y={node.y}
                width={NODE_W}
                height={NODE_H}
                rx={node.kind === 'context' ? NODE_H / 2 : 5}
                class={`wg-box wg-box--${node.status}`}
                style={`--lane: ${laneColour(node.laneK)}`}
              />
              <svg
                class="wg-label-clip"
                x={node.x}
                y={node.y}
                width={NODE_W}
                height={NODE_H}
                viewBox={`0 0 ${NODE_W} ${NODE_H}`}
                overflow="hidden"
                aria-hidden="true"
              >
                <text
                  x={NODE_W / 2}
                  y={NODE_H / 2 + 4}
                  text-anchor="middle"
                  class="wg-label"
                >{truncateLabel(node.title)}</text>
              </svg>
              {#if node.progressText}
                <svg
                  class="wg-progress-clip"
                  x={node.x}
                  y={node.y + NODE_H}
                  width={NODE_W}
                  height="18"
                  viewBox={`0 0 ${NODE_W} 18`}
                  overflow="hidden"
                  aria-hidden="true"
                >
                  <text
                    x={NODE_W / 2}
                    y="12"
                    text-anchor="middle"
                    class="wg-node-progress"
                    class:stale={node.heartbeatState === 'stale'}
                  >{node.progressText}</text>
                </svg>
              {/if}
            </g>
          {/each}
        </g>
        </svg>
      {/if}
    </div>

    {#if inspectorNode}
      <div
        id="wg-node-inspector"
        class="wg-inspector-overlay"
        bind:this={inspectorElement}
        data-anchor-horizontal={inspectorHorizontal}
        data-anchor-vertical={inspectorVertical}
        data-pinned={pinnedNodeId === inspectorNode.id}
        style={`left: ${inspectorLeft}px; top: ${inspectorTop}px`}
      >
        <NodeInspector
          node={inspectorNode}
          dependencies={inspectorDependencies}
          progress={inspectedNode?.progress ?? null}
        />
      </div>
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

  /* Reserve structural space above the canvas so the controls can never cover
     the first wave, including when a narrow panel makes this row wrap. */
  .wg-toolbar {
    display: flex;
    align-items: center;
    flex: 0 0 auto;
    flex-wrap: wrap;
    gap: 4px 8px;
    padding: 6px 8px;
    box-shadow: var(--edge-seam);
  }

  .wg-controls {
    display: flex;
    max-width: 100%;
    margin-left: auto;
    gap: 4px;
    overflow-x: auto;
  }

  .wg-source {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    color: var(--text-disabled);
    font-size: 10px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .wg-source-label {
    color: var(--text-secondary);
  }

  .wg-omissions {
    flex: 0 0 auto;
    max-height: 180px;
    margin: var(--space-2);
    padding: var(--space-3);
    overflow: auto;
    border: 1px solid var(--status-warning);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--status-warning) 8%, var(--bg-panel));
    box-shadow: var(--edge-seam);
    color: var(--text-primary);
  }

  .wg-omissions-header,
  .wg-omission-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .wg-omissions h2 {
    margin: 0;
    font: var(--text-small) var(--font-display);
  }

  .wg-omissions-total,
  .wg-omission-header span,
  .wg-omission-examples > span {
    color: var(--text-secondary);
    font: var(--text-micro) var(--font-mono);
  }

  .wg-omissions-intro,
  .wg-omission p {
    margin: var(--space-1) 0 0;
    color: var(--text-secondary);
    font: var(--text-small) var(--font-body);
  }

  .wg-omission-list,
  .wg-omission-examples ul {
    display: grid;
    gap: var(--space-2);
    margin: var(--space-2) 0 0;
    padding: 0;
    list-style: none;
  }

  .wg-omission {
    padding-top: var(--space-2);
    box-shadow: var(--edge-seam-top);
  }

  .wg-omission-header strong {
    font: var(--text-small) var(--font-mono);
  }

  .wg-omission-examples {
    margin-top: var(--space-2);
  }

  .wg-omission-examples ul {
    gap: var(--space-1);
    margin-top: var(--space-1);
  }

  .wg-omission-examples code {
    overflow-wrap: anywhere;
    color: var(--text-primary);
    font: var(--text-micro) var(--font-mono);
  }

  .wg-progress {
    flex: 0 0 auto;
    padding: var(--space-2);
    box-shadow: var(--edge-seam);
  }

  .wg-canvas {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .wg-scroller {
    width: 100%;
    height: 100%;
    overflow: auto;
  }

  .wg-inspector-overlay {
    position: absolute;
    z-index: 3;
    width: min(360px, calc(100% - 16px));
    max-height: calc(100% - 16px);
    overflow: auto;
    border-radius: var(--radius-lg);
    box-shadow: var(--elev-3);
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

  .wg-wave-label {
    fill: var(--text-disabled);
    font-family: var(--font-mono);
    font-size: 9px;
  }

  .wg-wave-label.active {
    fill: var(--text-primary);
    font-weight: 700;
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

  .wg-node:focus-visible {
    outline: none;
  }

  .wg-node:focus-visible .wg-box,
  .wg-node.pinned .wg-box {
    stroke: var(--accent-cyan);
    stroke-width: 3;
  }

  /* Runtime bookkeeping stays discoverable but task-shaped work remains the
     dominant visual layer. The pill shape is a non-colour-only distinction. */
  .wg-node--context .wg-box {
    stroke-width: 1;
    stroke-dasharray: 1 3;
    opacity: 0.65;
  }

  .wg-node--context .wg-label {
    font-style: italic;
    opacity: 0.75;
  }

  .wg-progress-clip {
    overflow: hidden;
    pointer-events: none;
  }

  .wg-node-progress {
    fill: var(--text-secondary);
    font-family: var(--font-mono);
    font-size: 8px;
    pointer-events: none;
  }

  .wg-node-progress.stale {
    fill: var(--status-warning);
    font-weight: 700;
  }

  .wg-node--stale .wg-box {
    stroke: var(--status-warning);
    stroke-dasharray: 2 2;
  }

  .wg-node--fresh .wg-box--running {
    animation: wg-running-pulse 1.5s ease-in-out infinite;
  }

  .wg-label-clip {
    overflow: hidden;
    pointer-events: none;
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

  @keyframes wg-running-pulse {
    50% {
      opacity: 0.7;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .wg-node--fresh .wg-box--running {
      animation: none;
    }
  }

  @media (forced-colors: active) {
    .wg-box {
      forced-color-adjust: auto;
      fill: Canvas;
      stroke: CanvasText;
    }

    /* Forced colours intentionally flatten fills, so preserve the complete
       status vocabulary with pairwise-distinct, non-colour stroke patterns.
       Stroke width remains reserved for critical-path and focus/pin emphasis. */
    .wg-box--ready {
      stroke-dasharray: none;
    }

    .wg-box--running {
      stroke-dasharray: 12 2;
    }

    .wg-box--completed {
      stroke-dasharray: 1 3;
      stroke-linecap: round;
    }

    .wg-box--blocked {
      stroke-dasharray: 8 2 2 2;
    }

    .wg-box--pending {
      stroke-dasharray: 4 4;
    }

    .wg-box--failed {
      stroke-dasharray: 2 2;
    }

    .wg-box--cancelled {
      stroke-dasharray: 2 3 2 8;
    }

    .wg-node:focus-visible .wg-box,
    .wg-node.pinned .wg-box {
      stroke: Highlight;
    }

    .wg-label,
    .wg-node-progress,
    .wg-wave-label {
      fill: CanvasText;
    }
  }
</style>
