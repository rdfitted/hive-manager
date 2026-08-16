import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/svelte';
import NodeInspector from './NodeInspector.svelte';

afterEach(cleanup);

describe('NodeInspector', () => {
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
    expect(within(card).getByText('No immediate dependencies')).toBeTruthy();
  });
});
