import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import KnowledgeTable from './KnowledgeTable.svelte';
import type { KnowledgeNode } from '$lib/knowledge/types';

const nodes: KnowledgeNode[] = [
  {
    id: 'beta',
    title: 'Beta',
    folder: 'practices',
    path: 'practices/beta.md',
    last_updated: '2026-07-18T00:00:00Z',
    in_degree: 1,
    out_degree: 0,
  },
  {
    id: 'alpha',
    title: 'Alpha',
    folder: 'patterns',
    path: 'patterns/alpha.md',
    last_updated: '2026-07-01T00:00:00Z',
    in_degree: 2,
    out_degree: 3,
  },
];

function rowTitles(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('tbody .title')).map((element) =>
    element.textContent?.trim() ?? '',
  );
}

afterEach(cleanup);

describe('KnowledgeTable', () => {
  it('sorts every row and exposes a native action without replacing table semantics', async () => {
    const onSelect = vi.fn();
    const { container, getByRole } = render(KnowledgeTable, {
      props: { nodes, selectedId: null, onSelect },
    });

    expect(rowTitles(container)).toEqual(['Alpha', 'Beta']);

    await fireEvent.click(getByRole('button', { name: /Degree/ }));
    expect(rowTitles(container)).toEqual(['Alpha', 'Beta']);
    expect(getByRole('columnheader', { name: /Degree/ }).getAttribute('aria-sort')).toBe('descending');

    await fireEvent.click(getByRole('button', { name: /Degree/ }));
    expect(rowTitles(container)).toEqual(['Beta', 'Alpha']);

    const alphaAction = getByRole('button', { name: 'Open Alpha' });
    expect(alphaAction.tagName).toBe('BUTTON');
    expect(alphaAction.closest('tr')?.getAttribute('role')).toBeNull();
    expect(alphaAction.closest('tr')?.hasAttribute('tabindex')).toBe(false);

    await fireEvent.click(alphaAction);
    expect(onSelect).toHaveBeenCalledWith('alpha', alphaAction);
  });
});
