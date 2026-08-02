<script module lang="ts">
  import { resolveTokens, type CssTokenName } from '$lib/theme/resolveTokens';

  export interface SvgShadowLayer {
    dx: number;
    dy: number;
    stdDeviation: number;
    floodColor: string;
    floodOpacity: number;
  }

  export interface NodeGlowParameters {
    stdDeviation: number;
    opacity: number;
  }

  export const HALO_SCALE = 1.625;
  export const DEFAULT_NODE_GLOW: NodeGlowParameters = {
    stdDeviation: 2,
    opacity: 0.28,
  };

  const NODE_GLOW_TOKENS = {
    interactive: '--elev-2',
  } as const satisfies Record<string, CssTokenName>;
  const RGB_FUNCTION = /^rgba?\((.*)\)$/i;
  const PIXEL_LENGTH = /^([+-]?(?:\d+(?:\.\d+)?|\.\d+))px$/i;
  const UNITLESS_ZERO = /^[+-]?(?:0+(?:\.0*)?|\.0+)$/;

  export function haloRadius(radius: number): number {
    return radius * HALO_SCALE;
  }

  function splitAtTopLevel(value: string, separator: string): string[] {
    const parts: string[] = [];
    let depth = 0;
    let start = 0;

    for (let index = 0; index < value.length; index += 1) {
      const character = value[index];
      if (character === '(') depth += 1;
      else if (character === ')') {
        depth -= 1;
        if (depth < 0) throw new Error(`Unbalanced CSS shadow value: ${value}`);
      } else if (character === separator && depth === 0) {
        const part = value.slice(start, index).trim();
        if (!part) throw new Error(`Empty CSS shadow segment: ${value}`);
        parts.push(part);
        start = index + 1;
      }
    }

    if (depth !== 0) throw new Error(`Unbalanced CSS shadow value: ${value}`);
    const tail = value.slice(start).trim();
    if (!tail) throw new Error(`Empty CSS shadow segment: ${value}`);
    parts.push(tail);
    return parts;
  }

  function tokenizeShadowLayer(value: string): string[] {
    const tokens: string[] = [];
    let depth = 0;
    let start = -1;

    for (let index = 0; index <= value.length; index += 1) {
      const character = value[index] ?? ' ';
      if (character === '(') depth += 1;
      else if (character === ')') {
        depth -= 1;
        if (depth < 0) throw new Error(`Unbalanced CSS shadow layer: ${value}`);
      }

      const separator = /\s/.test(character) && depth === 0;
      if (separator) {
        if (start !== -1) {
          tokens.push(value.slice(start, index));
          start = -1;
        }
      } else if (start === -1) {
        start = index;
      }
    }

    if (depth !== 0) throw new Error(`Unbalanced CSS shadow layer: ${value}`);
    return tokens;
  }

  function parsePixelLength(token: string): number | null {
    if (UNITLESS_ZERO.test(token)) return 0;
    const match = PIXEL_LENGTH.exec(token);
    return match ? Number(match[1]) : null;
  }

  function parseRgbChannel(token: string): number {
    const trimmed = token.trim();
    const value = trimmed.endsWith('%')
      ? Number(trimmed.slice(0, -1)) * 2.55
      : Number(trimmed);
    if (!Number.isFinite(value) || value < 0 || value > 255) {
      throw new Error(`Invalid RGB channel in elevation token: ${token}`);
    }
    return value;
  }

  function parseAlpha(token: string | undefined): number {
    if (token === undefined) return 1;
    const trimmed = token.trim();
    const value = trimmed.endsWith('%')
      ? Number(trimmed.slice(0, -1)) / 100
      : Number(trimmed);
    if (!Number.isFinite(value) || value < 0 || value > 1) {
      throw new Error(`Invalid alpha channel in elevation token: ${token}`);
    }
    return value;
  }

  function parseRgbColor(token: string): { floodColor: string; floodOpacity: number } {
    const match = RGB_FUNCTION.exec(token);
    if (!match) throw new Error(`Unsupported elevation shadow color: ${token}`);

    const body = match[1].trim();
    let channels: string[];
    let alpha: string | undefined;
    if (body.includes(',')) {
      const parts = body.split(',').map((part) => part.trim());
      if (parts.length !== 3 && parts.length !== 4) {
        throw new Error(`Unsupported elevation shadow color: ${token}`);
      }
      channels = parts.slice(0, 3);
      alpha = parts[3];
    } else {
      const slashParts = body.split('/').map((part) => part.trim());
      if (slashParts.length > 2) throw new Error(`Unsupported elevation shadow color: ${token}`);
      channels = slashParts[0].split(/\s+/);
      alpha = slashParts[1];
    }

    if (channels.length !== 3) throw new Error(`Unsupported elevation shadow color: ${token}`);
    const [red, green, blue] = channels.map(parseRgbChannel);
    return {
      floodColor: `rgb(${red}, ${green}, ${blue})`,
      floodOpacity: parseAlpha(alpha),
    };
  }

  /** Parse a browser-computed box-shadow list into concrete SVG filter values. */
  export function parseComputedBoxShadows(value: string): SvgShadowLayer[] {
    if (!value.trim() || value.trim().toLowerCase() === 'none') {
      throw new Error('Elevation tokens must contain at least one outer shadow');
    }

    return splitAtTopLevel(value, ',').map((shadow) => {
      const lengths: number[] = [];
      let color: { floodColor: string; floodOpacity: number } | null = null;

      for (const token of tokenizeShadowLayer(shadow)) {
        if (token.toLowerCase() === 'inset') {
          throw new Error(`Inset shadows cannot drive the node glow: ${shadow}`);
        }
        if (RGB_FUNCTION.test(token)) {
          if (color) throw new Error(`Multiple colors in elevation shadow: ${shadow}`);
          color = parseRgbColor(token);
          continue;
        }
        const length = parsePixelLength(token);
        if (length === null) throw new Error(`Unsupported elevation shadow token: ${token}`);
        lengths.push(length);
      }

      if (!color) throw new Error(`Elevation shadow has no computed RGB color: ${shadow}`);
      if (lengths.length < 2 || lengths.length > 4) {
        throw new Error(`Elevation shadow must contain two to four lengths: ${shadow}`);
      }
      const [dx, dy, blur = 0, spread = 0] = lengths;
      if (blur < 0) throw new Error(`Elevation shadow blur cannot be negative: ${shadow}`);
      if (spread !== 0) {
        throw new Error(`Non-zero spread cannot drive the node glow: ${shadow}`);
      }

      return {
        dx,
        dy,
        stdDeviation: blur / 2,
        floodColor: color.floodColor,
        floodOpacity: color.floodOpacity,
      };
    });
  }

  function canonicalizeBoxShadow(value: string): string {
    const probe = document.createElement('span');
    probe.style.position = 'fixed';
    probe.style.visibility = 'hidden';
    probe.style.pointerEvents = 'none';
    probe.style.boxShadow = value;
    if (!probe.style.boxShadow) throw new Error(`Invalid elevation token value: ${value}`);

    document.documentElement.append(probe);
    try {
      const computed = getComputedStyle(probe).boxShadow;
      if (!computed || computed === 'none') throw new Error(`Invalid elevation token value: ${value}`);
      return computed;
    } finally {
      probe.remove();
    }
  }

  /**
   * Preserve the proven single-blur filter shape while sourcing its depth from
   * the first `--elev-2` layer (sigma 2 and opacity .55 in the current ramp).
   */
  export function resolveNodeGlowParameters(): NodeGlowParameters {
    const resolved = resolveTokens(NODE_GLOW_TOKENS);
    const [layer] = parseComputedBoxShadows(canonicalizeBoxShadow(resolved.interactive));
    if (!layer) throw new Error('The interactive elevation token has no shadow layer');
    return {
      stdDeviation: layer.stdDeviation,
      opacity: layer.floodOpacity,
    };
  }
