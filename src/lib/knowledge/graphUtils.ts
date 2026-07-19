import {
  KNOWLEDGE_EDGE_KINDS,
  type KnowledgeEdge,
  type KnowledgeEdgeKind,
  type KnowledgeGraph,
  type KnowledgeNode,
  type KnowledgeOmission,
  type KnowledgeSortKey,
  type SortDirection,
} from './types';

/**
 * Hues are spread around the wheel so these folders stay separable in the
 * dark UI: cyan 186, green 144, amber 37, silver 209 (low chroma), violet 274,
 * coral 5, lime 81, indigo 227, mint 164. `operations` (227) is the closest
 * neighbour to `.project` (209) by hue, but `.project` is a 29%-saturation neutral
 * while `operations` is fully saturated, so they never read as the same swatch.
 *
 * This map is no longer exhaustive: the backend discovers folders from the wiki
 * root, so any name can arrive. Unlisted folders get a deterministic colour from
 * {@link folderColor} rather than all collapsing onto one fallback swatch.
 */
export const FOLDER_COLORS: Record<string, string> = {
  patterns: '#00e5ff',
  practices: '#00ff66',
  research: '#ff9d00',
  '.project': '#a3b8cc',
  clients: '#c77dff',
  partners: '#ff5f52',
  vendors: '#b6f24a',
  operations: '#6f8dff',
  root: '#8ce8d0',
};

/** Hue in degrees of a `#rrggbb` swatch. */
function hexHue(hex: string): number {
  const red = parseInt(hex.slice(1, 3), 16) / 255;
  const green = parseInt(hex.slice(3, 5), 16) / 255;
  const blue = parseInt(hex.slice(5, 7), 16) / 255;
  const max = Math.max(red, green, blue);
  const delta = max - Math.min(red, green, blue);
  if (delta === 0) return 0;
  let hue: number;
  if (max === red) {
    hue = 60 * (((green - blue) / delta) % 6);
  } else if (max === green) {
    hue = 60 * ((blue - red) / delta + 2);
  } else {
    hue = 60 * ((red - green) / delta + 4);
  }
  return hue < 0 ? hue + 360 : hue;
}

/**
 * Hues reserved by {@link FOLDER_COLORS}, in degrees. Generated colours keep a
 * wide berth from these so a discovered folder never reads as a known one.
 *
 * Derived from the swatches themselves rather than hand-maintained: a
 * transcribed list drifts the moment a swatch is retuned, and even rounding it
 * to whole degrees eats into the separation margin.
 */
const RESERVED_HUES = Object.values(FOLDER_COLORS).map(hexHue);

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

  const omissions = normalizeOmissions(value.omissions);
  return {
    nodes,
    edges,
    // Prefer the structured report when the backend sends one; fall back to the
    // legacy boolean so an older backend still raises the banner.
    truncated: omissions.length > 0 || value.truncated === true,
    omissions,
  };
}

function normalizeOmission(value: unknown): KnowledgeOmission | null {
  if (!isRecord(value)) return null;
  const reason = stringValue(value.reason);
  if (!reason) return null;
  const count =
    typeof value.count === 'number' && Number.isFinite(value.count) && value.count > 0
      ? Math.floor(value.count)
      : 1;
  const examples = Array.isArray(value.examples)
    ? value.examples.filter((example): example is string => typeof example === 'string')
    : [];
  return {
    reason,
    count,
    detail: stringValue(value.detail) ?? reason.replace(/_/g, ' '),
    examples,
  };
}

export function normalizeOmissions(value: unknown): KnowledgeOmission[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is KnowledgeOmission => normalizeOmission(entry) !== null)
    .map((entry) => normalizeOmission(entry) as KnowledgeOmission);
}

/** How many example IDs the banner names inline before collapsing to `+N more`. */
const MAX_SHOWN_EXAMPLES = 3;

/**
 * One banner line per reason, e.g. `3 pages omitted: the file is larger than the
 * read limit (patterns/huge, patterns/other)`. This is the whole point of the
 * structured report: the operator learns what they lost and why, instead of an
 * amber bar that could mean any of a dozen things.
 */
