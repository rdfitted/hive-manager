import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/svelte';
import ContractViewer from './ContractViewer.svelte';

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    activeSession: writable({ id: 'typed-contract-session' }),
  };
});

afterEach(() => {
  cleanup();
  mocks.invoke.mockReset();
});

describe('ContractViewer', () => {
  it('shows an empty state without a contract and never invokes the missing command', () => {
    render(ContractViewer);

    expect(screen.getByText('No sprint contract available.')).toBeTruthy();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it('renders every typed criterion kind without invoking the legacy command', () => {
    render(ContractViewer, {
      props: {
        contract: {
          milestone_name: 'Typed QA',
          acceptance_criteria: [
            {
              number: 1,
              category: 'FUNC',
              kind: 'PassFail',
              description: 'The primary workflow completes',
            },
            {
              number: 2,
              category: 'DESIGN 1-10 floor 5',
              kind: { Scored: { min: 1, max: 10, floor: 5 } },
              description: 'The interface remains coherent',
            },
            {
              number: 3,
              category: 'PERF lighthouse >= 80',
              kind: {
                Measured: { metric: 'lighthouse', op: 'Ge', target: 80 },
              },
              description: 'The measured budget holds',
            },
            {
              number: 4,
              category: null,
              kind: 'Unspecified',
              description: 'Legacy prose remains visible',
            },
          ],
          pass_threshold: ['All pass/fail criteria must PASS'],
          threshold_policy: {
            Rules: {
              require_all_pass_fail: true,
              scored_mean: null,
              fail_scored_below_floor: true,
            },
          },
          warnings: [],
          raw_markdown: '# Sprint Contract: Typed QA',
        },
      },
    });

    const criteria = screen.getByRole('region', { name: 'Acceptance criteria' });
    expect(within(criteria).getByText('PassFail')).toBeTruthy();
    expect(within(criteria).getByText('Scored 1-10 floor 5')).toBeTruthy();
    expect(within(criteria).getByText('Measured lighthouse >= 80')).toBeTruthy();
    expect(within(criteria).getByText('Unspecified')).toBeTruthy();
    expect(within(criteria).getAllByRole('listitem')).toHaveLength(4);
    expect(screen.getByText('All pass/fail criteria must PASS')).toBeTruthy();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});
