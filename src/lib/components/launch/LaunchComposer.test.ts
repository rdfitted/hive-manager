// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import LaunchComposer from './LaunchComposer.svelte';
import source from './LaunchComposer.svelte?raw';

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
  it('F6: declares the editor focus-visible accent outline before its forced-colors override (source check only)', () => {
    const [normal, forced] = source.split('@media (forced-colors: active)');
    expect(normal, 'normal-color editor focus outline declaration').toMatch(/textarea:focus-visible\s*\{[^}]*outline:\s*2px solid var\(--accent-cyan\);[^}]*outline-offset:\s*2px;/);
    expect(normal).toContain('.composer-input:focus-within { border-color: var(--accent-cyan); }');
    expect(forced).toContain('textarea:focus-visible { outline: 2px solid Highlight; outline-offset: 2px; }');
  });
  it('keeps every configuration chip and action inside the card after the editor', async () => {
    const view = composer('hive', { oncancel: vi.fn() });
    const editor = view.getByLabelText('What would you like to work on?');
    const card = view.getByRole('group', { name: 'Launch composer' });
    expect(card.contains(editor)).toBe(true);
    for (const label of ['Mode', 'Workers', 'Preset', 'Codex transport', 'Claude transport']) {
      expect(card.contains(view.getByLabelText(label))).toBe(true);
    }
    for (const name of [/Project/, /^QA/, 'Advanced', 'Smoke Test', 'Cancel', 'Launch']) {
      expect(card.contains(view.getByRole('button', { name }))).toBe(true);
    }
    const tabbables = Array.from(card.querySelectorAll('textarea, select, button'));
    expect(tabbables[0]).toBe(editor);
    expect(tabbables[tabbables.length - 1]).toBe(view.getByRole('button', { name: 'Launch' }));
    await fireEvent.pointerDown(card);
    expect(document.activeElement).toBe(editor);
  });

  it('keeps text and chip callbacks independent from launch', async () => {
    const onvalue = vi.fn();
    const onmode = vi.fn();
    const onworkers = vi.fn();
    const onpreset = vi.fn();
    const oncodextransport = vi.fn();
    const onclaudetransport = vi.fn();
    const onbrowse = vi.fn();
    const onadvanced = vi.fn();
    const view = composer('hive', { onvalue, onmode, onworkers, onpreset, oncodextransport, onclaudetransport, onbrowse, onadvanced });
    await fireEvent.input(view.getByLabelText('What would you like to work on?'), { target: { value: 'keep\nall text' } });
    await fireEvent.change(view.getByLabelText('Mode'), { target: { value: 'solo' } });
    await fireEvent.change(view.getByLabelText('Workers'), { target: { value: '2' } });
    await fireEvent.change(view.getByLabelText('Preset'), { target: { value: 'custom' } });
    await fireEvent.change(view.getByLabelText('Codex transport'), { target: { value: 'protocol' } });
    await fireEvent.change(view.getByLabelText('Claude transport'), { target: { value: 'protocol' } });
    await fireEvent.click(view.getByRole('button', { name: /Project/ }));
    await fireEvent.click(view.getByRole('button', { name: 'Advanced' }));
    expect(onvalue).toHaveBeenCalledWith('keep\nall text');
    expect(onmode).toHaveBeenCalledWith('solo');
    expect(onworkers).toHaveBeenCalledWith(2);
    expect(onpreset).toHaveBeenCalledWith('custom');
    expect(oncodextransport).toHaveBeenCalledWith(true);
    expect(onclaudetransport).toHaveBeenCalledWith(true);
    expect(onbrowse).toHaveBeenCalledOnce();
    expect(onadvanced).toHaveBeenCalledOnce();
    expect(view.onlaunch).not.toHaveBeenCalled();
  });

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
