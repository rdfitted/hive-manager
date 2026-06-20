import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import ResumeConfirmModal from './ResumeConfirmModal.svelte';
import type { ResumeReport } from '$lib/stores/sessions';

afterEach(() => {
  cleanup();
});

describe('ResumeConfirmModal', () => {
  it('shows per-step warning rows and default-checks skipped write steps', () => {
    const report: ResumeReport = {
      skipped: [
        {
          run_id: 'session-1',
          step_id: 'step-complete',
          kind: 'git_commit',
          status: 'skipped',
          started_at: '2026-06-19T00:00:00Z',
        },
      ],
      interrupted: [
        {
          run_id: 'session-1',
          step_id: 'step-started',
          kind: 'worker_spawn',
          status: 'unknown',
          started_at: '2026-06-19T00:01:00Z',
        },
      ],
      uncertain: [
        {
          run_id: 'session-1',
          step_id: 'step-started',
          effect_kind: 'branch',
          effect_ref: 'worker-branch',
          confirmed: false,
          confidence: 'uncertain',
          recorded_at: '2026-06-19T00:01:01Z',
        },
      ],
    };

    const { getByText, getByLabelText } = render(ResumeConfirmModal, {
      props: {
        open: true,
        sessionName: 'demo',
        report,
      },
    });

    expect(getByText('Completed steps (1)')).toBeTruthy();
    expect(getByText('Interrupted steps (1)')).toBeTruthy();
    expect(getByText('Unconfirmed side-effects (1)')).toBeTruthy();
    expect(getByText(/worker-bra/)).toBeTruthy();
    expect((getByLabelText('Skip completed write-steps (recommended)') as HTMLInputElement).checked).toBe(true);
  });
});
