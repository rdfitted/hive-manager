export type MarkdownBlock =
  | { type: 'heading'; level: number; text: string }
  | { type: 'paragraph'; text: string }
  | { type: 'quote'; text: string }
  | { type: 'code'; language: string; text: string }
  | { type: 'list'; ordered: boolean; items: string[]; start?: number; children?: Record<number, MarkdownBlock[]> }
  | { type: 'table'; headers: string[]; align: ('left' | 'center' | 'right' | null)[]; rows: string[][] }
  | { type: 'rule' };

export type MarkdownInline =
  | { type: 'text'; text: string }
  | { type: 'strong'; text: string }
  | { type: 'emphasis'; text: string }
  | { type: 'code'; text: string }
  | { type: 'link'; text: string; href: string }
  | { type: 'wikilink'; text: string };

function safeLink(value: string): string | null {
  try {
    const url = new URL(value);
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.toString() : null;
  } catch {
    return null;
  }
}

/** Parse a deliberately small inline subset without ever emitting HTML, in O(n) work. */
export function parseInlineMarkdown(source: string): MarkdownInline[] {
  const tokens: MarkdownInline[] = [];
  // Cache each candidate's exclusive end in one reverse pass. Failed openers
  // must not search the same suffix again (notably '[' and unfinished links).
  // Entries for '(' cache URL ends for the '[' candidate to its left.
  const ends = new Uint32Array(source.length);
  let newline = source.length;
  let backtick = source.length;
  let star = source.length;
  let secondStar = source.length;
  let underscore = source.length;
  let secondUnderscore = source.length;
  let bracket = source.length;
  let parenthesis = source.length;
  for (let index = source.length - 1; index >= 0; index--) {
    const character = source.charAt(index);
    if (character === '\n') newline = index;
    else if (character === '`') {
      if (backtick > index + 1 && backtick < newline) ends[index] = backtick + 1;
      backtick = index;
    } else if (character === '*' || character === '_') {
      const next = character === '*' ? star : underscore;
      const second = character === '*' ? secondStar : secondUnderscore;
      if (next === index + 1 && second > index + 2 && second < newline && source.charAt(second + 1) === character) {
        ends[index] = second + 2;
      } else if (next > index + 1 && next < newline) ends[index] = next + 1;
      if (character === '*') { secondStar = star; star = index; }
      else { secondUnderscore = underscore; underscore = index; }
    } else if (character === ']') bracket = index;
    else if (character === ')') parenthesis = index;
    else if (character === '(') {
      if (parenthesis > index + 1 && parenthesis < newline) ends[index] = parenthesis + 1;
    } else if (character === '[' && bracket < newline) {
      // Links precede wiki links, matching the existing inline subset.
      if (bracket > index + 1 && source.charAt(bracket + 1) === '(' && ends[bracket + 1]) {
        ends[index] = ends[bracket + 1];
      } else if (source.charAt(index + 1) === '[' && bracket > index + 2 && source.charAt(bracket + 1) === ']') {
        ends[index] = bracket + 2;
      }
    }
  }
  let cursor = 0;

  for (let start = 0; start < source.length; start++) {
    const end = ends[start];
    if (!end || source.charAt(start) === '(') continue;
    if (start > cursor) tokens.push({ type: 'text', text: source.slice(cursor, start) });
    const raw = source.slice(start, end);

    if (raw.startsWith('`')) {
      tokens.push({ type: 'code', text: raw.slice(1, -1) });
    } else if (raw.startsWith('**') || raw.startsWith('__')) {
      tokens.push({ type: 'strong', text: raw.slice(2, -2) });
    } else if (raw.startsWith('[[')) {
      tokens.push({ type: 'wikilink', text: raw.slice(2, -2) });
    } else if (raw.startsWith('[')) {
      const linkMatch = raw.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
      const href = linkMatch ? safeLink(linkMatch[2].trim()) : null;
      if (linkMatch && href) {
        tokens.push({ type: 'link', text: linkMatch[1], href });
      } else {
        tokens.push({ type: 'text', text: raw });
      }
    } else {
      tokens.push({ type: 'emphasis', text: raw.slice(1, -1) });
    }
    cursor = end;
    start = end - 1;
  }

  if (cursor < source.length) tokens.push({ type: 'text', text: source.slice(cursor) });
  return tokens.length > 0 ? tokens : [{ type: 'text', text: source }];
}

function tableCells(line: string): string[] {
  const row = line.trim().replace(/^\|/, '').replace(/(?<!\\)\|$/, '');
  const cells: string[] = [];
  let cell = '';
  let code = false;
  for (let index = 0; index < row.length; index++) {
    const character = row[index];
    if (character === '\\' && row[index + 1] === '|') { cell += '|'; index++; }
    else if (character === '`') { code = !code; cell += character; }
    else if (character === '|' && !code) { cells.push(cell.trim()); cell = ''; }
    else cell += character;
  }
  cells.push(cell.trim());
  return cells;
}

function isTableStart(line: string, next = ''): boolean {
  if (!line.includes('|')) return false;
  const headers = tableCells(line);
  const separator = tableCells(next);
  return headers.length === separator.length && separator.every((cell) => /^:?-+:?$/.test(cell));
}

