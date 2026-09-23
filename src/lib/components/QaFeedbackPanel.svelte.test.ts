import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor, within } from '@testing-library/svelte';
import QaFeedbackPanel from './QaFeedbackPanel.svelte';

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));

vi.mock('$lib/stores/sessions', async () => {
  const { writable } = await import('svelte/store');
  return {
    activeSession: writable({ id: 'qa-panel-session' }),
  };
});

afterEach(() => {
  cleanup();
  mocks.invoke.mockReset();
  mocks.listen.mockClear();
});

describe('QaFeedbackPanel', () => {
  it('loads and renders every persisted criterion result', async () => {
    mocks.invoke.mockResolvedValue({
      session_id: 'qa-panel-session',
      milestone_id: 'Typed QA',
      iteration: 2,
      passed: false,
      advisory_verdict: 'fail',
      advisory_disagrees: false,
      advisory_threshold_disagreement: false,
      advisory_flip: false,
      summary: 'One criterion needs work',
      timestamp: '2026-09-22T22:00:00.000Z',
      criteria: [
        {
          id: '1',
          label: 'Primary flow completes',
          passed: true,
          evidence: 'API worker observed completion',
          advisory_status: 'pass',
          advisory_threshold_disagreement: false,
          unchanged_evidence_flip: false,
        },
        {
          id: '2',
          label: 'Failure is visible',
          passed: false,
          evidence: 'UI worker reproduced the regression',
          advisory_status: 'fail',
          advisory_threshold_disagreement: false,
          unchanged_evidence_flip: false,
        },
      ],
    });

    render(QaFeedbackPanel);

    await waitFor(() => {
      expect(screen.getByText('One criterion needs work')).toBeTruthy();
    });
    expect(mocks.invoke).toHaveBeenCalledWith('get_qa_verdict', {
      sessionId: 'qa-panel-session',
    });
    const criteria = screen.getByRole('list', { name: 'QA criteria' });
    expect(within(criteria).getAllByRole('listitem')).toHaveLength(2);
    expect(within(criteria).getByText('Criterion 1')).toBeTruthy();
    expect(within(criteria).getByText('Primary flow completes')).toBeTruthy();
    expect(within(criteria).getByText('API worker observed completion')).toBeTruthy();
    expect(within(criteria).getByText('Criterion 2')).toBeTruthy();
    expect(within(criteria).getByText('Failure is visible')).toBeTruthy();
    expect(within(criteria).getByText('UI worker reproduced the regression')).toBeTruthy();
    const comparison = screen.getByRole('group', { name: 'QA verdict comparison' });
    expect(within(screen.getByRole('group', { name: 'Evaluator verdict' })).getByText('Fail')).toBeTruthy();
    expect(within(screen.getByRole('group', { name: 'Advisory verdict' })).getByText('Fail')).toBeTruthy();
    expect(comparison.classList.contains('disagreement')).toBe(false);
    expect(comparison.classList.contains('lattice-forced-colors-boundary')).toBe(true);
    expect(comparison.classList.contains('lattice-forced-colors-boundary--active')).toBe(false);
    expect(within(criteria).getAllByText('Advisory: Fail')).toHaveLength(1);
  });

  it('highlights a threshold-only disagreement without changing the Evaluator result', async () => {
    mocks.invoke.mockResolvedValue({
      session_id: 'qa-panel-session',
      milestone_id: 'Typed QA',
      iteration: 3,
      passed: false,
      advisory_verdict: 'pass',
      advisory_disagrees: true,
      advisory_threshold_disagreement: true,
      advisory_flip: false,
      summary: 'Evaluator requested another attempt',
      timestamp: '2026-09-22T22:00:00.000Z',
      criteria: [{
        id: '1',
        label: 'Score clears the minimum',
        passed: false,
        evidence: 'Score falls below the floor',
        advisory_status: 'pass',
        advisory_threshold_disagreement: true,
        unchanged_evidence_flip: false,
      }],
    });

    render(QaFeedbackPanel);
    const comparison = await screen.findByRole('group', { name: 'QA verdict comparison' });
    expect(within(screen.getByRole('group', { name: 'Evaluator verdict' })).getByText('Fail')).toBeTruthy();
    expect(within(screen.getByRole('group', { name: 'Advisory verdict' })).getByText('Pass')).toBeTruthy();
    expect(comparison.classList.contains('disagreement')).toBe(true);
    expect(comparison.classList.contains('lattice-forced-colors-boundary--active')).toBe(true);
    expect(within(comparison).getByText('Threshold disagreement')).toBeTruthy();
    const criterion = within(screen.getByRole('list', { name: 'QA criteria' })).getByRole('listitem');
    expect(criterion.classList.contains('disagreement')).toBe(true);
    expect(criterion.classList.contains('lattice-forced-colors-boundary')).toBe(true);
    expect(criterion.classList.contains('lattice-forced-colors-boundary--active')).toBe(true);
    expect(within(criterion).getByText('Evaluator: Fail')).toBeTruthy();
    expect(within(criterion).getByText('Advisory: Pass')).toBeTruthy();
    expect(within(criterion).getByText('Threshold disagreement')).toBeTruthy();
  });

  it('keeps an undetermined advisory verdict distinct from a disagreement', async () => {
    mocks.invoke.mockResolvedValue({
      session_id: 'qa-panel-session',
      milestone_id: 'Legacy QA',
      iteration: 1,
      passed: true,
      advisory_verdict: 'undetermined',
      advisory_disagrees: false,
      advisory_threshold_disagreement: false,
      advisory_flip: false,
      summary: 'Legacy verdict',
      timestamp: '2026-09-22T22:00:00.000Z',
      criteria: [],
    });

    render(QaFeedbackPanel);
    const comparison = await screen.findByRole('group', { name: 'QA verdict comparison' });
    expect(within(screen.getByRole('group', { name: 'Evaluator verdict' })).getByText('Pass')).toBeTruthy();
    expect(within(screen.getByRole('group', { name: 'Advisory verdict' })).getByText('Undetermined')).toBeTruthy();
    expect(comparison.classList.contains('disagreement')).toBe(false);
    expect(comparison.classList.contains('lattice-forced-colors-boundary--active')).toBe(false);
    expect(within(comparison).queryByText('Verdicts disagree')).toBeNull();
  });
});
