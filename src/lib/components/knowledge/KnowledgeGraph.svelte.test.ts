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
