import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import type { AgentInfo } from '$lib/stores/sessions';
const fixture = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined), consume: vi.fn().mockResolvedValue(null) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: fixture.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('$lib/stores/agentEvents', async () => ({ agentEvents: (await import('svelte/store')).writable({ sessionId: 's', feeds: {}, histories: {} }) }));
vi.mock('$lib/stores/pendingContext', async () => ({ pendingContext: { ...(await import('svelte/store')).writable({ current: null }), consume: fixture.consume, render: () => 'Fixture context' } }));
import CommandBar from './CommandBar.svelte';
import CommandBarHarness from './CommandBarHarness.test.svelte';
const agents: AgentInfo[] = ['Alpha', 'Beta'].map((label, index) => ({ id: label.toLowerCase(), role: index ? { Worker: { index, parent: null } } : 'Queen', status: 'Running', parent_id: null, config: { label, cli: 'codex', model: 'fixture-model', flags: ['-c', 'model_reasoning_effort="high"'] } }));
afterEach(() => { cleanup(); vi.clearAllMocks(); fixture.invoke.mockResolvedValue(undefined); fixture.consume.mockResolvedValue(null); });
async function type(editor: HTMLElement, text: string) {
  editor.textContent = text; await fireEvent.input(editor); return editor;
}
describe('Ops Wall command routing', () => {
  it('G59 consume race: captures recipients and draft ownership before target/session changes', async () => {
    let resolve!: (value: null) => void;
    fixture.consume.mockImplementationOnce(() => new Promise<null>(done => { resolve = done; }));
    const view = render(CommandBarHarness, { sessionId: 's', agents, targetId: 'alpha', initialDrafts: { 's:alpha': 'Alpha private', 's:beta': 'Beta preserved', 'other:alpha': 'Other preserved' } });
    const editor = view.getByRole('textbox');
    await fireEvent.keyDown(editor, { key: 'Enter' });
    await waitFor(() => expect(fixture.consume).toHaveBeenCalledWith('s'));
    await fireEvent.keyDown(editor, { key: 'Tab' });
    expect(editor.textContent).toBe('Beta preserved');
    await view.rerender({ sessionId: 'other', targetId: 'alpha' });
    expect(editor.textContent).toBe('Other preserved');
    resolve(null);
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', { request: { session_id: 's', target_agent_id: 'alpha', message: 'Alpha private' } }));
    expect(fixture.invoke).toHaveBeenCalledTimes(1);
    expect(editor.textContent).toBe('Other preserved');
    await view.rerender({ sessionId: 's', targetId: 'beta' });
    expect(editor.textContent).toBe('Beta preserved');
    await view.rerender({ targetId: 'alpha' });
    expect(editor.textContent).toBe('');
  });
  it('G59 picker Enter selects a target while the editor retains focus, without submitting', async () => {
    const view = render(CommandBar, { sessionId: 's', agents, targetId: 'alpha' });
    const editor = await type(view.getByRole('textbox'), 'Alpha unsent'); editor.focus();
    await fireEvent.click(view.getByRole('button', { name: 'Command target: @Alpha' }));
    await fireEvent.keyDown(editor, { key: 'ArrowDown' });
    await fireEvent.keyDown(editor, { key: 'Enter' });
    expect(view.queryByRole('button', { name: 'Command target: @Beta' })).toBeTruthy();
    expect(document.activeElement).toBe(editor);
    expect(fixture.consume).not.toHaveBeenCalled();
    expect(fixture.invoke).not.toHaveBeenCalled();
    await fireEvent.keyDown(editor, { key: 'Tab', shiftKey: true });
    expect(editor.textContent).toBe('Alpha unsent');
  });
  it('G59 picker opens above the bottom bar and remains within the viewport', async () => {
    const view = render(CommandBar, { sessionId: 's', agents, targetId: 'alpha' });
    const picker = view.getByRole('button', { name: 'Command target: @Alpha' });
    picker.getBoundingClientRect = () => ({ left: window.innerWidth - 20, top: window.innerHeight - 40, right: window.innerWidth, bottom: window.innerHeight - 20, width: 20, height: 20, x: window.innerWidth - 20, y: window.innerHeight - 40, toJSON() {} });
    await fireEvent.click(picker);
    const menu = view.getByRole('listbox', { name: 'Mentions' });
    expect(parseFloat(menu.style.left)).toBeLessThanOrEqual(window.innerWidth - 320);
    expect(parseFloat(menu.style.top) + Math.min(240, 3 * 34 + 8)).toBeLessThan(window.innerHeight - 40);
  });
  it('G59 target-only: payload mentions never change the selected recipient', async () => {
    const view = render(CommandBar, { sessionId: 's', agents, targetId: 'beta' });
    await type(view.getByRole('textbox'), 'Message @Alpha');
    await fireEvent.click(view.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', { request: { session_id: 's', target_agent_id: 'beta', message: 'Message @Alpha' } }));
    expect(fixture.invoke).toHaveBeenCalledTimes(1);
  });
  it('G59 all: each unique agent receives once even when another recipient fails', async () => {
    fixture.invoke.mockImplementation(async (_command: string, args: { request: { target_agent_id: string } }) => { if (args.request.target_agent_id === 'alpha') throw new Error('Fixture send failure'); });
    const delivery = vi.fn();
    const view = render(CommandBar, { sessionId: 's', agents: [...agents, agents[0]], targetId: '@all', ondelivery: delivery });
    await type(view.getByRole('textbox'), 'Broadcast fixture');
    await fireEvent.click(view.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledTimes(2));
    expect(fixture.invoke.mock.calls.map(call => call[1].request.target_agent_id).sort()).toEqual(['alpha', 'beta']);
    await waitFor(() => expect(delivery).toHaveBeenCalledWith('s', 'beta', 'Sent'));
    expect(delivery).toHaveBeenCalledWith('s', 'alpha', 'Send failed');
  });
  it('CR-5 Grid: editor identity and per-agent drafts survive target changes and Tab cycles', async () => {
    const drafts = { 's:alpha': 'Alpha saved', 's:beta': 'Beta saved' };
    const view = render(CommandBar, { sessionId: 's', agents, targetId: 'alpha', drafts });
    const editor = view.getByRole('textbox');
    expect(editor.textContent).toBe('Alpha saved');
    await type(view.getByRole('textbox'), 'Alpha draft');
    await fireEvent.keyDown(editor, { key: 'Tab' }); await tick();
    expect(view.getByRole('button', { name: 'Command target: @Beta' })).toBeTruthy();
    expect(view.getByRole('textbox')).toBe(editor);
    expect(editor.textContent).toBe('Beta saved');
    await type(view.getByRole('textbox'), 'Beta draft');
    await fireEvent.keyDown(editor, { key: 'Tab', shiftKey: true }); await tick();
    expect(view.getByRole('textbox')).toBe(editor);
    expect(editor.textContent).toBe('Alpha draft');
    await fireEvent.click(view.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(fixture.invoke).toHaveBeenCalledWith('operator_inject', { request: { session_id: 's', target_agent_id: 'alpha', message: 'Alpha draft' } }));
    expect(fixture.invoke).toHaveBeenCalledTimes(1);
    await fireEvent.keyDown(editor, { key: 'Tab' });
    expect(editor.textContent).toBe('Beta draft');
  });
  it('G59 picker reuses stable IDs and preserves an editor-owned Tab', async () => {
    const view = render(CommandBar, { sessionId: 's', agents, targetId: 'alpha' });
    const editor = view.getByRole('textbox');
    const handled = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }); handled.preventDefault(); editor.dispatchEvent(handled); await tick();
    expect(view.getByRole('button', { name: 'Command target: @Alpha' })).toBeTruthy();
    await fireEvent.click(view.getByRole('button', { name: 'Command target: @Alpha' }));
    await fireEvent.mouseDown(view.getByRole('option', { name: /Beta/ }));
    expect(view.getByRole('button', { name: 'Command target: @Beta' })).toBeTruthy();
    expect(view.getByRole('textbox')).toBe(editor);
  });
  it('G59 delivery results remain bound to the captured session and recipient', async () => {
    let resolve!: () => void;
    fixture.invoke.mockImplementation(() => new Promise<void>(done => { resolve = done; }));
    const delivery = vi.fn();
    const view = render(CommandBar, { sessionId: 's', agents, targetId: 'beta', ondelivery: delivery });
    await type(view.getByRole('textbox'), 'Captured recipient'); await fireEvent.click(view.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(delivery).toHaveBeenCalledWith('s', 'beta', 'Sending'));
    await view.rerender({ sessionId: 'other', targetId: 'alpha' }); resolve();
    await waitFor(() => expect(delivery).toHaveBeenCalledWith('s', 'beta', 'Sent'));
    expect(delivery).not.toHaveBeenCalledWith('other', expect.anything(), expect.anything());
  });
});
