import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { sessions, type Session } from '$lib/stores/sessions';
import ResumeConfirmModal from './ResumeConfirmModal.svelte';
import type { ResumeReport } from '$lib/stores/sessions';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

afterEach(() => {
  cleanup();
  sessions.dismissResumeNotice();
  vi.clearAllMocks();
  document.body.innerHTML = '';
});

describe('ResumeConfirmModal', () => {
  it('names agents without saved conversations after the sidebar closes its confirmation', async () => {
    const messages = [
      'Synthetic principal (fixture-worker-1): no saved conversation ID; Resume is unavailable.',
      'Synthetic reviewer (fixture-worker-2): no saved conversation ID; Resume is unavailable.',
    ];
    const view = render(ResumeConfirmModal, { props: { open: true, sessionName: 'Synthetic session' } });
    vi.mocked(invoke).mockResolvedValueOnce({
      id: 'fixture-resume-report', name: 'Synthetic session', project_path: 'C:/synthetic-report', agents: [],
      agent_resume_report: messages,
    } as unknown as Session);
    await sessions.resumeSession('fixture-resume-report');
    await tick();
    expect(view.getByRole('dialog', { name: 'Resume session' })).toBeTruthy();
    expect(view.queryByRole('dialog', { name: 'Resume report' })).toBeNull();
    // SessionSidebar resets these props after awaiting the returned session.
    await view.rerender({ open: false, sessionName: null, report: null });
    const report = await waitFor(() => view.getByRole('dialog', { name: 'Resume report' }));
    expect(within(report).getAllByRole('listitem').map((item) => item.textContent)).toEqual(messages);
    expect(within(report).getByText('Loaded Synthetic session')).toBeTruthy();
    await fireEvent.click(within(report).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(view.queryByRole('dialog', { name: 'Resume report' })).toBeNull());
    expect(get(sessions).resumeNotice).toBeNull();
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith('resume_session', { sessionId: 'fixture-resume-report' });
  });

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
