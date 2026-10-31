import { describe, expect, it } from 'vitest';
import { parse } from 'svelte/compiler';
import { overlayAllowlist } from './overlayAllowlist';

const sources = import.meta.glob('/src/**/*.svelte', { eager: true, query: '?raw', import: 'default' });

function hasOverlayAttribute(source: string): boolean {
  let found = false;
  function walk(value: unknown) {
    if (!value || typeof value !== 'object') return;
    const node = value as Record<string, unknown>;
    if (node.type === 'Attribute') {
      if (node.name === 'aria-modal') found = true;
      if (node.name === 'role' && /dialog/i.test(source.slice(node.start as number, node.end as number))) found = true;
    }
    for (const child of Object.values(node)) {
      if (Array.isArray(child)) child.forEach(walk);
      else if (child && typeof child === 'object') walk(child);
    }
  }
  walk(parse(source, { modern: true }));
  return found;
}

describe('G1 overlay ownership', () => {
  it('allows an aria-modal query selector string', () => {
    expect(hasOverlayAttribute(`<script>document.querySelector('[aria-modal="true"]');</script><div />`)).toBe(false);
  });
  it('detects dynamic dialog role and aria-modal attributes', () => {
    expect(hasOverlayAttribute(`<div role={modal ? 'dialog' : undefined} />`)).toBe(true);
    expect(hasOverlayAttribute(`<div aria-modal={modal} />`)).toBe(true);
    expect(hasOverlayAttribute(`<div role="alertdialog" />`)).toBe(true);
  });
  it('requires every overlay outside the migration allowlist to use ui/Dialog', () => {
    const violations = Object.entries(sources).filter(([path, source]) => {
      const relpath = path.slice(1);
      return relpath !== 'src/lib/components/ui/Dialog.svelte'
        && !overlayAllowlist.includes(relpath)
        && hasOverlayAttribute(source as string);
    }).map(([path]) => path.slice(1));
    expect(violations).toEqual([]);
  });
});
