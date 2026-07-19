export const KNOWLEDGE_EDGE_KINDS = [
  'cross_ref',
  'wikilink',
  'global',
  'related',
  'from',
] as const;

export type KnowledgeEdgeKind = (typeof KNOWLEDGE_EDGE_KINDS)[number];

export interface KnowledgeNode {
  id: string;
  title: string;
  folder: string;
  path: string;
  last_updated: string | number | null;
  in_degree: number;
  out_degree: number;
}

export interface KnowledgeEdge {
  source: string;
  target: string;
  kind: KnowledgeEdgeKind;
}

/**
 * Why something the operator might have expected is missing. The backend replaced a single
 * `truncated: boolean` — which was assigned in a dozen unrelated places and so meant nothing
 * actionable — with a countable list of these.
 */
export interface KnowledgeOmission {
  reason: string;
  count: number;
  /** One sentence, safe to render verbatim. */
  detail: string;
  /** Up to a handful of affected page IDs. Never absolute filesystem paths. */
  examples: string[];
}

export interface KnowledgeGraph {
  nodes: KnowledgeNode[];
  edges: KnowledgeEdge[];
  /** Derived from `omissions` being non-empty; kept for backward compatibility. */
  truncated?: boolean;
  omissions?: KnowledgeOmission[];
}

export interface KnowledgePage {
  id: string;
  title: string;
  folder: string;
  path: string;
  content: string;
  last_updated: string | number | null;
  truncated?: boolean;
  omissions?: KnowledgeOmission[];
}

export type KnowledgeView = 'graph' | 'table';
export type KnowledgeSortKey = 'title' | 'folder' | 'degree' | 'last_updated';
export type SortDirection = 'asc' | 'desc';
