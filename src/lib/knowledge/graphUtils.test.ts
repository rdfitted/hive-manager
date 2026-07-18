import { describe, expect, it } from 'vitest';
import {
  compareKnowledgeNodes,
  filterKnowledgeGraph,
  normalizeKnowledgeGraph,
  nodeDegree,
} from './graphUtils';
import type { KnowledgeGraph, KnowledgeNode } from './types';

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

  it('filters both endpoints together so the visible graph cannot contain dangling edges', () => {
    const filtered = filterKnowledgeGraph(graph, 'alpha', 'all');
    expect(filtered.nodes.map((entry) => entry.id)).toEqual(['alpha']);
    expect(filtered.edges).toEqual([]);

    const folderFiltered = filterKnowledgeGraph(graph, '', 'patterns');
    expect(folderFiltered.nodes.map((entry) => entry.id)).toEqual(['alpha']);
    expect(folderFiltered.edges).toEqual([]);
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

