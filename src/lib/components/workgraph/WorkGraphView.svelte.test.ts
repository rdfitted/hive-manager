import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { BindingRef, NodeStatus, WorkGraphNode } from '$lib/workgraph/types';

const storeMocks = vi.hoisted(() => ({
  setActiveSession: undefined as ((session: { id: string } | null) => void) | undefined,
}));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  const activeSession = writable<{ id: string } | null>({ id: 'session-a' });
  storeMocks.setActiveSession = activeSession.set;
  return { activeSession };
});

vi.mock('$lib/config', () => ({
  apiUrl: (path: string) => `http://localhost:18800${path}`,
}));

import WorkGraphView from './WorkGraphView.svelte';
import workGraphViewSource from './WorkGraphView.svelte?raw';

const ROLE = (value: string) => ({ kind: 'role' as const, value });

function graphNode(
  id: string,
  status: NodeStatus,
  lane: BindingRef,
  overrides: Partial<WorkGraphNode> = {}
): WorkGraphNode {
  return {
    id,
    title: `Task ${id}`,
    kind: 'task',
    status,
    lane,
    contract: { inputs: [], outputs: [], acceptance: [] },
    contract_summary: { input_count: 0, output_count: 0, acceptance_count: 0 },
    expansion: null,
    progress: null,
    ...overrides,
  };
}

function payload(overrides: Record<string, unknown> = {}) {
  return {
    view: 'runtime',
    source: 'live',
    nodes: [
      graphNode('T1', 'completed', ROLE('backend'), { title: 'Prepare API contract' }),
      graphNode('T2', 'blocked', ROLE('backend'), { title: 'Project runtime graph' }),
      graphNode('T3', 'pending', ROLE('frontend'), { title: 'Render work graph' }),
    ],
    edges: [
      { source: 'T1', target: 'T2', kind: 'depends_on', provenance: 'planner' },
      { source: 'T1', target: 'T3', kind: 'touches', provenance: 'codegraph' },
    ],
    waves: [['T1'], ['T2', 'T3']],
    status_by_node: { T1: 'completed', T2: 'blocked', T3: 'pending' },
    lane_assignment: { T1: ROLE('backend'), T2: ROLE('backend'), T3: ROLE('frontend') },
    critical_path: ['T1', 'T2'],
    provenance_by_edge: [],
    divergence: null,
    ...overrides,
  };
}

