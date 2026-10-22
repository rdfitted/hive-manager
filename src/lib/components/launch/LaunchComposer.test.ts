// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import LaunchComposer from './LaunchComposer.svelte';

afterEach(cleanup);
const summary = 'QA: on · Evaluator + 3 (UI, API, A11Y)';
function composer(mode = 'hive', overrides = {}) {
  const onqa = vi.fn();
  const onlaunch = vi.fn();
  const onsmoke = vi.fn();
  return { onqa, onlaunch, onsmoke, ...render(LaunchComposer, { props: {
    mode, projectPath: 'D:/project', showQa: mode === 'hive' || mode === 'solo',
    showSmoke: mode === 'hive' || mode === 'research', qaEnabled: true,
    qaSummary: summary, qaCaveat: 'UI and A11Y workers need a rendered browser.',
    onqa, onlaunch, onsmoke, onmode: vi.fn(), onvalue: vi.fn(), onworkers: vi.fn(),
    onpreset: vi.fn(), onbrowse: vi.fn(), onadvanced: vi.fn(), ...overrides,
  } }) };
}

describe('LaunchComposer controls', () => {
  it.each(['hive', 'solo', 'research', 'fusion', 'debate', 'templates'])('%s shows only its QA and smoke controls', (mode) => {
    const view = composer(mode);
    expect(!!view.queryByRole('button', { name: /^QA/ })).toBe(['hive', 'solo'].includes(mode));
    expect(!!view.queryByRole('button', { name: 'Smoke Test' })).toBe(['hive', 'research'].includes(mode));
  });
  it('QA exposes R4 wording, pressed state and a persistent accessible caveat', async () => {
    const view = composer();
    const chip = view.getByRole('button', { name: summary });
    expect(chip.getAttribute('aria-pressed')).toBe('true');
    expect(chip.tabIndex).toBe(0);
    expect(view.container.querySelector(`#${chip.getAttribute('aria-describedby')}`)?.textContent).toContain('rendered browser');
    await fireEvent.click(chip);
    expect(view.onqa).toHaveBeenCalledWith(false);
    expect(chip.getAttribute('aria-pressed')).toBe('true'); // Parent owns the state.
    await view.rerender({ qaEnabled: false, qaSummary: 'QA: off' });
    expect(view.getByRole('button', { name: 'QA: off' }).getAttribute('aria-pressed')).toBe('false');
  });
  it('smoke is a secondary action and invokes its dedicated callback', async () => {
    const view = composer();
    const smoke = view.getByRole('button', { name: 'Smoke Test' });
    expect(smoke.classList.contains('lattice-btn--secondary')).toBe(true);
    expect(smoke.classList.contains('lattice-btn--filled')).toBe(false);
    await fireEvent.click(smoke);
    expect(view.onsmoke).toHaveBeenCalledOnce();
    expect(view.onlaunch).not.toHaveBeenCalled();
  });
  it.each([{ launching: true }, { projectPath: ' ' }, { mode: 'templates' }])('disables Smoke and Launch together for %j', (overrides) => {
    const view = composer('hive', overrides);
    for (const name of ['Launch', 'Smoke Test']) expect((view.getByRole('button', { name: new RegExp(`^${name}`) }) as HTMLButtonElement).disabled).toBe(true);
  });
  it.each(['ctrlKey', 'metaKey'])('%s+Enter launches; plain Enter and IME Enter do not', async (modifier) => {
    const view = composer();
    const editor = view.getByLabelText('What would you like to work on?');
    await fireEvent.keyDown(editor, { key: 'Enter' });
    await fireEvent.keyDown(editor, { key: 'Enter', [modifier]: true, isComposing: true });
    expect(view.onlaunch).not.toHaveBeenCalled();
    await fireEvent.keyDown(editor, { key: 'Enter', [modifier]: true });
    expect(view.onlaunch).toHaveBeenCalledOnce();
    expect(editor.classList.contains('lattice-forced-colors-boundary')).toBe(true);
  });
  it('renders Cancel only when a close callback exists', async () => {
    const view = composer();
    expect(view.queryByRole('button', { name: 'Cancel' })).toBeNull();
    const oncancel = vi.fn();
    await view.rerender({ oncancel });
    await fireEvent.click(view.getByRole('button', { name: 'Cancel' }));
    expect(oncancel).toHaveBeenCalledOnce();
  });
});