function listItem(line: string) {
  const match = line.match(/^(\s*)(?:([-*+])|(\d+)[.)])\s+(.*)$/);
  if (!match) return null;
  return {
    indent: match[1].replace(/\t/g, '    ').length,
    contentIndent: match[0].slice(0, match[0].length - match[4].length).replace(/\t/g, '    ').length,
    ordered: match[3] !== undefined,
    number: match[3] ? Number(match[3]) : 1,
    text: match[4],
  };
}

function isBlockStart(line: string, next = ''): boolean {
  return (
    /^#{1,6}\s+/.test(line) ||
    /^```/.test(line) ||
    /^>\s?/.test(line) ||
    /^\s*[-*+]\s+/.test(line) ||
    /^\s*\d+[.)]\s+/.test(line) ||
    /^\s*(?:---+|___+|\*\*\*+)\s*$/.test(line) ||
    isTableStart(line, next)
  );
}

/** A small, safe block-level markdown parser. Svelte escapes every text field. */
export function parseMarkdown(source: string, depth = 0): MarkdownBlock[] {
  // Bound recursion for deeply indented, untrusted output; retain it as readable text.
  if (depth >= 32) return [{ type: 'paragraph', text: source }];
  const lines = source.replace(/\r\n?/g, '\n').split('\n');
  const blocks: MarkdownBlock[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) {
      index += 1;
      continue;
    }

    const fence = line.match(/^```\s*([^\s`]*)/);
    if (fence) {
      const language = fence[1] ?? '';
      const code: string[] = [];
      index += 1;
      while (index < lines.length && !/^```\s*$/.test(lines[index])) {
        code.push(lines[index]);
        index += 1;
      }
      if (index < lines.length) index += 1;
      blocks.push({ type: 'code', language, text: code.join('\n') });
      continue;
    }

    const heading = line.match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (heading) {
      blocks.push({ type: 'heading', level: heading[1].length, text: heading[2] });
      index += 1;
      continue;
    }

    if (/^\s*(?:---+|___+|\*\*\*+)\s*$/.test(line)) {
      blocks.push({ type: 'rule' });
      index += 1;
      continue;
    }

    if (/^>\s?/.test(line)) {
      const quote: string[] = [];
      while (index < lines.length && /^>\s?/.test(lines[index])) {
        quote.push(lines[index].replace(/^>\s?/, ''));
        index += 1;
      }
      blocks.push({ type: 'quote', text: quote.join('\n') });
      continue;
    }

    if (isTableStart(line, lines[index + 1])) {
      const headers = tableCells(line);
      const align = tableCells(lines[index + 1]).map((cell): 'left' | 'center' | 'right' | null =>
        cell.startsWith(':') && cell.endsWith(':') ? 'center' : cell.endsWith(':') ? 'right' : cell.startsWith(':') ? 'left' : null);
      const rows: string[][] = [];
      index += 2;
      while (index < lines.length && lines[index].trim() && !isBlockStart(lines[index], lines[index + 1])) {
        const cells = tableCells(lines[index++]);
        rows.push(headers.map((_, column) => cells[column] ?? ''));
      }
      blocks.push({ type: 'table', headers, align, rows });
      continue;
    }

    const first = listItem(line);
    if (first) {
      const items: string[] = [];
      const children: Record<number, MarkdownBlock[]> = {};
      while (index < lines.length) {
        const item = listItem(lines[index]);
        if (!item || item.indent !== first.indent || item.ordered !== first.ordered) break;
        items.push(item.text);
        index += 1;
        const continuation: string[] = [];
        while (index < lines.length) {
          if (!lines[index].trim()) {
            let next = index + 1;
            while (next < lines.length && !lines[next].trim()) next++;
            const sibling = next < lines.length ? listItem(lines[next]) : null;
            if (sibling && sibling.indent === first.indent && sibling.ordered === first.ordered) { index = next; break; }
            if (next >= lines.length || (lines[next].match(/^\s*/)?.[0].replace(/\t/g, '    ').length ?? 0) <= first.indent) break;
          } else {
            const indent = lines[index].match(/^\s*/)?.[0].replace(/\t/g, '    ').length ?? 0;
            if (indent <= first.indent) break;
          }
          const expanded = lines[index++].replace(/^\t+/, (tabs) => '    '.repeat(tabs.length));
          continuation.push(expanded.slice(Math.min(item.contentIndent, expanded.match(/^ */)?.[0].length ?? 0)));
        }
        if (continuation.some((entry) => entry.trim())) children[items.length - 1] = parseMarkdown(continuation.join('\n'), depth + 1);
      }
      blocks.push({ type: 'list', ordered: first.ordered, items,
        ...(first.ordered && first.number !== 1 ? { start: first.number } : {}),
        ...(Object.keys(children).length ? { children } : {}),
      });
      continue;
    }

    const paragraph = [line.trim()];
    index += 1;
    while (index < lines.length && lines[index].trim() && !isBlockStart(lines[index], lines[index + 1])) {
      paragraph.push(lines[index].trim());
      index += 1;
    }
    blocks.push({ type: 'paragraph', text: paragraph.join(' ') });
  }

  return blocks;
}
