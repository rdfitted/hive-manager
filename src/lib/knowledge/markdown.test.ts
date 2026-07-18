import { describe, expect, it } from 'vitest';
import { parseInlineMarkdown, parseMarkdown } from './markdown';

describe('knowledge markdown parser', () => {
  it('parses common wiki blocks without producing raw HTML', () => {
    const blocks = parseMarkdown(`# Title

Paragraph with <script>alert('x')</script>.

- first
- second

> note

\`\`\`ts
const answer = 42;
\`\`\``);

    expect(blocks).toEqual([
      { type: 'heading', level: 1, text: 'Title' },
      { type: 'paragraph', text: "Paragraph with <script>alert('x')</script>." },
      { type: 'list', ordered: false, items: ['first', 'second'] },
      { type: 'quote', text: 'note' },
      { type: 'code', language: 'ts', text: 'const answer = 42;' },
    ]);
  });

  it('keeps inline rendering structured and rejects executable links', () => {
    expect(parseInlineMarkdown('Use **care** with `fetch` and [[safe-pattern]].')).toEqual([
      { type: 'text', text: 'Use ' },
      { type: 'strong', text: 'care' },
      { type: 'text', text: ' with ' },
      { type: 'code', text: 'fetch' },
      { type: 'text', text: ' and ' },
      { type: 'wikilink', text: 'safe-pattern' },
      { type: 'text', text: '.' },
    ]);
    expect(parseInlineMarkdown('[unsafe](javascript:alert(1))')).toEqual([
      { type: 'text', text: '[unsafe](javascript:alert(1)' },
      { type: 'text', text: ')' },
    ]);
  });
});