</script>

<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { ArrowClockwise } from 'phosphor-svelte';
  import { createForceSimulation, type ForceNode, type ForceSimulation } from '$lib/knowledge/forceSim';
  import {
    EDGE_COLORS,
    folderColor,
    folderKindLabel,
    isRelationshipFolder,
    nodeDegree,
  } from '$lib/knowledge/graphUtils';
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
  let nodeGlow = $state<NodeGlowParameters>(DEFAULT_NODE_GLOW);

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
    try {
      nodeGlow = resolveNodeGlowParameters();
    } catch {
      // Component tests and defensive embeds may mount without the global token sheet.
      nodeGlow = DEFAULT_NODE_GLOW;
    }

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

  function nodeHaloRadius(node: KnowledgeNode): number {
    return haloRadius(radius(node));
  }

  /**
   * Side of a 45deg-rotated square whose half-diagonal equals `r`, so a diamond
   * occupies exactly the circle's horizontal/vertical extent. Layout, label
   * offsets and the pin mark all keep using radius() untouched.
   */
  function diamondSide(r: number): number {
    return r * Math.SQRT2;
  }

  /**
   * Axis coordinate of the pin mark's centre. A circle's bounding-box corner
   * inset 1px lands on the rim; a diamond's 45deg diagonal crosses its edge at
   * r/2, nudged 1px out so the mark overhangs by a constant ~1.4px at every
   * degree instead of drifting off the edge as the node grows.
   */
  function pinAxis(r: number, isRelationship: boolean): number {
    return isRelationship ? r / 2 + 1 : r - 1;
  }

  function shortTitle(title: string): string {
    return title.length > 34 ? `${title.slice(0, 31)}…` : title;
  }
