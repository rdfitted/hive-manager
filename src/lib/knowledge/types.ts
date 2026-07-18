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

export interface KnowledgeGraph {
  nodes: KnowledgeNode[];
  edges: KnowledgeEdge[];
  truncated?: boolean;
}

export interface KnowledgePage {
  id: string;
  title: string;
  folder: string;
  path: string;
  content: string;
  last_updated: string | number | null;
  truncated?: boolean;
}

export type KnowledgeView = 'graph' | 'table';
export type KnowledgeSortKey = 'title' | 'folder' | 'degree' | 'last_updated';
export type SortDirection = 'asc' | 'desc';
