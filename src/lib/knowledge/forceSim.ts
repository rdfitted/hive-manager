import type { KnowledgeEdge, KnowledgeNode } from './types';

export interface ForceNode extends KnowledgeNode {
  x: number;
  y: number;
  vx: number;
  vy: number;
  fx: number | null;
  fy: number | null;
}

export interface ForceSimulation {
  readonly nodes: ForceNode[];
  readonly alpha: number;
  tick: () => number;
  reheat: (value?: number) => void;
  setBounds: (width: number, height: number) => void;
  setPinned: (id: string, x: number, y: number) => void;
  unpin: (id: string) => void;
  unpinAll: () => void;
}

function hashString(value: string): number {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function initialPosition(id: string, index: number, count: number, width: number, height: number) {
  const hash = hashString(id);
  const angle = (index / Math.max(count, 1)) * Math.PI * 2 + ((hash & 255) / 255) * 0.45;
  const ring = 0.18 + (((hash >>> 8) & 255) / 255) * 0.24;
  const radius = Math.min(width, height) * ring;
  return {
    x: width / 2 + Math.cos(angle) * radius,
    y: height / 2 + Math.sin(angle) * radius,
  };
}

/** Dependency-free, bounded force simulation for the knowledge graph. */
export function createForceSimulation(
  sourceNodes: KnowledgeNode[],
  edges: KnowledgeEdge[],
  initialWidth: number,
  initialHeight: number,
): ForceSimulation {
  let width = Math.max(initialWidth, 240);
  let height = Math.max(initialHeight, 240);
  let currentAlpha = 1;

  const nodes: ForceNode[] = sourceNodes.map((node, index) => ({
    ...node,
    ...initialPosition(node.id, index, sourceNodes.length, width, height),
    vx: 0,
    vy: 0,
    fx: null,
    fy: null,
  }));
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const linked = edges
    .map((edge) => {
      const source = byId.get(edge.source);
      const target = byId.get(edge.target);
      return source && target ? { source, target } : null;
    })
    .filter((edge): edge is { source: ForceNode; target: ForceNode } => edge !== null);

  function tick(): number {
    if (nodes.length === 0) {
      currentAlpha = 0;
      return currentAlpha;
    }

    const centerX = width / 2;
    const centerY = height / 2;

    // Pairwise charge. The graph is capped at 400 nodes by the backend, keeping
    // this deterministic O(n²) pass comfortably bounded and dependency-free.
    for (let leftIndex = 0; leftIndex < nodes.length; leftIndex += 1) {
      const left = nodes[leftIndex];
      for (let rightIndex = leftIndex + 1; rightIndex < nodes.length; rightIndex += 1) {
        const right = nodes[rightIndex];
        let dx = right.x - left.x;
        let dy = right.y - left.y;
        let distanceSquared = dx * dx + dy * dy;
        if (distanceSquared < 1) {
          dx = ((hashString(`${left.id}:${right.id}`) & 1) || -1) * 0.75;
          dy = 0.75;
          distanceSquared = dx * dx + dy * dy;
        }
        const distance = Math.sqrt(distanceSquared);
        const force = (720 * currentAlpha) / Math.max(distanceSquared, 36);
        const forceX = (dx / distance) * force;
        const forceY = (dy / distance) * force;
        left.vx -= forceX;
        left.vy -= forceY;
        right.vx += forceX;
        right.vy += forceY;
      }
    }

    for (const link of linked) {
      const dx = link.target.x - link.source.x;
      const dy = link.target.y - link.source.y;
      const distance = Math.max(Math.sqrt(dx * dx + dy * dy), 1);
      const desiredDistance = 72;
      const force = (distance - desiredDistance) * 0.045 * currentAlpha;
      const forceX = (dx / distance) * force;
      const forceY = (dy / distance) * force;
      link.source.vx += forceX;
      link.source.vy += forceY;
      link.target.vx -= forceX;
      link.target.vy -= forceY;
    }

    for (const node of nodes) {
      if (node.fx !== null && node.fy !== null) {
        node.x = node.fx;
        node.y = node.fy;
        node.vx = 0;
        node.vy = 0;
        continue;
      }

      node.vx += (centerX - node.x) * 0.006 * currentAlpha;
      node.vy += (centerY - node.y) * 0.006 * currentAlpha;
      node.vx *= 0.84;
      node.vy *= 0.84;
      node.x += node.vx;
      node.y += node.vy;

      const padding = 28;
      if (node.x < padding) {
        node.x = padding;
        node.vx *= -0.25;
      } else if (node.x > width - padding) {
        node.x = width - padding;
        node.vx *= -0.25;
      }
      if (node.y < padding) {
        node.y = padding;
        node.vy *= -0.25;
      } else if (node.y > height - padding) {
        node.y = height - padding;
        node.vy *= -0.25;
      }
    }

    currentAlpha = Math.max(0, currentAlpha * 0.972 - 0.0015);
    return currentAlpha;
  }

  return {
    nodes,
    get alpha() {
      return currentAlpha;
    },
    tick,
    reheat(value = 0.45) {
      currentAlpha = Math.max(currentAlpha, value);
    },
    setBounds(nextWidth, nextHeight) {
      width = Math.max(nextWidth, 240);
      height = Math.max(nextHeight, 240);
      for (const node of nodes) {
        if (node.fx !== null && node.fy !== null) {
          node.fx = Math.min(Math.max(node.fx, 18), width - 18);
          node.fy = Math.min(Math.max(node.fy, 18), height - 18);
          node.x = node.fx;
          node.y = node.fy;
        } else {
          node.x = Math.min(Math.max(node.x, 28), width - 28);
          node.y = Math.min(Math.max(node.y, 28), height - 28);
        }
      }
      currentAlpha = Math.max(currentAlpha, 0.25);
    },
    setPinned(id, x, y) {
      const node = byId.get(id);
      if (!node) return;
      node.fx = Math.min(Math.max(x, 18), width - 18);
      node.fy = Math.min(Math.max(y, 18), height - 18);
      node.x = node.fx;
      node.y = node.fy;
      node.vx = 0;
      node.vy = 0;
      currentAlpha = Math.max(currentAlpha, 0.35);
    },
    unpin(id) {
      const node = byId.get(id);
      if (!node) return;
      node.fx = null;
      node.fy = null;
      currentAlpha = Math.max(currentAlpha, 0.25);
    },
    unpinAll() {
      for (const node of nodes) {
        node.fx = null;
        node.fy = null;
      }
      currentAlpha = Math.max(currentAlpha, 0.5);
    },
  };
}
