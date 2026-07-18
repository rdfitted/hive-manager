<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { ArrowClockwise } from 'phosphor-svelte';
  import { createForceSimulation, type ForceNode, type ForceSimulation } from '$lib/knowledge/forceSim';
  import { EDGE_COLORS, folderColor, nodeDegree } from '$lib/knowledge/graphUtils';
  import type { KnowledgeEdge, KnowledgeNode } from '$lib/knowledge/types';

  interface Props {
    nodes: KnowledgeNode[];
    edges: KnowledgeEdge[];
    selectedId: string | null;
    onSelect: (id: string, trigger?: Element) => void;
  }

  interface DragSnapshot {
    id: string;
    x: number;
    y: number;
    vx: number;
    vy: number;
    fx: number | null;
    fy: number | null;
  }

  const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)';

  function getReducedMotionQuery(): MediaQueryList | null {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return null;
    return window.matchMedia(REDUCED_MOTION_QUERY);
  }

  let { nodes, edges, selectedId, onSelect }: Props = $props();
  let host: HTMLDivElement;
  let svg: SVGSVGElement;
  let width = $state(900);
  let height = $state(620);
  let positions = $state<ForceNode[]>([]);
  let simulation: ForceSimulation | null = null;
  let animationFrame: number | null = null;
  let resetVersion = $state(0);
  let hoveredId = $state<string | null>(null);
  let draggingId: string | null = null;
  let draggingElement: SVGGElement | null = null;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragMoved = false;
  let dragSnapshot: DragSnapshot | null = null;
  let reducedMotionQuery = getReducedMotionQuery();
  let reducedMotion = $state(reducedMotionQuery?.matches ?? false);

  let positionById = $derived.by(() => new Map(positions.map((node) => [node.id, node])));
  let pinnedCount = $derived(positions.filter((node) => node.fx !== null).length);
  let selectedNeighbors = $derived.by(() => {
    const ids = new Set<string>();
    if (!selectedId) return ids;
    ids.add(selectedId);
    for (const edge of edges) {
      if (edge.source === selectedId) ids.add(edge.target);
      if (edge.target === selectedId) ids.add(edge.source);
    }
    return ids;
  });

  function stopAnimation() {
    if (animationFrame !== null) {
      cancelAnimationFrame(animationFrame);
      animationFrame = null;
    }
  }

  function animate() {
    if (!simulation || reducedMotion) {
      animationFrame = null;
      return;
    }
    const alpha = simulation.tick();
    positions = [...simulation.nodes];
    if (alpha > 0.008 || draggingId !== null) {
      animationFrame = requestAnimationFrame(animate);
    } else {
      animationFrame = null;
    }
  }

  function scheduleAnimation() {
    if (reducedMotion) {
      stopAnimation();
      if (simulation) positions = [...simulation.nodes];
      return;
    }
    if (animationFrame === null) animationFrame = requestAnimationFrame(animate);
  }

  $effect(() => {
    const currentNodes = nodes;
    const currentEdges = edges;
    resetVersion;
    const currentWidth = untrack(() => width);
    const currentHeight = untrack(() => height);
    stopAnimation();
    simulation = createForceSimulation(currentNodes, currentEdges, currentWidth, currentHeight);
    positions = [...simulation.nodes];
    untrack(() => scheduleAnimation());

    return () => stopAnimation();
  });

  $effect(() => {
    const currentWidth = width;
    const currentHeight = height;
    if (!simulation) return;

    simulation.setBounds(currentWidth, currentHeight);
    positions = [...simulation.nodes];
    untrack(() => scheduleAnimation());
  });

  onMount(() => {
    const updateSize = () => {
      const rect = host.getBoundingClientRect();
      const nextWidth = Math.max(Math.round(rect.width), 320);
      const nextHeight = Math.max(Math.round(rect.height), 320);
      if (Math.abs(nextWidth - width) > 4) width = nextWidth;
      if (Math.abs(nextHeight - height) > 4) height = nextHeight;
    };

    updateSize();
    const observer = new ResizeObserver(updateSize);
    observer.observe(host);

    reducedMotionQuery ??= getReducedMotionQuery();
    const updateMotionPreference = () => {
      const nextReducedMotion = reducedMotionQuery?.matches ?? false;
      if (nextReducedMotion === reducedMotion) return;
      reducedMotion = nextReducedMotion;
      if (reducedMotion) {
        stopAnimation();
        if (simulation) positions = [...simulation.nodes];
      } else {
        scheduleAnimation();
      }
    };
    reducedMotionQuery?.addEventListener('change', updateMotionPreference);

    return () => {
      observer.disconnect();
      reducedMotionQuery?.removeEventListener('change', updateMotionPreference);
    };
  });

  function graphPoint(event: PointerEvent): { x: number; y: number } {
    const rect = svg.getBoundingClientRect();
    return {
      x: ((event.clientX - rect.left) / Math.max(rect.width, 1)) * width,
      y: ((event.clientY - rect.top) / Math.max(rect.height, 1)) * height,
    };
  }

  function handlePointerDown(event: PointerEvent, node: ForceNode) {
    if (event.button !== 0 || !simulation) return;
    event.preventDefault();
    (event.currentTarget as SVGGElement).setPointerCapture(event.pointerId);
    draggingId = node.id;
    draggingElement = event.currentTarget as SVGGElement;
    dragStartX = event.clientX;
    dragStartY = event.clientY;
    dragMoved = false;
    dragSnapshot = {
      id: node.id,
      x: node.x,
      y: node.y,
      vx: node.vx,
      vy: node.vy,
      fx: node.fx,
      fy: node.fy,
    };
    simulation.setPinned(node.id, node.x, node.y);
    positions = [...simulation.nodes];
    scheduleAnimation();
  }

  function handlePointerMove(event: PointerEvent) {
    if (!draggingId || !simulation) return;
    if (Math.hypot(event.clientX - dragStartX, event.clientY - dragStartY) > 3) {
      dragMoved = true;
    }
    const point = graphPoint(event);
    simulation.setPinned(draggingId, point.x, point.y);
    positions = [...simulation.nodes];
    scheduleAnimation();
  }

  function releasePointerCapture(pointerId: number) {
    if (draggingElement?.hasPointerCapture(pointerId)) {
      draggingElement.releasePointerCapture(pointerId);
    }
  }

  function finishPointer(event: PointerEvent) {
    if (!draggingId || !simulation) return;
    const selected = draggingId;
    const trigger = draggingElement;
    const startedPinned = dragSnapshot !== null && dragSnapshot.fx !== null && dragSnapshot.fy !== null;
    if (!dragMoved && !startedPinned) simulation.unpin(selected);
    releasePointerCapture(event.pointerId);
    draggingId = null;
    draggingElement = null;
    dragSnapshot = null;
    positions = [...simulation.nodes];
    scheduleAnimation();
    onSelect(selected, trigger ?? undefined);
  }

  function cancelPointer(event: PointerEvent) {
    if (!draggingId || !simulation) return;
    const snapshot = dragSnapshot;
    if (snapshot?.id === draggingId) {
      const node = simulation.nodes.find((entry) => entry.id === snapshot.id);
      if (node) {
        node.x = snapshot.x;
        node.y = snapshot.y;
        node.vx = snapshot.vx;
        node.vy = snapshot.vy;
        node.fx = snapshot.fx;
        node.fy = snapshot.fy;
      }
    }
    releasePointerCapture(event.pointerId);
    draggingId = null;
    draggingElement = null;
    dragSnapshot = null;
    positions = [...simulation.nodes];
    scheduleAnimation();
  }

  function unpinNode(id: string) {
    simulation?.unpin(id);
    if (simulation) positions = [...simulation.nodes];
    scheduleAnimation();
  }

  function unpinAll() {
    simulation?.unpinAll();
    if (simulation) positions = [...simulation.nodes];
    scheduleAnimation();
  }

  function radius(node: KnowledgeNode): number {
    return Math.min(11, 5 + Math.sqrt(nodeDegree(node)) * 1.25);
  }

  function shortTitle(title: string): string {
    return title.length > 34 ? `${title.slice(0, 31)}…` : title;
  }