function mockFetch(body: unknown, ok = true, status = 200) {
  const fetchMock = vi.fn().mockResolvedValue({
    ok,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

/** Find a node's rect by the stable id carried separately from its label. */
function boxFor(container: HTMLElement, id: string): Element | null | undefined {
  return [...container.querySelectorAll('.wg-node')]
    .find((node) => node.getAttribute('data-node-id') === id)
    ?.querySelector('.wg-box');
}

/** Let the $effect fire and its awaited fetch settle. */
async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
}

beforeEach(() => {
  vi.useFakeTimers();
  storeMocks.setActiveSession?.({ id: 'session-a' });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('WorkGraphView', () => {
  it('requests the work-graph projection for the active session', async () => {
    const fetchMock = mockFetch(payload());
    render(WorkGraphView);
    await settle();

    expect(fetchMock).toHaveBeenCalledWith(
      'http://localhost:18800/api/sessions/session-a/work-graph?view=runtime'
    );
  });

  it('renders one node per task, positioned by wave', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    const boxes = container.querySelectorAll('.wg-box');
    expect(boxes).toHaveLength(3);

    // Wave 0 sits above wave 1 — the layout must express dependency depth.
    const y = (id: string) => Number(boxFor(container, id)?.getAttribute('y'));
    expect(y('T1')).toBeLessThan(y('T2'));
    expect(y('T2')).toBe(y('T3'));
  });

  it('truncates and clips every label to its own node box', async () => {
    const longId = '1234567890'.repeat(5);
    const lane = ROLE('runtime-context');
    mockFetch(
      payload({
        nodes: [
          graphNode(longId, 'running', lane, { title: longId }),
        ],
        edges: [],
        waves: [[longId]],
        status_by_node: { [longId]: 'running' },
        lane_assignment: { [longId]: lane },
        critical_path: [],
      })
    );
    const { container } = render(WorkGraphView);
    await settle();

    const node = [...container.querySelectorAll('.wg-node')].find(
      (candidate) => candidate.getAttribute('data-node-id') === longId
    );
    const box = node?.querySelector('.wg-box');
    const clip = node?.querySelector('.wg-label-clip');

    expect(node?.querySelector('.wg-label')?.textContent?.trim()).toBe('12345678…');
    expect(clip?.getAttribute('overflow')).toBe('hidden');
    expect(clip?.getAttribute('x')).toBe(box?.getAttribute('x'));
    expect(clip?.getAttribute('y')).toBe(box?.getAttribute('y'));
    expect(clip?.getAttribute('width')).toBe(box?.getAttribute('width'));
    expect(clip?.getAttribute('height')).toBe(box?.getAttribute('height'));
  });

  it('renders readable titles and keeps runtime context visually secondary', async () => {
    const contextId = `context-${'x'.repeat(42)}`;
    const task = graphNode('T16', 'running', ROLE('frontend'), { title: 'Render node titles' });
    const context = graphNode(contextId, 'running', ROLE('runtime'), {
      title: 'Runtime context',
      kind: 'context',
    });
    mockFetch(
      payload({
        nodes: [task, context],
        edges: [],
        waves: [[task.id, context.id]],
        status_by_node: { [task.id]: task.status, [context.id]: context.status },
        lane_assignment: { [task.id]: task.lane, [context.id]: context.lane },
        critical_path: [task.id],
      })
    );
    const { container } = render(WorkGraphView);
    await settle();

    const taskElement = container.querySelector(`[data-node-id="${task.id}"]`);
    const contextElement = [...container.querySelectorAll('.wg-node')].find(
      (candidate) => candidate.getAttribute('data-node-id') === contextId
    ) as SVGGElement | undefined;

    expect(contextId).toHaveLength(50);
    expect(taskElement?.querySelector('.wg-label')?.textContent?.trim()).toBe('Render n…');
    expect(contextElement?.querySelector('.wg-label')?.textContent?.trim()).toBe('Runtime …');
    expect(contextElement?.classList.contains('wg-node--context')).toBe(true);
    expect(taskElement?.classList.contains('wg-node--context')).toBe(false);
    expect(contextElement?.querySelector('.wg-box')?.getAttribute('rx')).toBe('13');
    expect(taskElement?.querySelector('.wg-box')?.getAttribute('rx')).toBe('5');
    expect(contextElement?.getAttribute('aria-label')).toContain(`Runtime context — ${contextId}`);

    contextElement?.focus();
    expect(document.activeElement).toBe(contextElement);
  });

  it('reveals identical inspector content on hover and keyboard focus', async () => {
    const dependency = graphNode('T1', 'completed', ROLE('backend'), {
      title: 'Prepare API contract',
      kind: 'checkpoint',
    });
    const target = graphNode('T2', 'running', ROLE('frontend'), {
      title: 'Wire node inspector',
      contract: {
        inputs: ['Canonical graph payload'],
        outputs: ['Accessible inspector'],
        acceptance: ['Hover and focus match'],
      },
    });
    mockFetch(
      payload({
        nodes: [dependency, target],
        edges: [{ source: dependency.id, target: target.id, kind: 'depends_on', provenance: 'planner' }],
        waves: [[dependency.id], [target.id]],
        status_by_node: { [dependency.id]: dependency.status, [target.id]: target.status },
        lane_assignment: { [dependency.id]: dependency.lane, [target.id]: target.lane },
        critical_path: [dependency.id, target.id],
      })
    );
    const { container } = render(WorkGraphView);
    await settle();

    expect(container.querySelector('.wg-svg')?.getAttribute('role')).toBe('group');
    const node = container.querySelector(`[data-node-id="${target.id}"]`) as SVGGElement;
    await fireEvent.mouseEnter(node);
    await tick();
    const hoverInspector = screen.getByLabelText('Node inspector for Wire node inspector');
    const hoverContent = hoverInspector.textContent?.replace(/\s+/g, ' ').trim();
    expect(hoverContent).toContain('Canonical graph payload');
    expect(hoverContent).toContain('Prepare API contract');

    await fireEvent.mouseLeave(node);
    await tick();
    expect(screen.queryByLabelText('Node inspector for Wire node inspector')).toBeNull();

    await fireEvent.focus(node);
    await tick();
    const focusInspector = screen.getByLabelText('Node inspector for Wire node inspector');
    expect(focusInspector.textContent?.replace(/\s+/g, ' ').trim()).toBe(hoverContent);
    expect(node.querySelector('title')).toBeNull();
  });

  it('pins the inspector and unpins it with Escape and click-away', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    const node = container.querySelector('[data-node-id="T2"]') as SVGGElement;
    await fireEvent.click(node);
    await tick();
    expect(container.querySelector('.wg-inspector-overlay')?.getAttribute('data-pinned')).toBe('true');

    await fireEvent.mouseLeave(node);
    await fireEvent.blur(node);
    await tick();
    expect(screen.getByLabelText('Node inspector for Project runtime graph')).toBeTruthy();

    await fireEvent.keyDown(window, { key: 'Escape' });
    await tick();
    expect(screen.queryByLabelText('Node inspector for Project runtime graph')).toBeNull();

    await fireEvent.click(node);
    await tick();
    expect(screen.getByLabelText('Node inspector for Project runtime graph')).toBeTruthy();
    await fireEvent.click(document.body);
    await tick();
    expect(screen.queryByLabelText('Node inspector for Project runtime graph')).toBeNull();
  });

  it('flips and clamps the inspector anchor near the canvas edges', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    const rect = (left: number, top: number, width: number, height: number) =>
      ({
        x: left,
        y: top,
        left,
        top,
        width,
        height,
        right: left + width,
        bottom: top + height,
        toJSON: () => ({}),
      }) as DOMRect;
    const canvas = container.querySelector('.wg-canvas') as HTMLDivElement;
    const scroller = container.querySelector('.wg-scroller') as HTMLDivElement;
    const node = container.querySelector('[data-node-id="T2"]') as SVGGElement;
    vi.spyOn(canvas, 'getBoundingClientRect').mockReturnValue(rect(0, 0, 300, 200));
    vi.spyOn(node, 'getBoundingClientRect').mockReturnValue(rect(270, 170, 26, 26));

    await fireEvent.mouseEnter(node);
    await tick();
    const overlay = container.querySelector('.wg-inspector-overlay') as HTMLDivElement;
    vi.spyOn(overlay, 'getBoundingClientRect').mockReturnValue(rect(0, 0, 200, 120));
    await fireEvent.scroll(scroller);
    await tick();

    const left = Number.parseFloat(overlay.style.left);
    const top = Number.parseFloat(overlay.style.top);
    expect(overlay.getAttribute('data-anchor-horizontal')).toBe('left');
    expect(overlay.getAttribute('data-anchor-vertical')).toBe('above');
    expect(left).toBeGreaterThanOrEqual(8);
    expect(left + 200).toBeLessThanOrEqual(292);
    expect(top).toBeGreaterThanOrEqual(8);
    expect(top + 120).toBeLessThanOrEqual(192);
  });

  it('reserves a structural toolbar so controls never occlude wave one', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    const toolbar = container.querySelector('.wg-toolbar');
    const controls = container.querySelector('.wg-controls');
    const canvas = container.querySelector('.wg-canvas');

    expect(toolbar?.contains(controls)).toBe(true);
    expect(canvas?.contains(controls)).toBe(false);
    expect(toolbar?.parentElement).toBe(canvas?.parentElement);
    expect(toolbar!.compareDocumentPosition(canvas!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(getComputedStyle(toolbar!).position).not.toBe('absolute');
  });

  it('renders progress totals and marks the active wave with text', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    expect(screen.getByTestId('nodes-progress').textContent).toBe('1 / 3');
    expect(screen.getByTestId('waves-progress').textContent).toBe('1 / 2');
    expect(screen.getByTestId('critical-path-remaining').textContent).toBe('1');

    const waveLabels = [...container.querySelectorAll('.wg-wave-label')];
    expect(waveLabels.map((label) => label.textContent?.replace(/\s+/g, ' ').trim())).toEqual([
      'Wave 1 of 2',
      'Wave 2 of 2 Active',
    ]);
    expect(waveLabels[0].classList.contains('active')).toBe(false);
    expect(waveLabels[1].classList.contains('active')).toBe(true);
    expect(waveLabels[1].getAttribute('aria-label')).toBe('Wave 2 of 2, Active');
  });

  it('keeps the focused node label byte-identical while timing advances and inspector progress stays readable', async () => {
    vi.setSystemTime(new Date('2026-08-16T19:10:00.000Z'));
    const fresh = graphNode('fresh', 'running', ROLE('frontend'), {
      title: 'Healthy worker',
      progress: {
        started_at: '2026-08-16T19:09:50.000Z',
        finished_at: null,
        attempts: 1,
        agent_id: 'worker-fresh',
        last_heartbeat_at: '2026-08-16T19:09:30.000Z',
      },
    });
    const stale = graphNode('stale', 'running', ROLE('backend'), {
      title: 'Frozen worker',
      progress: {
        started_at: '2026-08-16T19:09:50.000Z',
        finished_at: null,
        attempts: 2,
        agent_id: 'worker-stale',
        last_heartbeat_at: '2026-08-16T19:05:00.000Z',
      },
    });
    const absent = graphNode('absent', 'running', ROLE('runtime'), {
      title: 'Unmeasured worker',
      progress: null,
    });
    const fetchMock = mockFetch(
      payload({
        nodes: [fresh, stale, absent],
        edges: [],
        waves: [[fresh.id, stale.id, absent.id]],
        status_by_node: { fresh: 'running', stale: 'running', absent: 'running' },
        lane_assignment: { fresh: fresh.lane, stale: stale.lane, absent: absent.lane },
        critical_path: [fresh.id],
      })
    );
    const { container } = render(WorkGraphView);
    await settle();

    const nodeFor = (id: string) => container.querySelector(`[data-node-id="${id}"]`)!;
    const progressFor = (id: string) =>
      nodeFor(id).querySelector('.wg-node-progress')?.textContent?.replace(/\s+/g, ' ').trim();

    expect(progressFor('fresh')).toBe('10s · Live');
    expect(progressFor('stale')).toBe('Stale · 10s');
    expect(progressFor('absent')).toBe('No timing');
    expect(nodeFor('fresh').classList.contains('wg-node--fresh')).toBe(true);
    expect(nodeFor('stale').classList.contains('wg-node--stale')).toBe(true);

    const focusedNode = nodeFor('fresh') as SVGGElement;
    await fireEvent.focus(focusedNode);
    await tick();
    const labelBeforeTick = focusedNode.getAttribute('aria-label');
    expect(labelBeforeTick).toBe('Healthy worker — fresh, task, running, role:frontend');

    const inspectorProgress = screen.getByRole('region', { name: 'Node progress' });
    const inspectorTextBeforeTick = inspectorProgress.textContent?.replace(/\s+/g, ' ').trim();
    expect(inspectorProgress.hasAttribute('aria-live')).toBe(false);
    expect(inspectorTextBeforeTick).toContain('2026-08-16T19:09:50.000Z');
    expect(inspectorTextBeforeTick).toContain('2026-08-16T19:09:30.000Z');
    expect(inspectorTextBeforeTick).toContain('worker-fresh');

    vi.advanceTimersByTime(1000);
    await tick();
    expect(progressFor('fresh')).toBe('11s · Live');
    expect(progressFor('stale')).toBe('Stale · 11s');
    expect(focusedNode.getAttribute('aria-label')).toBe(labelBeforeTick);
    expect(inspectorProgress.textContent?.replace(/\s+/g, ' ').trim()).toBe(
      inspectorTextBeforeTick
    );
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('attaches reduced-motion and forced-colors coverage to the animated node boundary', () => {
    expect(workGraphViewSource).toMatch(
      /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.wg-node--fresh \.wg-box--running[\s\S]*?animation: none;/
    );
    expect(workGraphViewSource).toMatch(
      /@media \(forced-colors: active\)[\s\S]*?\.wg-box \{[\s\S]*?forced-color-adjust: auto;/
    );
  });

  it('preserves every node status as a distinct forced-colors pattern', () => {
    const statuses = [
      'ready',
      'running',
      'completed',
      'blocked',
      'pending',
      'failed',
      'cancelled',
    ] as const satisfies readonly NodeStatus[];
    const coversEveryStatus: Exclude<NodeStatus, (typeof statuses)[number]> extends never
      ? true
      : never = true;
    const forcedColors = workGraphViewSource.slice(
      workGraphViewSource.indexOf('@media (forced-colors: active)')
    );
    const patterns = statuses.map((status) => {
      const rule = forcedColors.match(new RegExp(`\\.wg-box--${status}\\s*\\{([^}]*)\\}`));
      const pattern = rule?.[1].match(/stroke-dasharray:\s*([^;]+);/)?.[1].trim();
      expect(pattern, `${status} must retain a forced-colors pattern`).toBeTruthy();
      expect(rule?.[1], `${status} must not reuse critical-path stroke width`).not.toMatch(
        /stroke-width:/
      );
      return pattern;
    });

    expect(coversEveryStatus).toBe(true);
    expect(new Set(patterns).size).toBe(statuses.length);
    expect(patterns[statuses.indexOf('blocked')]).not.toBe(patterns[statuses.indexOf('pending')]);
  });

  it('separates and labels the source badge outside the view controls', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    const source = container.querySelector('.wg-source');
    const controls = container.querySelector('.wg-controls');

    expect(source?.textContent?.replace(/\s+/g, ' ').trim()).toBe('Source live');
    expect(source?.closest('button')).toBeNull();
    expect(controls?.contains(source)).toBe(false);
    expect(controls?.querySelectorAll('button')).toHaveLength(3);
    expect(controls?.textContent).toContain('Runtime');
    expect(controls?.textContent).not.toContain('live');
  });

  it('maps blocked and not-started to mutually exclusive treatments', async () => {
    // The acceptance criterion: these two read identically in a task list and
    // must never read identically here. Assert the payload status → modifier
    // mapping by task id (not by selecting on the modifier, which would be
    // tautological), and assert exclusivity in both directions so a bug that
    // applied both classes could not pass.
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    const blocked = boxFor(container, 'T2'); // status: blocked in the payload
    const pending = boxFor(container, 'T3'); // status: pending in the payload

    expect(blocked?.classList.contains('wg-box--blocked')).toBe(true);
    expect(blocked?.classList.contains('wg-box--pending')).toBe(false);
    expect(pending?.classList.contains('wg-box--pending')).toBe(true);
    expect(pending?.classList.contains('wg-box--blocked')).toBe(false);
  });

  it('marks critical-path nodes distinctly', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    expect(container.querySelectorAll('.wg-node.critical')).toHaveLength(2);
  });

  it('distinguishes edge provenance classes', async () => {
    mockFetch(payload());
    const { container } = render(WorkGraphView);
    await settle();

    expect(container.querySelector('.wg-edge--planner')).not.toBeNull();
    expect(container.querySelector('.wg-edge--codegraph')).not.toBeNull();
  });

  it('explains an empty graph instead of rendering a blank canvas', async () => {
    mockFetch(
      payload({ nodes: [], edges: [], waves: [], status_by_node: {}, lane_assignment: {}, critical_path: [] })
    );
    const { container } = render(WorkGraphView);
    await settle();

    expect(screen.getByText('No tasks in this work graph')).toBeTruthy();
    expect(
      screen.getByText('The work graph endpoint returned no task nodes for this session.')
    ).toBeTruthy();
    expect(container.textContent).not.toContain('started before the work graph shipped');
    expect(container.querySelector('.wg-svg')).toBeNull();
  });

  it('renders non-live omission evidence even when the graph has zero nodes', async () => {
    mockFetch(
      payload({
        nodes: [],
        edges: [],
        waves: [],
        status_by_node: {},
        lane_assignment: {},
        critical_path: [],
        omissions: [
          {
            reason: 'completion_unresolved',
            count: 4,
            detail: 'Four completion events could not be resolved to work-graph tasks.',
            examples: ['agent:worker-1', 'agent:worker-4'],
          },
        ],
      })
    );
    const { container } = render(WorkGraphView);
    await settle();

    const notice = screen.getByRole('region', { name: 'Work graph omissions' });
    expect(notice.classList.contains('lattice-forced-colors-boundary')).toBe(true);
    expect(notice.hasAttribute('aria-live')).toBe(false);
    expect(notice.getAttribute('role')).toBeNull();
    expect(notice.textContent).toContain('4 omitted items');
    expect(notice.textContent).toContain('Completion unresolved');
    expect(notice.textContent).toContain('Count 4');
    expect(notice.textContent).toContain(
      'Four completion events could not be resolved to work-graph tasks.'
    );
    expect(notice.textContent).toContain('agent:worker-1');
    expect(notice.textContent).toContain('agent:worker-4');
    expect(container.querySelector('.wg-msg-title')?.textContent).toBe(
      'No tasks in this work graph'
    );

    // The panel clips its list at max-height and has no focusable descendants,
    // so it must be focusable itself or a keyboard-only user cannot scroll to
    // the omissions below the fold.
    expect(notice.getAttribute('tabindex')).toBe('0');
    expect(notice.querySelector('a, button, input, select, textarea, [tabindex]')).toBeNull();
    notice.focus();
    expect(document.activeElement).toBe(notice);
  });

  it('surfaces a failed request rather than an empty state', async () => {
    mockFetch({ error: 'Session not found: session-a' }, false, 404);
    render(WorkGraphView);
    await settle();

    expect(screen.getByText('Could not load the work graph')).toBeTruthy();
    expect(screen.getByText('Session not found: session-a')).toBeTruthy();
  });
});
