import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import HtmlPreview, { PREVIEW_CSP, previewDocument } from './HtmlPreview.svelte';

afterEach(cleanup);

describe('chat HTML preview', () => {
  it('creates no document until clicked, uses an empty sandbox and closes explicitly', async () => {
    const view = render(HtmlPreview, { source: '<h1>Orbital report</h1>' });
    expect(view.container.querySelector('iframe')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Preview HTML' }));
    const frame = view.getByTitle('HTML preview') as HTMLIFrameElement;
    expect(frame.getAttribute('sandbox')).toBe('');
    expect(frame.hasAttribute('src')).toBe(false);
    expect(frame.getAttribute('referrerpolicy')).toBe('no-referrer');
    expect(frame.srcdoc).toContain(`<head><meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">`);
    expect(frame.srcdoc).toContain('<h1>Orbital report</h1>');
    expect(view.container.querySelector('h1')).toBeNull();
    await fireEvent.click(view.getByRole('button', { name: 'Close HTML preview' }));
    expect(view.container.querySelector('iframe')).toBeNull();
  });

  it('drops active markup, resource loads and navigation while preserving passive HTML and CSS', () => {
    const value = previewDocument(`<meta http-equiv="refresh" content="0;url=http://127.0.0.1:18800/">
      <base href="http://127.0.0.1:18800/"><script>parent.hacked=true</script>
      <style>p { color: teal; }</style><p style="margin: 2px" onclick="parent.hacked=true">Report</p>
      <a href="javascript:alert(1)" target="_top" ping="http://127.0.0.1:18800/">link</a>
      <form action="http://127.0.0.1:18800/"><button formaction="http://127.0.0.1:18800/">submit</button></form>
      <iframe src="http://127.0.0.1:18800/"></iframe><object data="http://127.0.0.1:18800/"></object>
      <img src="http://127.0.0.1:18800/" srcset="http://127.0.0.1:18800/ 2x" onerror="parent.hacked=true">
      <svg><a href="javascript:alert(1)">svg</a></svg><math><mi>x</mi></math>
      <template><img src="http://127.0.0.1:18800/"></template>`);
    const template = document.createElement('template'); template.innerHTML = value;
    expect(template.content.querySelector('script, base, form, iframe, object, svg, math, template')).toBeNull();
    expect(template.content.querySelector('[onclick], [onerror], [href], [src], [srcset], [action], [target], [ping]')).toBeNull();
    expect(template.content.querySelectorAll('meta[http-equiv]')).toHaveLength(1);
    expect(template.content.querySelector('style')?.textContent).toContain('color: teal');
    expect(template.content.querySelector('p')?.getAttribute('style')).toBe('margin: 2px');
    expect(template.content.querySelector('a')?.textContent).toBe('link');
    expect(PREVIEW_CSP).toContain("connect-src 'none'");
    expect(PREVIEW_CSP).toContain("script-src 'none'");
  });

  it('closes a preview when its source changes', async () => {
    const view = render(HtmlPreview, { source: '<p>First</p>' });
    await fireEvent.click(view.getByRole('button', { name: 'Preview HTML' }));
    await view.rerender({ source: '<p>Second</p>' });
    expect(view.container.querySelector('iframe')).toBeNull();
  });
});
