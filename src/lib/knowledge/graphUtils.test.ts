import { describe, expect, it } from 'vitest';
import {
  FOLDER_COLORS,
  RELATIONSHIP_FOLDERS,
  compareKnowledgeNodes,
  describeOmission,
  filterKnowledgeGraph,
  folderColor,
  folderKindLabel,
  isRelationshipFolder,
  normalizeKnowledgeGraph,
  normalizeOmissions,
  nodeDegree,
} from './graphUtils';
import type { KnowledgeGraph, KnowledgeNode } from './types';

/** The folders that keep a hand-picked swatch. The set of folders is otherwise open. */
const CURATED_FOLDERS = [
  'patterns',
  'practices',
  'research',
  '.project',
  'clients',
  'partners',
  'vendors',
  'operations',
  'root',
] as const;

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
    const filtered = filterKnowledgeGraph(graph, 'alpha', null);
    expect(filtered.nodes.map((entry) => entry.id)).toEqual(['alpha']);
    expect(filtered.edges).toEqual([]);

    const folderFiltered = filterKnowledgeGraph(graph, '', 'patterns');
    expect(folderFiltered.nodes.map((entry) => entry.id)).toEqual(['alpha']);
    expect(folderFiltered.edges).toEqual([]);
  });

  it('carries the truncation flag through filtering so the cap notice cannot be filtered away', () => {
    const truncatedGraph: KnowledgeGraph = { ...graph, truncated: true };

    expect(filterKnowledgeGraph(truncatedGraph, '', null).truncated).toBe(true);
    expect(filterKnowledgeGraph(truncatedGraph, 'alpha', 'patterns').truncated).toBe(true);
    // A query that matches nothing must still report the corpus as truncated.
    expect(filterKnowledgeGraph(truncatedGraph, 'no-such-page', null)).toMatchObject({
      nodes: [],
      edges: [],
      truncated: true,
    });
    expect(filterKnowledgeGraph({ ...graph, truncated: false }, '', null).truncated).toBe(false);
    expect(filterKnowledgeGraph(graph, '', null).truncated).toBeUndefined();
  });

  it('resolves a distinct color for every curated folder', () => {
    expect(Object.keys(FOLDER_COLORS).sort()).toEqual([...CURATED_FOLDERS].sort());

    for (const name of CURATED_FOLDERS) {
      expect(folderColor(name)).toBe(FOLDER_COLORS[name]);
    }
    expect(folderColor('Clients')).toBe(folderColor('clients'));
    // Every curated folder must be its own swatch — no two share one.
    expect(new Set(Object.values(FOLDER_COLORS)).size).toBe(CURATED_FOLDERS.length);
  });

  it('gives discovered folders stable, distinguishable colors instead of one shared fallback', () => {
    // The backend now discovers folders from the wiki root, so these names are
    // representative of what actually shows up, not hypothetical.
    const discovered = [
      'agents',
      'zettelkasten',
      'Field Notes',
      'meta',
      'archive',
      'inbox',
      'people',
      'projects-2026',
    ];
    const colors = discovered.map(folderColor);

    // Every one is a real color, and no two collapse onto the same swatch — the
    // old behavior gave all of these one identical fallback pink.
    for (const color of colors) {
      expect(color).toMatch(/^hsl\(/);
    }
    expect(new Set(colors).size).toBe(discovered.length);

    // Stable across calls and case-insensitive, so a reload does not reshuffle
    // the legend.
    expect(folderColor('agents')).toBe(folderColor('agents'));
    expect(folderColor('AGENTS')).toBe(folderColor(' agents '));

    // ...and none of them is byte-identical to a curated swatch. This alone is a
    // weak check — curated swatches are hex and generated ones are hsl(), so it
    // can never fire. The hue-separation test below is the real guard.
    for (const color of colors) {
      expect(Object.values(FOLDER_COLORS)).not.toContain(color);
    }
  });

  it('keeps every generated folder color a readable distance from the curated hues', () => {
    // Independent oracle: derive the curated hues from the swatches themselves
    // rather than trusting the module's own reserved-hue list, so a drifted list
    // fails here instead of silently guarding the wrong point on the wheel.
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
    function separation(left: number, right: number): number {
      const raw = Math.abs(left - right) % 360;
      return raw > 180 ? 360 - raw : raw;
    }

    // Ordinary wiki folder names, not adversarial ones — several of these landed
    // within 6 degrees of a curated swatch before the reserved-hue guard was fixed.
    const discovered = [
      'archive',
      'ideas',
      'zettelkasten',
      'books',
      'journal',
      'travel',
      'work',
      'reading',
      'school',
      'legal',
      'agents',
      'meta',
      'inbox',
      'people',
      'projects-2026',
      'Field Notes',
    ];

    for (const name of discovered) {
      const color = folderColor(name);
      const match = /^hsl\((\d+(?:\.\d+)?) /.exec(color);
      expect(match, `${name} produced an unparseable color: ${color}`).not.toBeNull();
      const hue = Number(match?.[1]);

      for (const [curated, hex] of Object.entries(FOLDER_COLORS)) {
        expect(
          separation(hue, hexHue(hex)),
          `folder "${name}" -> ${color} reads as curated "${curated}" (${hex})`,
        ).toBeGreaterThanOrEqual(14);
      }
    }
  });

  it('normalizes the structured omission report and derives truncated from it', () => {
    const normalized = normalizeKnowledgeGraph({
      nodes: [],
      edges: [],
      omissions: [
        {
          reason: 'file_too_large',
          count: 3,
          detail: 'pages omitted: the file is larger than the read limit',
          examples: ['patterns/huge', 'agents/bigger'],
        },
        { reason: 'edge_cap_reached' },
        // Malformed entries are dropped rather than rendered.
        { count: 4 },
        'not-an-object',
      ],
    });

    expect(normalized.omissions).toEqual([
      {
        reason: 'file_too_large',
        count: 3,
        detail: 'pages omitted: the file is larger than the read limit',
        examples: ['patterns/huge', 'agents/bigger'],
      },
      { reason: 'edge_cap_reached', count: 1, detail: 'edge cap reached', examples: [] },
    ]);
    // `truncated` is derived, so a backend that only sends omissions still raises the banner.
    expect(normalized.truncated).toBe(true);

    const clean = normalizeKnowledgeGraph({ nodes: [], edges: [], omissions: [] });
    expect(clean.truncated).toBe(false);
    // An older backend sending only the boolean is still honored.
    expect(normalizeKnowledgeGraph({ nodes: [], edges: [], truncated: true }).truncated).toBe(true);
  });

  it('describes each omission with its count, reason, and examples', () => {
    expect(
      describeOmission({
        reason: 'file_too_large',
        count: 3,
        detail: 'pages omitted: the file is larger than the read limit',
        examples: ['patterns/huge', 'agents/bigger'],
      }),
    ).toBe(
      '3 pages omitted: the file is larger than the read limit (patterns/huge, agents/bigger, +1 more)',
    );

    expect(
      describeOmission({
        reason: 'edge_cap_reached',
        count: 12,
        detail: 'links omitted: the link cap was reached (all pages are shown)',
        examples: [],
      }),
    ).toBe('12 links omitted: the link cap was reached (all pages are shown)');
  });

  it('accounts for every omitted item when the backend supplies more examples than are shown', () => {
    // The backend sends up to MAX_OMISSION_EXAMPLES = 5 but only 3 are named inline. The
    // remainder used to be computed against examples.length, so `named + implied` fell short of
    // `count` — the one arithmetic the banner exists to get right.
    expect(
      describeOmission({
        reason: 'node_cap_reached',
        count: 8,
        detail: 'pages omitted: the node cap for this request was reached',
        examples: [
          'patterns/page-2',
          'patterns/page-3',
          'patterns/page-4',
          'patterns/page-5',
          'patterns/page-6',
        ],
      }),
    ).toBe(
      '8 pages omitted: the node cap for this request was reached (patterns/page-2, patterns/page-3, patterns/page-4, +5 more)',
    );

    // count === examples.length: the suffix must NOT disappear, or two supplied IDs vanish with
    // no indicator at all from a line that reads as complete.
    expect(
      describeOmission({
        reason: 'node_cap_reached',
        count: 5,
        detail: 'pages omitted',
        examples: ['a', 'b', 'c', 'd', 'e'],
      }),
    ).toBe('5 pages omitted (a, b, c, +2 more)');

    // The shape the backend's own cap test produces: 400 omitted, 5 examples.
    expect(
      describeOmission({
        reason: 'node_cap_reached',
        count: 400,
        detail: 'pages omitted: the node cap for this request was reached',
        examples: ['p/000', 'p/001', 'p/002', 'p/003', 'p/004'],
      }),
    ).toBe(
      '400 pages omitted: the node cap for this request was reached (p/000, p/001, p/002, +397 more)',
    );
  });

  it('lets a folder literally named "all" be filtered in isolation', () => {
    // The folder set is discovered from the wiki root, so `all` is a legal directory name. While
    // the no-filter sentinel was the string 'all', selecting that folder returned the whole graph
    // — a dropdown entry that silently did nothing.
    const allFolderGraph: KnowledgeGraph = {
      nodes: [
        node('a1', 'Inbox One', 'all', 0, 0, '2026-07-01T00:00:00Z'),
        node('a2', 'Inbox Two', 'all', 0, 0, '2026-07-02T00:00:00Z'),
        node('p1', 'Pattern', 'patterns', 0, 0, '2026-07-03T00:00:00Z'),
      ],
      edges: [],
    };

    expect(filterKnowledgeGraph(allFolderGraph, '', 'all').nodes.map((n) => n.id)).toEqual([
      'a1',
      'a2',
    ]);
    expect(filterKnowledgeGraph(allFolderGraph, '', null).nodes).toHaveLength(3);
  });

  it('passes omissions through filtering so a folder filter cannot imply pages were dropped', () => {
    const omissions = normalizeOmissions([
      { reason: 'node_cap_reached', count: 2, detail: 'pages omitted', examples: ['a'] },
    ]);
    const withOmissions: KnowledgeGraph = { ...graph, truncated: true, omissions };

    expect(filterKnowledgeGraph(withOmissions, 'alpha', 'patterns').omissions).toEqual(omissions);
    expect(filterKnowledgeGraph(withOmissions, 'no-such-page', null).omissions).toEqual(omissions);
  });

  it('classifies relationship folders apart from operational ones with a text equivalent', () => {
    expect([...RELATIONSHIP_FOLDERS].sort()).toEqual(['clients', 'partners', 'vendors']);
    expect(RELATIONSHIP_FOLDERS.has('operations')).toBe(false);

    for (const name of ['clients', 'partners', 'vendors']) {
      expect(isRelationshipFolder(name)).toBe(true);
      expect(folderKindLabel(name)).toBe('relationship entity');
    }
    for (const name of ['patterns', 'practices', 'research', '.project', 'operations']) {
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
