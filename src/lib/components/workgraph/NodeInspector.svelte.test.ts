import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/svelte';
import NodeInspector from './NodeInspector.svelte';
import nodeInspectorSource from './NodeInspector.svelte?raw';

afterEach(cleanup);

describe('NodeInspector', () => {
  it('exposes the contract as a named region without widening section spacing', () => {
    render(NodeInspector, {
      props: {
        node: {
          id: 'T13',
          title: 'Build the node inspector',
          kind: 'task',
          lane: 'wg-cards',
          status: 'running',
          contract: { inputs: [], outputs: [], acceptance: [] },
        },
        dependencies: [],
      },
    });

    expect(screen.getByRole('region', { name: 'Node contract' })).toBeTruthy();
    expect(nodeInspectorSource).toContain('.contract section + section {');
    expect(nodeInspectorSource).not.toMatch(/^\s*section \+ section \{/m);
  });

  it('renders the full node contract and immediate dependencies', () => {
    render(NodeInspector, {
      props: {
        node: {
          id: 'T13',
          title: 'Build the node inspector',
          kind: 'task',
          lane: 'wg-cards',
          status: 'running',
          contract: {
            inputs: ['Work graph node payload', 'Immediate adjacency'],
            outputs: ['Standalone inspector card'],
            acceptance: ['Content is reachable on hover and keyboard focus'],
          },
        },
        dependencies: [
          { id: 'T11', title: 'Extract graph helpers', kind: 'task' },
          { id: 'T12', title: 'Publish frontend types', kind: 'task' },
        ],
      },
    });

    const card = screen.getByRole('complementary', {
      name: 'Node inspector for Build the node inspector',
    });
    expect(card.classList.contains('lattice-forced-colors-boundary')).toBe(true);
    expect(within(card).getByRole('heading', { name: 'Build the node inspector' })).toBeTruthy();
    expect(within(card).getByText('T13')).toBeTruthy();
    expect(card.querySelector('.kind-badge')?.textContent).toBe('task');
    expect(within(card).getByText('wg-cards')).toBeTruthy();
    expect(within(card).getAllByText('running')).toHaveLength(2);

    expect(within(card).getByRole('heading', { name: 'Inputs' })).toBeTruthy();
    expect(within(card).getByText('Work graph node payload')).toBeTruthy();
    expect(within(card).getByText('Immediate adjacency')).toBeTruthy();
    expect(within(card).getByRole('heading', { name: 'Outputs' })).toBeTruthy();
    expect(within(card).getByText('Standalone inspector card')).toBeTruthy();
    expect(within(card).getByRole('heading', { name: 'Acceptance' })).toBeTruthy();
    expect(
      within(card).getByText('Content is reachable on hover and keyboard focus')
    ).toBeTruthy();

    expect(within(card).getByRole('heading', { name: 'Immediate dependencies' })).toBeTruthy();
    expect(within(card).getByText('Extract graph helpers')).toBeTruthy();
    expect(within(card).getByText('T11')).toBeTruthy();
    expect(within(card).getByText('Publish frontend types')).toBeTruthy();
    expect(within(card).getByText('T12')).toBeTruthy();
    expect(within(card).queryByText('No contract recorded')).toBeNull();
  });

  it('renders progress timing in a static accessible region', () => {
    render(NodeInspector, {
      props: {
        node: {
          id: 'T8',
          title: 'Expose runtime progress',
          kind: 'task',
          lane: 'api',
          status: 'running',
          contract: { inputs: [], outputs: [], acceptance: [] },
        },
        dependencies: [],
        progress: {
          started_at: '2026-08-16T19:09:50.000Z',
          finished_at: null,
          attempts: 2,
          agent_id: 'worker-api',
          last_heartbeat_at: '2026-08-16T19:09:30.000Z',
        },
      },
    });

    const progressRegion = screen.getByRole('region', { name: 'Node progress' });
    expect(progressRegion.hasAttribute('aria-live')).toBe(false);
    expect(within(progressRegion).getByText('2026-08-16T19:09:50.000Z')).toBeTruthy();
    expect(within(progressRegion).getByText('2026-08-16T19:09:30.000Z')).toBeTruthy();
    expect(within(progressRegion).getByText('2')).toBeTruthy();
    expect(within(progressRegion).getByText('worker-api')).toBeTruthy();
    expect(within(progressRegion).getByText('Not recorded')).toBeTruthy();
  });

  it('renders inferred completion provenance and every source reference', () => {
    render(NodeInspector, {
      props: {
        node: {
          id: 'T4',
          title: 'Lane fan-out completion',
          kind: 'task',
          lane: 'role:P1',
          status: 'completed',
          contract: { inputs: [], outputs: [], acceptance: [] },
        },
        dependencies: [],
        completionProvenance: 'inferred',
        completionSourceRefs: ['event:lane-complete', 'event:worker-finalized'],
      },
    });

    const evidence = screen.getByRole('region', { name: 'Completion evidence' });
    expect(within(evidence).getByTestId('completion-provenance').textContent).toBe('Inferred');
    expect(within(evidence).getByText('Completed through lane fan-out')).toBeTruthy();

    const sourceRefs = within(evidence).getByRole('list', {
      name: 'Completion source references',
    });
    expect(within(sourceRefs).getAllByRole('listitem')).toHaveLength(2);
    expect(within(sourceRefs).getByText('event:lane-complete')).toBeTruthy();
    expect(within(sourceRefs).getByText('event:worker-finalized')).toBeTruthy();
  });

  it('renders exactly one fallback line for an empty contract without empty headings', () => {
    render(NodeInspector, {
      props: {
        node: {
          id: 'context-1',
          title: 'Runtime context',
          kind: 'context',
          lane: 'runtime',
          status: 'pending',
          contract: { inputs: [], outputs: [], acceptance: [] },
        },
        dependencies: [],
      },
    });

    const card = screen.getByRole('complementary', {
      name: 'Node inspector for Runtime context',
    });
    expect(within(card).getAllByText('No contract recorded')).toHaveLength(1);
    expect(within(card).queryByRole('heading', { name: 'Inputs' })).toBeNull();
    expect(within(card).queryByRole('heading', { name: 'Outputs' })).toBeNull();
    expect(within(card).queryByRole('heading', { name: 'Acceptance' })).toBeNull();
    expect(within(card).getByText('No progress recorded')).toBeTruthy();
    expect(within(card).getByText('No immediate dependencies')).toBeTruthy();
  });
});
