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

  it('parses GFM tables with alignment, escaped pipes, inline code and uneven rows', () => {
    expect(parseMarkdown('Intro\n\n| Name | Value | Note |\n| :--- | ---: | :---: |\n| **one** | 2 | a\\|b |\n| two | `x|y` |\n\nAfter')).toEqual([
      { type: 'paragraph', text: 'Intro' },
      { type: 'table', headers: ['Name', 'Value', 'Note'], align: ['left', 'right', 'center'], rows: [['**one**', '2', 'a|b'], ['two', '`x|y`', '']] },
      { type: 'paragraph', text: 'After' },
    ]);
  });

  it('keeps blank-separated bold numbered items together and nests lists under their parent', () => {
    expect(parseMarkdown('3. **First**\n   - child\n     7. **Nested**\n\n4. **Second**')).toEqual([
      { type: 'list', ordered: true, start: 3, items: ['**First**', '**Second**'], children: {
        0: [{ type: 'list', ordered: false, items: ['child'], children: {
          0: [{ type: 'list', ordered: true, start: 7, items: ['**Nested**'] }],
        } }],
      } },
    ]);
  });

  it.each([
    '#\n**unfinished [link](\n| malformed | table\n| -- |\n<script>literal</script>',
    '```html\n<img src=x onerror=alert(1)>',
    Array.from({ length: 80 }, (_, level) => `${'  '.repeat(level)}- depth ${level}`).join('\n'),
    '[data](data:text/html,hello) [relative](/local) [bad](not a url)',
  ])('keeps malformed or deeply nested output readable without throwing: %s', (source) => {
    expect(() => parseMarkdown(source)).not.toThrow();
    expect(() => parseInlineMarkdown(source)).not.toThrow();
    expect(parseMarkdown(source).length).toBeGreaterThan(0);
    for (const token of parseInlineMarkdown(source)) {
      if (token.type === 'link') expect(new URL(token.href).protocol).toMatch(/^https?:$/);
    }
  });
});
