import {
  KNOWLEDGE_EDGE_KINDS,
  type KnowledgeEdge,
  type KnowledgeEdgeKind,
  type KnowledgeGraph,
  type KnowledgeNode,
  type KnowledgeSortKey,
  type SortDirection,
} from './types';

/**
 * Hues are spread around the wheel so all eight folders stay separable in the
 * dark UI: cyan 186, green 144, amber 37, silver 212 (low chroma), violet 274,
 * coral 5, lime 81, indigo 227. `operations` (227) is the closest neighbour to
 * `project` (212) by hue, but `project` is a 27%-saturation neutral while
 * `operations` is fully saturated, so they never read as the same swatch.
 */
export const FOLDER_COLORS: Record<string, string> = {
  patterns: '#00e5ff',
  practices: '#00ff66',
  research: '#ff9d00',
  project: '#a3b8cc',
  clients: '#c77dff',
  partners: '#ff5f52',
  vendors: '#b6f24a',
  operations: '#6f8dff',
};

/**
 * Relationship entities — *who* we work with. Everything else in
 * {@link FOLDER_COLORS} is operational knowledge (*how* we work). The graph
 * double-codes this split as shape + accessible text, never colour alone.
 */
export const RELATIONSHIP_FOLDERS: ReadonlySet<string> = new Set([
  'clients',
  'partners',
  'vendors',
]);

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

  const nodes: KnowledgeNode[] = [];
  const nodeIds = new Set<string>();
  if (Array.isArray(value.nodes)) {
    for (const valueNode of value.nodes) {
      const normalizedNode = normalizeNode(valueNode);
      if (normalizedNode === null || nodeIds.has(normalizedNode.id)) continue;
      nodeIds.add(normalizedNode.id);
      nodes.push(normalizedNode);
    }
  }
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

/** Case-folded so a folder string from the API cannot slip past the set. */
export function isRelationshipFolder(folder: string): boolean {
  return RELATIONSHIP_FOLDERS.has(folder.trim().toLowerCase());
}

/**
 * Text equivalent of the node-shape encoding, for aria-labels and tooltips.
 * Shape alone is not an accessible signal.
 */
export function folderKindLabel(folder: string): string {
  return isRelationshipFolder(folder) ? 'relationship entity' : 'operational knowledge';
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
