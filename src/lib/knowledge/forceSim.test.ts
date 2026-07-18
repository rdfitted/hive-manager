import { describe, expect, it } from 'vitest';
import { createForceSimulation } from './forceSim';
import type { KnowledgeEdge, KnowledgeNode } from './types';

function node(id: string): KnowledgeNode {
  return {
    id,
    title: id,
    folder: 'patterns',
    path: `patterns/${id}.md`,
    last_updated: null,
    in_degree: 1,
    out_degree: 1,
  };
}

const nodes = [node('alpha'), node('beta'), node('gamma')];
const edges: KnowledgeEdge[] = [
  { source: 'alpha', target: 'beta', kind: 'related' },
  { source: 'beta', target: 'gamma', kind: 'wikilink' },
];

describe('dependency-free knowledge force simulation', () => {
  it('settles deterministically to finite positions inside the canvas', () => {
    const first = createForceSimulation(nodes, edges, 800, 500);
    const second = createForceSimulation(nodes, edges, 800, 500);

    for (let index = 0; index < 240; index += 1) {
      first.tick();
      second.tick();
    }

    expect(first.alpha).toBeLessThan(0.01);
    for (let index = 0; index < first.nodes.length; index += 1) {
      const current = first.nodes[index];
      expect(Number.isFinite(current.x)).toBe(true);
      expect(Number.isFinite(current.y)).toBe(true);
      expect(current.x).toBeGreaterThanOrEqual(28);
      expect(current.x).toBeLessThanOrEqual(772);
      expect(current.y).toBeGreaterThanOrEqual(28);
      expect(current.y).toBeLessThanOrEqual(472);
      expect(current.x).toBeCloseTo(second.nodes[index].x, 8);
      expect(current.y).toBeCloseTo(second.nodes[index].y, 8);
    }
  });

  it('pins, moves, and releases nodes while reheating the layout', () => {
    const simulation = createForceSimulation(nodes, edges, 640, 480);
    simulation.setPinned('beta', 120, 140);
    for (let index = 0; index < 20; index += 1) simulation.tick();

    const beta = simulation.nodes.find((entry) => entry.id === 'beta');
    expect(beta?.x).toBe(120);
    expect(beta?.y).toBe(140);
    expect(beta?.fx).toBe(120);

    simulation.setPinned('beta', 190, 210);
    simulation.tick();
    expect(beta?.x).toBe(190);
    expect(beta?.y).toBe(210);

    simulation.unpin('beta');
    expect(beta?.fx).toBeNull();
    expect(beta?.fy).toBeNull();
    expect(simulation.alpha).toBeGreaterThan(0);
  });

  it('preserves and clamps pinned state when the canvas bounds change', () => {
    const simulation = createForceSimulation(nodes, edges, 800, 500);
    simulation.setPinned('beta', 700, 420);

    simulation.setBounds(320, 260);

    const beta = simulation.nodes.find((entry) => entry.id === 'beta');
    expect(beta?.fx).toBe(302);
    expect(beta?.fy).toBe(242);
    expect(beta?.x).toBe(302);
    expect(beta?.y).toBe(242);
    expect(simulation.alpha).toBeGreaterThan(0);
  });
});