export function describeOmission(omission: KnowledgeOmission): string {
  const head = `${omission.count} ${omission.detail}`;
  if (omission.examples.length === 0) return head;
  const shownExamples = omission.examples.slice(0, MAX_SHOWN_EXAMPLES);
  const shown = shownExamples.join(', ');
  // Subtract what was actually rendered, not how many the backend supplied. It sends up to
  // MAX_OMISSION_EXAMPLES = 5 (knowledge.rs) but only MAX_SHOWN_EXAMPLES are named, so subtracting
  // examples.length made `named + implied` fall short of `count` — and when count === examples
  // .length it dropped the `+N more` suffix entirely, silently discarding two named pages.
  const rest = omission.count - shownExamples.length;
  const more = rest > 0 ? `, +${rest} more` : '';
  return `${head} (${shown}${more})`;
}

export function normalizeKnowledgePage(value: unknown): import('./types').KnowledgePage | null {
  if (!isRecord(value)) return null;
  const id = stringValue(value.id);
  const title = stringValue(value.title);
  const folder = stringValue(value.folder);
  const path = stringValue(value.path);
  if (!id || !title || !folder || !path || typeof value.content !== 'string') return null;

  const omissions = normalizeOmissions(value.omissions);
  return {
    id,
    title,
    folder,
    path,
    content: value.content,
    last_updated: updatedValue(value.last_updated),
    truncated: omissions.length > 0 || value.truncated === true,
    omissions,
  };
}

/** FNV-1a. Small, dependency-free, and well spread for short ASCII-ish keys. */
function hashFolderName(folder: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < folder.length; index += 1) {
    hash ^= folder.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash;
}

/**
 * Stable colour for a folder tag.
 *
 * Known folders keep their curated swatch. Every other folder — the backend now
 * discovers them from the wiki root, so the set is open — gets a colour derived
 * from its name, which makes it (a) stable across reloads, and (b) distinct from
 * its neighbours, instead of every unknown folder sharing one fallback pink.
 *
 * The generated hue is placed in a gap between the reserved hues, and saturation
 * and lightness are pinned to a legible band for the dark UI rather than being
 * hashed too, so no folder can land on an unreadable near-black or a washed-out
 * grey.
 */
export function folderColor(folder: string): string {
  const key = folder.trim().toLowerCase();
  const known = FOLDER_COLORS[key];
  if (known) return known;

  const hash = hashFolderName(key);
  // Walk the hue circle in a coprime step so successive folders land far apart,
  // then nudge away from any reserved hue.
  let hue = (hash * 47) % 360;
  for (let attempt = 0; attempt < RESERVED_HUES.length; attempt += 1) {
    const clash = RESERVED_HUES.find((reserved) => {
      const distance = Math.abs(((hue - reserved + 540) % 360) - 180);
      return distance < 14;
    });
    if (clash === undefined) break;
    hue = (hue + 17) % 360;
  }
  const saturation = 62 + ((hash >>> 8) % 26); // 62–87%
  const lightness = 58 + ((hash >>> 16) % 14); // 58–71%
  return `hsl(${hue} ${saturation}% ${lightness}%)`;
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

/**
 * `folder` is `null` for "no filter". A string sentinel cannot work here: the backend discovers
 * folders from the wiki root, so `all` is a legal directory name and a folder literally named
 * `all` would be unselectable — the dropdown would offer a control that silently did nothing.
 */
export function filterKnowledgeGraph(
  graph: KnowledgeGraph,
  query: string,
  folder: string | null,
): KnowledgeGraph {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const nodes = graph.nodes.filter((node) => {
    if (folder !== null && node.folder !== folder) return false;
    if (!normalizedQuery) return true;
    return [node.title, node.path, node.folder].some((value) =>
      value.toLocaleLowerCase().includes(normalizedQuery),
    );
  });
  const visibleIds = new Set(nodes.map((node) => node.id));
  const edges = graph.edges.filter(
    (edge) => visibleIds.has(edge.source) && visibleIds.has(edge.target),
  );
  // Omissions describe the corpus scan, not the client-side filter, so they pass
  // through untouched — filtering to one folder must not imply pages were dropped.
  return { nodes, edges, truncated: graph.truncated, omissions: graph.omissions };
}