</script>

<div class="graph-host" bind:this={host}>
  <div class="graph-controls" aria-label="Graph layout controls">
    <span class="pin-count">{pinnedCount} pinned</span>
    <button type="button" onclick={unpinAll} disabled={pinnedCount === 0}>Unpin all</button>
    <button type="button" onclick={() => resetVersion += 1} title="Reset graph layout">
      <ArrowClockwise size={13} weight="light" />
      Reset
    </button>
  </div>

  <svg
    bind:this={svg}
    viewBox={`0 0 ${width} ${height}`}
    role="group"
    aria-label={`Interactive knowledge graph with ${nodes.length} pages and ${edges.length} relationships`}
    onpointermove={handlePointerMove}
    onpointerup={finishPointer}
    onpointercancel={cancelPointer}
  >
    <defs>
      <pattern id="knowledge-grid" width="28" height="28" patternUnits="userSpaceOnUse">
        <path d="M 28 0 L 0 0 0 28" fill="none" stroke="var(--border-structural)" stroke-width="0.45" />
      </pattern>
      <filter id="node-glow" x="-100%" y="-100%" width="300%" height="300%">
        <feGaussianBlur stdDeviation="2" result="glow" />
        <feMerge><feMergeNode in="glow" /><feMergeNode in="SourceGraphic" /></feMerge>
      </filter>
    </defs>
    <rect width={width} height={height} fill="url(#knowledge-grid)" />

    <g class="edges" aria-hidden="true">
      {#each edges as edge, index (`${edge.source}:${edge.target}:${edge.kind}:${index}`)}
        {@const source = positionById.get(edge.source)}
        {@const target = positionById.get(edge.target)}
        {#if source && target}
          <line
            x1={source.x}
            y1={source.y}
            x2={target.x}
            y2={target.y}
            stroke={EDGE_COLORS[edge.kind]}
            class:muted={selectedId !== null && edge.source !== selectedId && edge.target !== selectedId}
            class:related={edge.kind === 'related'}
          />
        {/if}
      {/each}
    </g>

    <g class="nodes">
      {#each positions as node (node.id)}
        {@const isSelected = node.id === selectedId}
        {@const isMuted = selectedId !== null && !selectedNeighbors.has(node.id)}
        <g
          class="node"
          class:selected={isSelected}
          class:muted={isMuted}
          class:pinned={node.fx !== null}
          transform={`translate(${node.x} ${node.y})`}
          role="button"
          tabindex="0"
          aria-label={`${node.title}, ${node.folder}, ${nodeDegree(node)} connections${node.fx !== null ? ', pinned' : ''}`}
          onpointerdown={(event) => handlePointerDown(event, node)}
          onmouseenter={() => hoveredId = node.id}
          onmouseleave={() => hoveredId = null}
          ondblclick={() => unpinNode(node.id)}
          onkeydown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') {
              event.preventDefault();
              onSelect(node.id, event.currentTarget);
            }
          }}
        >
          <circle class="node-halo" r={radius(node) + 5} fill={folderColor(node.folder)} />
          <circle class="node-core" r={radius(node)} fill={folderColor(node.folder)} />
          {#if node.fx !== null}
            <circle class="pin-mark" cx={radius(node) - 1} cy={-radius(node) + 1} r="2.5" />
          {/if}
          {#if isSelected || hoveredId === node.id || nodes.length <= 36}
            <text x={radius(node) + 7} y="4">{shortTitle(node.title)}</text>
          {/if}
          <title>{node.title} · {node.path} · Double-click to unpin</title>
        </g>
      {/each}
    </g>
  </svg>

  <div class="graph-hint">Drag to pin · Double-click to release · Select to read</div>
</div>

<style>
  .graph-host {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 320px;
    overflow: hidden;
    background:
      radial-gradient(circle at 50% 45%, color-mix(in srgb, var(--accent-cyan) 5%, transparent), transparent 54%),
      var(--bg-void);
  }

  svg {
    display: block;
    width: 100%;
    height: 100%;
    touch-action: none;
    user-select: none;
  }

  .edges line {
    stroke-width: 1;
    stroke-opacity: 0.38;
    transition: stroke-opacity 160ms ease;
  }

  .edges line.related {
    stroke-dasharray: 4 4;
  }

  .edges line.muted {
    stroke-opacity: 0.055;
  }

  .node {
    cursor: grab;
    outline: none;
    opacity: 0.9;
    transition: opacity 160ms ease;
  }

  .node:active {
    cursor: grabbing;
  }

  .node.muted {
    opacity: 0.16;
  }

  .node-core {
    stroke: var(--bg-void);
    stroke-width: 2;
  }

  .node-halo {
    opacity: 0.08;
  }

  .node:hover .node-halo,
  .node:focus-visible .node-halo,
  .node.selected .node-halo {
    opacity: 0.28;
  }

  .node:hover .node-core,
  .node:focus-visible .node-core,
  .node.selected .node-core {
    stroke: var(--text-primary);
    filter: url(#node-glow);
  }

  .node.selected .node-core {
    stroke-width: 2.5;
  }

  .node text {
    fill: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 10px;
    paint-order: stroke;
    stroke: var(--bg-void);
    stroke-width: 3px;
    stroke-linejoin: round;
    pointer-events: none;
  }

  .pin-mark {
    fill: var(--bg-void);
    stroke: var(--text-primary);
    stroke-width: 1;
    pointer-events: none;
  }

  .graph-controls {
    position: absolute;
    top: var(--space-3);
    right: var(--space-3);
    z-index: 2;
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-1);
    border: 1px solid var(--border-structural);
    background: color-mix(in srgb, var(--bg-surface) 91%, transparent);
    backdrop-filter: blur(8px);
  }

  .graph-controls button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 0;
    padding: 5px 7px;
    background: transparent;
    color: var(--text-secondary);
    font: 10px var(--font-mono);
    text-transform: uppercase;
    cursor: pointer;
  }

  .graph-controls button:hover:not(:disabled) {
    color: var(--accent-cyan);
    background: var(--bg-elevated);
  }

  .graph-controls button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .pin-count {
    padding: 0 6px;
    color: var(--text-disabled);
    font: 10px var(--font-mono);
    text-transform: uppercase;
  }

  .graph-hint {
    position: absolute;
    left: var(--space-3);
    bottom: var(--space-3);
    color: var(--text-disabled);
    font: 10px var(--font-mono);
    letter-spacing: 0.025em;
    pointer-events: none;
  }

  @media (prefers-reduced-motion: reduce) {
    .node,
    .edges line {
      transition: none;
    }
  }
</style>
