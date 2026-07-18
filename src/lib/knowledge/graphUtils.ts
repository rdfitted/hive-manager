import {
  KNOWLEDGE_EDGE_KINDS,
  type KnowledgeEdge,
  type KnowledgeEdgeKind,
  type KnowledgeGraph,
  type KnowledgeNode,
  type KnowledgeSortKey,
  type SortDirection,
} from './types';

export const FOLDER_COLORS: Record<string, string> = {
  patterns: '#00e5ff',
  practices: '#00ff66',
  research: '#ff9d00',
  project: '#a3b8cc',
};

export const EDGE_COLORS: Record<KnowledgeEdgeKind, string> = {
  cross_ref: '#00e5ff',
  wikilink: '#8b949e',
  global: '#ff9d00',
  related: '#00ff66',
  from: '#a3b8cc',
};

export const EDGE_LABELS: Record<KnowledgeEdgeKind, string> = {
  cross_ref: 'Cross reference',
  wikilink: 'Wikilink',
  global: 'Global',
  related: 'Related',
  from: 'Provenance',
};

const DEFAULT_FOLDER_COLOR = '#ff6bcb';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function degreeValue(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : 0;
}

function updatedValue(value: unknown): string | number | null {
  if (typeof value === 'string' || (typeof value === 'number' && Number.isFinite(value))) {
    return value;
  }
  return null;
}

function normalizeNode(value: unknown): KnowledgeNode | null {
  if (!isRecord(value)) return null;
  const id = stringValue(value.id);
  const title = stringValue(value.title);
  const folder = stringValue(value.folder);
  const path = stringValue(value.path);
  if (!id || !title || !folder || !path) return null;

  return {
    id,
    title,
    folder,
    path,
    last_updated: updatedValue(value.last_updated),
    in_degree: degreeValue(value.in_degree),
    out_degree: degreeValue(value.out_degree),
  };
}

function normalizeEdge(value: unknown): KnowledgeEdge | null {
  if (!isRecord(value)) return null;
  const source = stringValue(value.source);
  const target = stringValue(value.target);
  const kind = stringValue(value.kind);
  if (
    !source ||
    !target ||
    !kind ||
    !KNOWLEDGE_EDGE_KINDS.includes(kind as KnowledgeEdgeKind)
  ) {
    return null;
  }

  return { source, target, kind: kind as KnowledgeEdgeKind };
}

/**
 * Treat the backend as a trust boundary. Invalid records and dangling edges are
 * discarded so the graph renderer never receives an unusable endpoint.
 */
export function normalizeKnowledgeGraph(value: unknown): KnowledgeGraph {
  if (!isRecord(value)) return { nodes: [], edges: [] };

  const nodes = Array.isArray(value.nodes)
    ? value.nodes.map(normalizeNode).filter((node): node is KnowledgeNode => node !== null)
    : [];
  const nodeIds = new Set(nodes.map((node) => node.id));
  const edges = Array.isArray(value.edges)
    ? value.edges
        .map(normalizeEdge)
        .filter(
          (edge): edge is KnowledgeEdge =>
            edge !== null && nodeIds.has(edge.source) && nodeIds.has(edge.target),
        )
    : [];

  return { nodes, edges, truncated: value.truncated === true };
}

export function normalizeKnowledgePage(value: unknown): import('./types').KnowledgePage | null {
  if (!isRecord(value)) return null;
  const id = stringValue(value.id);
  const title = stringValue(value.title);
  const folder = stringValue(value.folder);
  const path = stringValue(value.path);
  if (!id || !title || !folder || !path || typeof value.content !== 'string') return null;

  return {
    id,
    title,
    folder,
    path,
    content: value.content,
    last_updated: updatedValue(value.last_updated),
    truncated: value.truncated === true,
  };
}

export function folderColor(folder: string): string {
  return FOLDER_COLORS[folder.toLowerCase()] ?? DEFAULT_FOLDER_COLOR;
}

export function nodeDegree(node: KnowledgeNode): number {
  return node.in_degree + node.out_degree;
}

export function timestampValue(value: string | number | null): number {
  if (value === null) return 0;
  if (typeof value === 'number') {
    // Rust timestamps may be seconds or milliseconds since the epoch.
    return value < 10_000_000_000 ? value * 1000 : value;
  }
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

export function formatLastUpdated(value: string | number | null): string {
  const timestamp = timestampValue(value);
  if (timestamp === 0) return 'Unknown';
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  }).format(new Date(timestamp));
}

export function compareKnowledgeNodes(
  left: KnowledgeNode,
  right: KnowledgeNode,
  key: KnowledgeSortKey,
  direction: SortDirection,
): number {
  let comparison = 0;
  if (key === 'degree') {
    comparison = nodeDegree(left) - nodeDegree(right);
  } else if (key === 'last_updated') {
    comparison = timestampValue(left.last_updated) - timestampValue(right.last_updated);
  } else {
    comparison = left[key].localeCompare(right[key], undefined, { sensitivity: 'base' });
  }

  if (comparison === 0) {
    comparison = left.title.localeCompare(right.title, undefined, { sensitivity: 'base' });
  }
  return direction === 'asc' ? comparison : -comparison;
}

export function filterKnowledgeGraph(
  graph: KnowledgeGraph,
  query: string,
  folder: string,
): KnowledgeGraph {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const nodes = graph.nodes.filter((node) => {
    if (folder !== 'all' && node.folder !== folder) return false;
    if (!normalizedQuery) return true;
    return [node.title, node.path, node.folder].some((value) =>
      value.toLocaleLowerCase().includes(normalizedQuery),
    );
  });
  const visibleIds = new Set(nodes.map((node) => node.id));
  const edges = graph.edges.filter(
    (edge) => visibleIds.has(edge.source) && visibleIds.has(edge.target),
  );
  return { nodes, edges, truncated: graph.truncated };
}
