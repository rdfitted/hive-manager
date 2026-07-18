import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import KnowledgePagePreview from './KnowledgePagePreview.svelte';
import type { KnowledgePage } from '$lib/knowledge/types';

const page: KnowledgePage = {
  id: 'patterns/safe-preview',
  title: 'Safe Preview',
  folder: 'patterns',
  path: 'patterns/safe-preview.md',
  last_updated: '2026-07-18T00:00:00Z',
  truncated: true,
  content: `# Safe **Preview**

Keep <script>alert('never')</script> visible as text.

[Docs](https://example.com/docs) and [unsafe](javascript:alert(1)).`,
};

afterEach(cleanup);

describe('KnowledgePagePreview', () => {
  it('renders structured markdown as escaped, read-only content', async () => {
    const onClose = vi.fn();
    const { container, getByRole, getByText } = render(KnowledgePagePreview, {
      props: {
        selectedId: page.id,
        page,
        loading: false,
        error: null,
        onClose,
        onRetry: vi.fn(),
      },
    });

    expect(container.querySelector('script')).toBeNull();
    expect(container.textContent).toContain("<script>alert('never')</script>");
    expect(getByRole('link', { name: 'Docs' }).getAttribute('href')).toBe('https://example.com/docs');
    expect(container.querySelector('a[href^="javascript:"]')).toBeNull();
    expect(getByText('Preview capped for safety')).toBeTruthy();

    await fireEvent.click(getByRole('button', { name: 'Close page preview' }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('offers a retry action for preview errors', async () => {
    const onRetry = vi.fn();
    const { getByRole } = render(KnowledgePagePreview, {
      props: {
        selectedId: page.id,
        page: null,
        loading: false,
        error: 'Preview failed',
        onClose: vi.fn(),
        onRetry,
      },
    });

    await fireEvent.click(getByRole('button', { name: 'Try again' }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it('moves focus into the preview and restores the invoking control on close', async () => {
    const returnFocus = document.createElement('button');
    document.body.append(returnFocus);
    returnFocus.focus();
    const onClose = vi.fn();
    const { getByRole } = render(KnowledgePagePreview, {
      props: {
        selectedId: page.id,
        page,
        loading: false,
        error: null,
        onClose,
        onRetry: vi.fn(),
        returnFocus,
      },
    });

    const closeButton = getByRole('button', { name: 'Close page preview' });
    expect(document.activeElement).toBe(closeButton);

    await fireEvent.click(closeButton);
    expect(onClose).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(returnFocus);
    returnFocus.remove();
  });
});