</script>

<div class="graph-host" bind:this={host}>
  <div class="graph-controls" aria-label="Graph layout controls">
    <span class="pin-count">{pinnedCount} pinned</span>
    <button
      class="lattice-btn lattice-btn--secondary lattice-btn--compact"
      type="button"
      onclick={unpinAll}
      disabled={pinnedCount === 0}
    >
      Unpin all
    </button>
    <button
      class="lattice-btn lattice-btn--secondary lattice-btn--compact"
      type="button"
      onclick={() => resetVersion += 1}
      title="Reset graph layout"
    >
      <ArrowClockwise size={13} weight="light" />
      Reset
    </button>
  </div>

  <!-- Keep normal nodes filterless. The proven interactive glow retains one
       Gaussian primitive while taking blur/halo opacity from --elev-2. -->
  <svg
    bind:this={svg}
    viewBox={`0 0 ${width} ${height}`}
    role="group"
    aria-label={`Interactive knowledge graph with ${nodes.length} pages and ${edges.length} relationships`}
    onpointermove={handlePointerMove}
    onpointerup={finishPointer}
    onpointercancel={cancelPointer}
    style={`--node-glow-opacity: ${nodeGlow.opacity};`}
  >
    <defs>
      <pattern id="knowledge-grid" width="28" height="28" patternUnits="userSpaceOnUse">
        <path d="M 28 0 L 0 0 0 28" fill="none" stroke="var(--border-structural)" stroke-width="0.45" />
      </pattern>
      <filter id="node-glow" x="-100%" y="-100%" width="300%" height="300%">
        <feGaussianBlur stdDeviation={nodeGlow.stdDeviation} result="glow" />
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
        {@const isRelationship = isRelationshipFolder(node.folder)}
        {@const kindLabel = folderKindLabel(node.folder)}
        {@const shapeLabel = isRelationship ? 'diamond' : 'circle'}
        {@const tooltip =
          `${node.title} · ${node.path} · ${kindLabel} (${shapeLabel}) · Double-click to unpin`}
        <g
          class="node"
          class:selected={isSelected}
          class:muted={isMuted}
          class:pinned={node.fx !== null}
          transform={`translate(${node.x} ${node.y})`}
          role="button"
          tabindex="0"
          aria-label={`${node.title}, ${node.folder} ${kindLabel}, ${nodeDegree(node)} connections${node.fx !== null ? ', pinned' : ''}`}
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
          {#if isRelationship}
            {@const haloSide = diamondSide(nodeHaloRadius(node))}
            {@const coreSide = diamondSide(radius(node))}
            <rect
              class="node-halo"
              x={-haloSide / 2}
              y={-haloSide / 2}
              width={haloSide}
              height={haloSide}
              transform="rotate(45)"
              fill={folderColor(node.folder)}
            />
            <rect
              class="node-core"
              x={-coreSide / 2}
              y={-coreSide / 2}
              width={coreSide}
              height={coreSide}
              transform="rotate(45)"
              fill={folderColor(node.folder)}
            />
          {:else}
            <circle class="node-halo" r={nodeHaloRadius(node)} fill={folderColor(node.folder)} />
            <circle class="node-core" r={radius(node)} fill={folderColor(node.folder)} />
          {/if}
          {#if node.fx !== null}
            {@const pin = pinAxis(radius(node), isRelationship)}
            <circle class="pin-mark" cx={pin} cy={-pin} r="2.5" />
          {/if}
          {#if isSelected || hoveredId === node.id || nodes.length <= 36}
            <text x={radius(node) + 7} y="4">{shortTitle(node.title)}</text>
          {/if}
          <title>{tooltip}</title>
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
    transition: stroke-opacity var(--motion-duration-fast) ease;
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
    transition: opacity var(--motion-duration-fast) ease;
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
    opacity: var(--node-glow-opacity, 0.28);
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
    /* Floating graph-control scrim, not a structural panel surface. */
    background: color-mix(in srgb, var(--bg-panel) 91%, transparent);
    border-radius: var(--radius-md);
    box-shadow: var(--elev-2), var(--edge-lip);
    backdrop-filter: blur(8px);
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

  /* Keep: the paired JS media query also stops the force simulation's RAF loop. */
  @media (prefers-reduced-motion: reduce) {
    .node,
    .edges line {
      transition: none;
    }
  }
</style>
