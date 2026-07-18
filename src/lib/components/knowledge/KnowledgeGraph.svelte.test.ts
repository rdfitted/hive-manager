import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import { tick } from 'svelte';

const forceMocks = vi.hoisted(() => {
  const pinnedNode = {
    id: 'patterns/pinned',
    title: 'Pinned Pattern',
    folder: 'patterns',
    path: 'patterns/pinned.md',
    last_updated: null,
    in_degree: 0,
    out_degree: 0,
    x: 120,
    y: 140,
    vx: 0,
    vy: 0,
    fx: 120,
    fy: 140,
  };
  const simulation = {
    nodes: [pinnedNode],
    alpha: 0,
    tick: vi.fn(() => 0),
    reheat: vi.fn(),
    setBounds: vi.fn(),
    setPinned: vi.fn(),
    unpin: vi.fn(),
    unpinAll: vi.fn(),
  };
  return {
    pinnedNode,
    simulation,
    createForceSimulation: vi.fn(() => simulation),
  };
});

vi.mock('$lib/knowledge/forceSim', () => ({
  createForceSimulation: forceMocks.createForceSimulation,
}));

import KnowledgeGraph from './KnowledgeGraph.svelte';

let resizeCallback: ResizeObserverCallback;
let hostSize = { width: 760, height: 520 };

beforeEach(() => {
  forceMocks.createForceSimulation.mockClear();
  forceMocks.simulation.setBounds.mockClear();
  forceMocks.pinnedNode.fx = 120;
  forceMocks.pinnedNode.fy = 140;
  hostSize = { width: 760, height: 520 };

  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(() => ({
    x: 0,
    y: 0,
    top: 0,
    right: hostSize.width,
    bottom: hostSize.height,
    left: 0,
    width: hostSize.width,
    height: hostSize.height,
    toJSON: () => ({}),
  }));
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 1));
  vi.stubGlobal('cancelAnimationFrame', vi.fn());
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: ResizeObserverCallback) {
      resizeCallback = callback;
    }
    observe() {}
    unobserve() {}
    disconnect() {}
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('KnowledgeGraph', () => {
  it('updates simulation bounds without recreating the pinned layout on resize', async () => {
    const { getByText } = render(KnowledgeGraph, {
      props: {
        nodes: [forceMocks.pinnedNode],
        edges: [],
        selectedId: null,
        onSelect: vi.fn(),
      },
    });
    await tick();

    expect(forceMocks.createForceSimulation).toHaveBeenCalledOnce();
    expect(forceMocks.simulation.setBounds).toHaveBeenLastCalledWith(760, 520);
    expect(getByText('1 pinned')).toBeTruthy();

    hostSize = { width: 480, height: 360 };
    resizeCallback([], {} as ResizeObserver);
    await tick();

    expect(forceMocks.createForceSimulation).toHaveBeenCalledOnce();
    expect(forceMocks.simulation.setBounds).toHaveBeenLastCalledWith(480, 360);
    expect(forceMocks.pinnedNode.fx).toBe(120);
    expect(forceMocks.pinnedNode.fy).toBe(140);
    expect(getByText('1 pinned')).toBeTruthy();
  });
});
