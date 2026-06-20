import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';
import ToolRenderHost from './ToolRenderHost.svelte';
import type { ConversationMessage } from '$lib/stores/conversations';
import {
  rendererCount,
  registerBuiltinRenderers,
  resetBuiltinRegistration,
  clearAllRenderers,
} from './index';

// @testing-library/svelte mounts real Svelte 5 components in jsdom (this file
// matches **/*.svelte.test.ts -> jsdom per vite.config). Tauri APIs are never
// touched here, but conversations.ts imports @tauri-apps/api/event at module
// load, so stub it to avoid a real Tauri runtime.
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

function message(over: Partial<ConversationMessage>): ConversationMessage {
  return {
    timestamp: '2026-06-19T00:00:00Z',
    from: 'queen',
    content: 'plain content',
    ...over,
  };
}

afterEach(() => {
  cleanup();
});

describe('ToolRenderHost', () => {
  it('mounts the Diff widget for renderer:"diff" (criterion 2)', () => {
    const msg = message({
      renderer: 'diff',
      data: { unified: '@@ -1 +1 @@\n+added line\n-removed line\n context' },
    });
    const { container } = render(ToolRenderHost, { props: { message: msg } });

    // A real diff widget renders add/del lines, NOT raw text.
    const addLine = container.querySelector('.diff-line.add');
    const delLine = container.querySelector('.diff-line.del');
    expect(addLine).not.toBeNull();
    expect(delLine).not.toBeNull();
    expect(addLine?.textContent).toContain('added line');
  });

  it('renders the <pre> JSON fallback for an unknown renderer (criterion 3)', () => {
    const msg = message({ renderer: 'future-widget', data: { a: 1, b: 'two' } });
    const { container } = render(ToolRenderHost, { props: { message: msg } });

    const fallback = container.querySelector('pre.tool-render-fallback');
    expect(fallback).not.toBeNull();
    expect(fallback?.textContent).toContain('"a": 1');
    expect(fallback?.textContent).toContain('"b": "two"');
    // No widget mounted.
    expect(container.querySelector('.diff-line')).toBeNull();
  });

  it('mounts the Chart widget for renderer:"chart" with inline SVG bars', () => {
    const msg = message({
      renderer: 'chart',
      data: { title: 'Builds', labels: ['pass', 'fail'], values: [12, 3] },
    });
    const { container, getByText } = render(ToolRenderHost, { props: { message: msg } });

    expect(getByText('Builds')).toBeTruthy();
    expect(container.querySelector('svg')).not.toBeNull();
    expect(container.querySelectorAll('.chart-bar')).toHaveLength(2);
    expect(container.querySelector('pre.tool-render-fallback')).toBeNull();
  });

  it('invokes onapprove with the actionId when the Approval card button is clicked', async () => {
    const msg = message({
      renderer: 'approval',
      from: 'queen',
      data: { title: 'Deploy?', actionId: 'act-42' },
    });
    const onapprove = vi.fn();
    const { container } = render(ToolRenderHost, {
      props: { message: msg, onapprove },
    });

    const btn = container.querySelector('[data-testid="approve"]') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    await fireEvent.click(btn);

    expect(onapprove).toHaveBeenCalledTimes(1);
    const detail = onapprove.mock.calls[0][0] as { actionId?: string };
    expect(detail.actionId).toBe('act-42');
  });

  it('registerBuiltinRenderers is idempotent', () => {
    // Built-ins were registered during the renders above. Re-running must not
    // grow the registry.
    const before = rendererCount();
    registerBuiltinRenderers();
    registerBuiltinRenderers();
    expect(rendererCount()).toBe(before);

    // And from a fully-cleared slate, a single re-registration restores exactly
    // the four built-ins.
    clearAllRenderers();
    resetBuiltinRegistration();
    registerBuiltinRenderers();
    registerBuiltinRenderers();
    expect(rendererCount()).toBe(4);
  });
});
