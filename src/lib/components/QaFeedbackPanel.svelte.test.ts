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
      summary: 'One criterion needs work',
      timestamp: '2026-09-22T22:00:00.000Z',
      criteria: [
        {
          id: '1',
          label: 'Primary flow completes',
          passed: true,
          evidence: 'API worker observed completion',
        },
        {
          id: '2',
          label: 'Failure is visible',
          passed: false,
          evidence: 'UI worker reproduced the regression',
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
  });
});
