import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import AgentPaneBodyHarness from './AgentPaneBodyTestHarness.svelte';
import TerminalStatusLine from './TerminalStatusLine.svelte';
import type { PaneMode } from './paneMode';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('phosphor-svelte', () => ({ ArrowUp: () => {}, ArrowUpRight: () => {}, Plus: () => {} }));

afterEach(cleanup);

describe('AgentPaneBody', () => {
  it('renders one terminal at the same call site and projects the status above the composer in Chat', async () => {
    const dispose = vi.fn();
    const setup = vi.fn(() => dispose);
    const terminal = createRawSnippet(() => ({ render: () => '<div data-testid="terminal">Scrollback</div>', setup }));
    const statusLine = createRawSnippet(() => ({ render: () => '<span>Live output</span>' }));
    const view = render(AgentPaneBodyHarness, { mode: 'chat', terminal, statusLine });
    const node = view.getByTestId('terminal');
    const terminalLayer = node.parentElement!;
    const chatLayer = view.container.querySelector<HTMLElement>('.chat-layer')!;
    const chatNode = chatLayer.firstElementChild;

    for (const mode of ['chat', 'split', 'terminal', 'raw', 'chat'] satisfies PaneMode[]) {
      await view.rerender({ mode });
      expect(view.container.querySelector('.agent-pane-body')?.getAttribute('data-mode')).toBe(mode);
      expect(view.getAllByTestId('terminal')).toEqual([node]);
      expect(node.parentElement).toBe(terminalLayer);
      expect(node.textContent).toBe('Scrollback');
      expect(chatLayer.firstElementChild).toBe(chatNode);
      expect(terminalLayer.inert).toBe(mode === 'chat' || mode === 'raw');
      expect(chatLayer.inert).toBe(mode === 'terminal');
      expect(setup).toHaveBeenCalledTimes(1);
      expect(dispose).not.toHaveBeenCalled();
      const status = view.queryByText('Live output');
      if (mode === 'chat') expect(status?.nextElementSibling?.getAttribute('aria-label')).toBe('Message composer');
      else expect(status).toBeNull();
    }
    view.unmount();
    expect(dispose).toHaveBeenCalledTimes(1);
  });
});

describe('TerminalStatusLine', () => {
  it('shows output and delegates the Split action, falling back when the line is cleared', async () => {
    const onOpenSplit = vi.fn();
    const view = render(TerminalStatusLine, { sessionId: 'test-session', agentId: 'test-agent', line: '<parsed output>', onOpenSplit });
    expect(view.getByText('<parsed output>').tagName).toBe('SPAN');
    await fireEvent.click(view.getByRole('button', { name: 'Open Split' }));
    expect(onOpenSplit).toHaveBeenCalledTimes(1);
    await view.rerender({ line: '', onOpenSplit: undefined, compact: true });
    expect(view.getByText('No activity yet')).toBeTruthy();
    expect(view.queryByRole('button', { name: 'Open Split' })).toBeNull();
  });
});
