import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
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
    fx: 120 as number | null,
    fy: 140 as number | null,
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
let reducedMotionMatches = false;
let motionChangeListener: (() => void) | null = null;
let requestAnimationFrameMock: ReturnType<typeof vi.fn>;
let cancelAnimationFrameMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  forceMocks.createForceSimulation.mockClear();
  forceMocks.simulation.tick.mockClear();
  forceMocks.simulation.setBounds.mockClear();
  forceMocks.simulation.setPinned.mockClear();
  forceMocks.simulation.unpin.mockClear();
  forceMocks.simulation.unpinAll.mockClear();
  forceMocks.pinnedNode.x = 120;
  forceMocks.pinnedNode.y = 140;
  forceMocks.pinnedNode.vx = 0;
  forceMocks.pinnedNode.vy = 0;
  forceMocks.pinnedNode.fx = 120;
  forceMocks.pinnedNode.fy = 140;
  forceMocks.simulation.setPinned.mockImplementation((_id, x, y) => {
    forceMocks.pinnedNode.x = x;
    forceMocks.pinnedNode.y = y;
    forceMocks.pinnedNode.vx = 0;
    forceMocks.pinnedNode.vy = 0;
    forceMocks.pinnedNode.fx = x;
    forceMocks.pinnedNode.fy = y;
  });
  forceMocks.simulation.unpin.mockImplementation(() => {
    forceMocks.pinnedNode.fx = null;
    forceMocks.pinnedNode.fy = null;
  });
  forceMocks.simulation.unpinAll.mockImplementation(() => {
    forceMocks.pinnedNode.fx = null;
    forceMocks.pinnedNode.fy = null;
  });
  hostSize = { width: 760, height: 520 };
  reducedMotionMatches = false;
  motionChangeListener = null;

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
  requestAnimationFrameMock = vi.fn(() => 1);
  cancelAnimationFrameMock = vi.fn();
  vi.stubGlobal('requestAnimationFrame', requestAnimationFrameMock);
  vi.stubGlobal('cancelAnimationFrame', cancelAnimationFrameMock);
  vi.stubGlobal('PointerEvent', class extends MouseEvent {
    readonly pointerId: number;

    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init);
      this.pointerId = init.pointerId ?? 0;
    }
  });
  vi.stubGlobal('matchMedia', vi.fn((media: string) => ({
    get matches() {
      return reducedMotionMatches;
    },
    media,
    onchange: null,
    addEventListener: vi.fn((_type: string, listener: () => void) => {
      motionChangeListener = listener;
    }),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(() => true),
  })));
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

  it('uses a static layout when reduced motion is requested and reacts to preference changes', async () => {
    reducedMotionMatches = true;
    const { container } = render(KnowledgeGraph, {
      props: {
        nodes: [forceMocks.pinnedNode],
        edges: [],
        selectedId: null,
        onSelect: vi.fn(),
      },
    });
    await tick();

    expect(requestAnimationFrameMock).not.toHaveBeenCalled();
    expect(forceMocks.simulation.tick).not.toHaveBeenCalled();
    expect(container.querySelector('.node')?.getAttribute('transform')).toBe('translate(120 140)');

    reducedMotionMatches = false;
    motionChangeListener?.();
    await tick();

    expect(requestAnimationFrameMock).toHaveBeenCalledOnce();
  });

  it('draws relationship folders as diamonds and names the distinction in the accessible label', async () => {
    reducedMotionMatches = true;
    const clientNode = {
      ...forceMocks.pinnedNode,
      id: 'clients/acme',
      title: 'Acme Corp',
      folder: 'clients',
      path: 'clients/acme.md',
      x: 200,
      y: 220,
      fx: null,
      fy: null,
    };
    const patternNode = {
      ...forceMocks.pinnedNode,
      id: 'patterns/alpha',
      title: 'Alpha Pattern',
      folder: 'patterns',
      path: 'patterns/alpha.md',
      x: 90,
      y: 110,
      fx: null,
      fy: null,
    };
    forceMocks.createForceSimulation.mockImplementationOnce(() => ({
      ...forceMocks.simulation,
      nodes: [clientNode, patternNode],
    }));

    const { getByRole } = render(KnowledgeGraph, {
      props: {
        nodes: [clientNode, patternNode],
        edges: [],
        selectedId: null,
        onSelect: vi.fn(),
      },
    });
    await tick();

    const clientMark = getByRole('button', { name: /Acme Corp/ });
    const patternMark = getByRole('button', { name: /Alpha Pattern/ });

    // Shape carries the relationship/operational split...
    expect(clientMark.querySelector('.node-core')?.tagName.toLowerCase()).toBe('rect');
    expect(clientMark.querySelector('.node-core')?.getAttribute('transform')).toBe('rotate(45)');
    expect(patternMark.querySelector('.node-core')?.tagName.toLowerCase()).toBe('circle');
    expect(clientMark.querySelector('.node-halo')?.tagName.toLowerCase()).toBe('rect');
    expect(patternMark.querySelector('.node-halo')?.tagName.toLowerCase()).toBe('circle');

    // ...and it is never the only signal: the accessible name says it too.
    expect(clientMark.getAttribute('aria-label')).toContain('clients relationship entity');
    expect(patternMark.getAttribute('aria-label')).toContain('patterns operational knowledge');
    expect(clientMark.querySelector('title')?.textContent).toContain('relationship entity');
    expect(patternMark.querySelector('title')?.textContent).toContain('operational knowledge');
  });

  it('keeps the pin mark touching the node edge for diamonds as well as circles', async () => {
    reducedMotionMatches = true;
    // Degree must be >= 3: the diamond's pin only detaches once the node grows,
    // so a degree-0 fixture would pass against the unfixed code and prove nothing.
    const degree = { in_degree: 2, out_degree: 2 };
    const radius = Math.min(11, 5 + Math.sqrt(degree.in_degree + degree.out_degree) * 1.25);
    const clientNode = {
      ...forceMocks.pinnedNode,
      ...degree,
      id: 'clients/acme',
      title: 'Acme Corp',
      folder: 'clients',
      path: 'clients/acme.md',
      x: 200,
      y: 220,
      fx: 200 as number | null,
      fy: 220 as number | null,
    };
    const patternNode = {
      ...clientNode,
      id: 'patterns/alpha',
      title: 'Alpha Pattern',
      folder: 'patterns',
      path: 'patterns/alpha.md',
    };
    forceMocks.createForceSimulation.mockImplementationOnce(() => ({
      ...forceMocks.simulation,
      nodes: [clientNode, patternNode],
    }));

    const { getByRole } = render(KnowledgeGraph, {
      props: {
        nodes: [clientNode, patternNode],
        edges: [],
        selectedId: null,
        onSelect: vi.fn(),
      },
    });
    await tick();

    const pinOf = (name: RegExp) => {
      const mark = getByRole('button', { name }).querySelector('.pin-mark');
      expect(mark).not.toBeNull();
      return {
        cx: Number(mark!.getAttribute('cx')),
        cy: Number(mark!.getAttribute('cy')),
      };
    };

    // A diamond's boundary is the L1 circle |x| + |y| = r, so the perpendicular
    // distance of the pin's centre past that edge is (|cx| + |cy| - r) / sqrt(2).
    // Netting off the 2.5px pin radius and the 1px outward half of the core's
    // 2px stroke leaves the visible gap, which must not be positive.
    const diamond = pinOf(/Acme Corp/);
    const diamondGap = (Math.abs(diamond.cx) + Math.abs(diamond.cy) - radius) / Math.SQRT2 - 3.5;
    expect(diamondGap).toBeLessThanOrEqual(0);

    // Circles are untouched by the shape-aware anchor: still the bounding-box
    // corner inset 1px, which sits inside the rim at every degree.
    const circle = pinOf(/Alpha Pattern/);
    expect(circle.cx).toBeCloseTo(radius - 1, 10);
    expect(circle.cy).toBeCloseTo(-(radius - 1), 10);
    expect(Math.hypot(circle.cx, circle.cy) - radius - 3.5).toBeLessThanOrEqual(0);
  });

  it('rolls back a cancelled drag without selecting or retaining a partial pin', async () => {
    reducedMotionMatches = true;
    forceMocks.pinnedNode.vx = 1.5;
    forceMocks.pinnedNode.vy = -0.75;
    forceMocks.pinnedNode.fx = null;
    forceMocks.pinnedNode.fy = null;
    const onSelect = vi.fn();
    const { container, getByRole, getByText } = render(KnowledgeGraph, {
      props: {
        nodes: [forceMocks.pinnedNode],
        edges: [],
        selectedId: null,
        onSelect,
      },
    });
    await tick();

    const node = getByRole('button', { name: /Pinned Pattern/ });
    const svg = container.querySelector('svg');
    expect(svg).not.toBeNull();
    const setPointerCapture = vi.fn();
    const releasePointerCapture = vi.fn();
    Object.assign(node, {
      setPointerCapture,
      hasPointerCapture: vi.fn(() => true),
      releasePointerCapture,
    });
    vi.spyOn(svg!, 'getBoundingClientRect').mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      right: 760,
      bottom: 520,
      left: 0,
      width: 760,
      height: 520,
      toJSON: () => ({}),
    });

    await fireEvent.pointerDown(node, { button: 0, pointerId: 7, clientX: 120, clientY: 140 });
    await fireEvent.pointerMove(node, { pointerId: 7, clientX: 300, clientY: 260 });
    expect(forceMocks.pinnedNode.fx).not.toBeNull();
    expect(forceMocks.pinnedNode.x).not.toBe(120);

    await fireEvent.pointerCancel(node, { pointerId: 7 });
    await tick();

    expect(forceMocks.pinnedNode).toMatchObject({
      x: 120,
      y: 140,
      vx: 1.5,
      vy: -0.75,
      fx: null,
      fy: null,
    });
    expect(onSelect).not.toHaveBeenCalled();
    expect(releasePointerCapture).toHaveBeenCalledOnce();
    expect(getByText('0 pinned')).toBeTruthy();
  });
});
