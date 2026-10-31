import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import ResumeConfirmModal from './ResumeConfirmModal.svelte';
import type { ResumeReport } from '$lib/stores/sessions';

afterEach(() => {
  cleanup();
  document.body.innerHTML = '';
});

describe('ResumeConfirmModal', () => {
  it.each(['Escape', 'outside click'] as const)(
    'ignores %s while confirming and allows it after confirmation finishes',
    async (interaction) => {
      const outside = document.createElement('button');
      document.body.append(outside);
      const cancel = vi.fn();
      const view = render(ResumeConfirmModal, {
        props: { open: false, confirming: true },
        events: { cancel },
      });
      await view.rerender({ open: true });
      const dialog = await waitFor(() => view.getByRole('dialog', { name: 'Resume session' }));
      // Bits registers its Escape and outside-interaction layers after opening.
      await new Promise((resolve) => requestAnimationFrame(resolve));

      async function dismiss() {
        if (interaction === 'Escape') {
          (document.activeElement ?? document.body).dispatchEvent(
            new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }),
          );
          await tick();
          await Promise.resolve();
        } else {
          outside.dispatchEvent(new MouseEvent('pointerdown', {
            bubbles: true, cancelable: true, button: 0, clientX: 100, clientY: 100,
          }));
          outside.click();
          await new Promise((resolve) => setTimeout(resolve, 30));
        }
      }

      await dismiss();
      expect(view.getByRole('dialog', { name: 'Resume session' })).toBe(dialog);
      expect(dialog.getAttribute('data-state')).toBe('open');
      expect(cancel).not.toHaveBeenCalled();

      await view.rerender({ confirming: false });
      await dismiss();
      await waitFor(() => expect(view.queryByRole('dialog', { name: 'Resume session' })).toBeNull());
      expect(cancel).toHaveBeenCalledOnce();
    },
  );

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
