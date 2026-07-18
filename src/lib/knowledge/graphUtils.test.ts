import { describe, expect, it } from 'vitest';
import {
  FOLDER_COLORS,
  RELATIONSHIP_FOLDERS,
  compareKnowledgeNodes,
  filterKnowledgeGraph,
  folderColor,
  folderKindLabel,
  isRelationshipFolder,
  normalizeKnowledgeGraph,
  nodeDegree,
} from './graphUtils';
import type { KnowledgeGraph, KnowledgeNode } from './types';

const EXPECTED_FOLDERS = [
  'patterns',
  'practices',
  'research',
  'project',
  'clients',
  'partners',
  'vendors',
  'operations',
] as const;

const UNKNOWN_FOLDER_COLOR = folderColor('folder-with-no-mapping');

function node(
  id: string,
  title: string,
  folder: string,
  inDegree: number,
  outDegree: number,
  lastUpdated: string,
): KnowledgeNode {
  return {
    id,
    title,
    folder,
    path: `${folder}/${id}.md`,
    last_updated: lastUpdated,
    in_degree: inDegree,
    out_degree: outDegree,
  };
}

const graph: KnowledgeGraph = {
  nodes: [
    node('alpha', 'Alpha Pattern', 'patterns', 1, 3, '2026-07-01T00:00:00Z'),
    node('beta', 'Beta Practice', 'practices', 2, 0, '2026-07-12T00:00:00Z'),
    node('gamma', 'Gamma Research', 'research', 4, 4, '2026-06-01T00:00:00Z'),
  ],
  edges: [
    { source: 'alpha', target: 'beta', kind: 'related' },
    { source: 'gamma', target: 'alpha', kind: 'wikilink' },
  ],
};

describe('knowledge graph utilities', () => {
  it('normalizes the API boundary and rejects malformed or dangling relationships', () => {
    const normalized = normalizeKnowledgeGraph({
      nodes: [
        graph.nodes[0],
        graph.nodes[1],
        { id: '', title: 'Invalid', folder: 'patterns', path: 'invalid.md' },
      ],
      edges: [
        graph.edges[0],
        { source: 'alpha', target: 'private-client-page', kind: 'cross_ref' },
        { source: 'alpha', target: 'beta', kind: 'unknown' },
      ],
      truncated: true,
    });

    expect(normalized.nodes.map((entry) => entry.id)).toEqual(['alpha', 'beta']);
    expect(normalized.edges).toEqual([graph.edges[0]]);
    expect(normalized.truncated).toBe(true);
  });

  it('keeps the first valid node for each id and validates edges against unique nodes', () => {
    const duplicateAlpha = {
      ...graph.nodes[0],
      title: 'Duplicate Alpha',
      path: 'research/duplicate-alpha.md',
    };
    const normalized = normalizeKnowledgeGraph({
      nodes: [graph.nodes[0], duplicateAlpha, graph.nodes[1]],
      edges: [
        graph.edges[0],
        { source: 'alpha', target: 'missing', kind: 'related' },
      ],
    });

    expect(normalized.nodes).toHaveLength(2);
    expect(normalized.nodes[0]).toEqual(graph.nodes[0]);
    expect(normalized.nodes.map((entry) => entry.id)).toEqual(['alpha', 'beta']);
    expect(normalized.edges).toEqual([graph.edges[0]]);
  });

  it('filters both endpoints together so the visible graph cannot contain dangling edges', () => {
    const filtered = filterKnowledgeGraph(graph, 'alpha', 'all');
    expect(filtered.nodes.map((entry) => entry.id)).toEqual(['alpha']);
    expect(filtered.edges).toEqual([]);

    const folderFiltered = filterKnowledgeGraph(graph, '', 'patterns');
    expect(folderFiltered.nodes.map((entry) => entry.id)).toEqual(['alpha']);
    expect(folderFiltered.edges).toEqual([]);
  });

  it('carries the truncation flag through filtering so the cap notice cannot be filtered away', () => {
    const truncatedGraph: KnowledgeGraph = { ...graph, truncated: true };

    expect(filterKnowledgeGraph(truncatedGraph, '', 'all').truncated).toBe(true);
    expect(filterKnowledgeGraph(truncatedGraph, 'alpha', 'patterns').truncated).toBe(true);
    // A query that matches nothing must still report the corpus as truncated.
    expect(filterKnowledgeGraph(truncatedGraph, 'no-such-page', 'all')).toMatchObject({
      nodes: [],
      edges: [],
      truncated: true,
    });
    expect(filterKnowledgeGraph({ ...graph, truncated: false }, '', 'all').truncated).toBe(false);
    expect(filterKnowledgeGraph(graph, '', 'all').truncated).toBeUndefined();
  });

  it('resolves a distinct color for every supported folder', () => {
    expect(Object.keys(FOLDER_COLORS).sort()).toEqual([...EXPECTED_FOLDERS].sort());

    for (const name of EXPECTED_FOLDERS) {
      expect(folderColor(name)).toBe(FOLDER_COLORS[name]);
      expect(folderColor(name)).not.toBe(UNKNOWN_FOLDER_COLOR);
    }

    expect(folderColor('clients')).not.toBe(UNKNOWN_FOLDER_COLOR);
    expect(folderColor('Clients')).toBe(folderColor('clients'));
    // Every folder must be its own hue — no two share a swatch.
    expect(new Set(Object.values(FOLDER_COLORS)).size).toBe(EXPECTED_FOLDERS.length);
  });

  it('classifies relationship folders apart from operational ones with a text equivalent', () => {
    expect([...RELATIONSHIP_FOLDERS].sort()).toEqual(['clients', 'partners', 'vendors']);
    expect(RELATIONSHIP_FOLDERS.has('operations')).toBe(false);

    for (const name of ['clients', 'partners', 'vendors']) {
      expect(isRelationshipFolder(name)).toBe(true);
      expect(folderKindLabel(name)).toBe('relationship entity');
    }
    for (const name of ['patterns', 'practices', 'research', 'project', 'operations']) {
      expect(isRelationshipFolder(name)).toBe(false);
      expect(folderKindLabel(name)).toBe('operational knowledge');
    }

    expect(isRelationshipFolder(' Clients ')).toBe(true);
  });

  it('sorts all table columns and computes total degree', () => {
    expect(nodeDegree(graph.nodes[0])).toBe(4);
    expect([...graph.nodes].sort((a, b) => compareKnowledgeNodes(a, b, 'title', 'desc'))[0].id)
      .toBe('gamma');
    expect([...graph.nodes].sort((a, b) => compareKnowledgeNodes(a, b, 'folder', 'asc'))[0].id)
      .toBe('alpha');
    expect([...graph.nodes].sort((a, b) => compareKnowledgeNodes(a, b, 'degree', 'desc'))[0].id)
      .toBe('gamma');
    expect([...graph.nodes].sort((a, b) => compareKnowledgeNodes(a, b, 'last_updated', 'desc'))[0].id)
      .toBe('beta');
  });
});
