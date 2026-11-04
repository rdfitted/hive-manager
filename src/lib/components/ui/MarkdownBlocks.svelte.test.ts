import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import MarkdownBlocks from './MarkdownBlocks.svelte';

const fixture = vi.hoisted(() => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: fixture.openUrl }));
const { readFileSync } = await vi.importActual<{ readFileSync(path: string, encoding: 'utf8'): string }>('node:fs');
afterEach(() => { cleanup(); vi.clearAllMocks(); document.querySelector('[data-markdown-test]')?.remove(); });

describe('shared Markdown blocks', () => {
  it('keeps Knowledge and non-HTML fences unchanged; only opted-in HTML offers a preview', async () => {
    const source = '```html\n<h1>Passive report</h1>\n```\n\n```javascript\nconsole.log(1)\n```';
    const view = render(MarkdownBlocks, { source });
    expect(view.queryByRole('button', { name: 'Preview HTML' })).toBeNull();
    expect(view.container.querySelector('iframe')).toBeNull();
    expect(view.container.querySelector('pre code')?.textContent).toBe('<h1>Passive report</h1>');
    await view.rerender({ source, allowHtmlPreview: true });
    expect(view.getAllByRole('button', { name: 'Preview HTML' })).toHaveLength(1);
    expect(view.container.querySelector('iframe')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Preview HTML' }));
    expect(view.getByTitle('HTML preview')).toBeTruthy();
    expect(view.container.querySelector('pre code')?.textContent).toBe('<h1>Passive report</h1>');
  });

  it('renders headings, inline styles, fenced code, quotes, rules and nested numbered lists', () => {
    const view = render(MarkdownBlocks, { source: '# **Title**\n\nUse *care* and `code`.\n\n3. **First**\n   - child\n     7. **Nested**\n\n4. **Second**\n\n> quote\n\n---\n\n```html\n<b>literal code</b>\n```' });
    expect(view.getByRole('heading', { level: 1, name: 'Title' }).querySelector('strong')).toBeTruthy();
    expect(view.container.querySelector('em')?.textContent).toBe('care');
    expect(view.container.querySelector('p code')?.textContent).toBe('code');
    const lists = view.container.querySelectorAll('ol');
    expect(lists).toHaveLength(2);
    expect(lists[0].start).toBe(3);
    expect(lists[0].children).toHaveLength(2);
    expect(lists[1].start).toBe(7);
    expect(lists[1].closest('ul')?.closest('li')?.querySelector('strong')?.textContent).toBe('First');
    expect(view.container.querySelector('blockquote')?.textContent).toBe('quote');
    expect(view.container.querySelector('hr')).toBeTruthy();
    expect(view.getByRole('region', { name: 'Code block: html' }).querySelector('code')?.textContent).toBe('<b>literal code</b>');
    expect(view.container.querySelector('b')).toBeNull();
  });

  it('keeps wide tables and unwrapped code in keyboard-accessible message-local scroll containers', () => {
    const style = document.createElement('style'); style.dataset.markdownTest = '';
    style.textContent = readFileSync('src/lib/components/ui/MarkdownBlocks.svelte', 'utf8').split('<style>')[1].split('</style>')[0];
    document.head.append(style);
    const view = render(MarkdownBlocks, { source: '| Name | Value |\n| :--- | ---: |\n| **one** | 1234567890 |\n\n```txt\nlong unwrapped line\n```' });
    const region = view.getByRole('region', { name: 'Markdown table' });
    expect(region.tabIndex).toBe(0);
    expect(getComputedStyle(region).overflow).toBe('auto');
    expect(getComputedStyle(region).maxWidth).toBe('100%');
    expect(within(region).getByRole('columnheader', { name: 'Value' }).style.textAlign).toBe('right');
    expect(within(region).getByRole('cell', { name: 'one' }).querySelector('strong')).toBeTruthy();
    const code = view.getByRole('region', { name: 'Code block: txt' });
    expect(code.tabIndex).toBe(0);
    expect(getComputedStyle(code).overflow).toBe('auto');
    expect(getComputedStyle(code).whiteSpace).toBe('pre');
  });

  it('opens only http/https links with the native opener and cancels in-app navigation', async () => {
    const view = render(MarkdownBlocks, { source: '[HTTPS](https://example.com/docs) [HTTP](http://example.com/) [script](javascript:alert(1)) [data](data:text/html,hello)' });
    expect(view.getAllByRole('link')).toHaveLength(2);
    for (const [name, href] of [['HTTPS', 'https://example.com/docs'], ['HTTP', 'http://example.com/']]) {
      const link = view.getByRole('link', { name });
      const click = new MouseEvent('click', { bubbles: true, cancelable: true });
      link.dispatchEvent(click);
      expect(click.defaultPrevented).toBe(true);
      await waitFor(() => expect(fixture.openUrl).toHaveBeenCalledWith(href));
    }
    expect(fixture.openUrl).toHaveBeenCalledTimes(2);
    fixture.openUrl.mockRejectedValueOnce(new Error('unavailable opener'));
    await fireEvent.click(view.getByRole('link', { name: 'HTTPS' }));
    expect(view.container.textContent).toContain('[data](data:text/html,hello)');
  });

  it.each([
    { activation: 'primary click', type: 'click', button: 0, detail: 1 },
    { activation: 'middle click', type: 'auxclick', button: 1, detail: 1 },
    { activation: 'keyboard activation', type: 'click', button: 0, detail: 0 },
  ])('cancels $activation navigation and invokes the native opener exactly once', async ({ activation, type, button, detail }) => {
    const view = render(MarkdownBlocks, { source: '[docs](https://example.com/docs)' });
    const link = view.getByRole('link', { name: 'docs' });
    if (activation === 'keyboard activation') {
      link.focus();
      await fireEvent.keyDown(link, { key: 'Enter', code: 'Enter' });
      // jsdom does not synthesize the browser's Enter activation click. Deliver
      // that click (detail 0) explicitly; no key handler should open it twice.
      expect(fixture.openUrl).not.toHaveBeenCalled();
    }
    const event = new MouseEvent(type, { bubbles: true, cancelable: true, button, detail });
    expect(link.dispatchEvent(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    await waitFor(() => expect(fixture.openUrl).toHaveBeenCalledTimes(1));
    expect(fixture.openUrl).toHaveBeenCalledWith('https://example.com/docs');
    if (activation === 'keyboard activation') {
      await fireEvent.keyUp(link, { key: 'Enter', code: 'Enter' });
      expect(document.activeElement).toBe(link);
      expect(fixture.openUrl).toHaveBeenCalledTimes(1);
    }
  });

  it('cancels other auxiliary navigation without opening a second native window', () => {
    const view = render(MarkdownBlocks, { source: '[docs](https://example.com/docs)' });
    const event = new MouseEvent('auxclick', { bubbles: true, cancelable: true, button: 2 });
    view.getByRole('link', { name: 'docs' }).dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(fixture.openUrl).not.toHaveBeenCalled();
  });
});
