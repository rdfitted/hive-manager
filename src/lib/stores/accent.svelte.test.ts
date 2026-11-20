import { cleanup, render } from '@testing-library/svelte';
import { compile } from 'svelte/compiler';
import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';
import HiveMark from '../components/ui/HiveMark.svelte';
import { createAccentStore } from './accent';

const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');

const storageKey = 'hive-manager-accent';
const styles: HTMLStyleElement[] = [];

function installCSS(css: string) {
  const style = document.createElement('style');
  style.textContent = css;
  document.head.append(style);
  styles.push(style);
}

function computedFill(path: SVGPathElement) {
  const style = getComputedStyle(path);
  let fill = style.fill || path.getAttribute('fill') || '';
  if (fill.toLowerCase() === 'currentcolor') fill = style.color;
  // jsdom exposes the CSSOM values without substituting custom property aliases.
  const root = getComputedStyle(document.documentElement);
  const seen = new Set<string>();
  for (let alias = /^var\((--[\w-]+)\)$/.exec(fill); alias; alias = /^var\((--[\w-]+)\)$/.exec(fill)) {
    const [, name] = alias;
    if (seen.has(name)) throw new Error(`Circular fill alias: ${name}`);
    seen.add(name);
    fill = root.getPropertyValue(name).trim();
  }
  return fill;
}

afterEach(() => {
  cleanup();
  styles.splice(0).forEach(style => style.remove());
  vi.restoreAllMocks();
  localStorage.removeItem(storageKey);
  delete document.documentElement.dataset.accent;
});

describe('G-accent', () => {
  it('applies the selected accent, changes HiveMark fill and restores the stored choice', () => {
    const { getByRole } = render(HiveMark, { title: 'Hive Manager' });
    const mark = getByRole('img', { name: 'Hive Manager' });
    const path = mark.querySelector('path')!;
    const markFile = 'src/lib/components/ui/HiveMark.svelte';
    const css = compile(readFileSync(markFile, 'utf8'), {
      filename: markFile, cssHash: () => 'g-accent-mark',
    }).css?.code;
    expect(css).toBeTruthy();
    mark.classList.add('g-accent-mark');
    installCSS(readFileSync('src/lib/styles/lattice-tokens.css', 'utf8').replace(/^@import[^;]+;/, ''));
    installCSS(readFileSync('src/lib/styles/accent.css', 'utf8'));
    installCSS(css!);
    const accent = createAccentStore();

    expect(document.documentElement.dataset.accent).toBe('honey');
    expect(computedFill(path)).toBe('#e5b97c');
    accent.toggle();
    expect(document.documentElement.dataset.accent).toBe('sky');
    expect(computedFill(path)).toBe('#5cc1ff');
    expect(localStorage.getItem(storageKey)).toBe('sky');
    expect(get(createAccentStore())).toBe('sky');

    accent.toggle();
    expect(document.documentElement.dataset.accent).toBe('honey');
    expect(computedFill(path)).toBe('#e5b97c');
    expect(localStorage.getItem(storageKey)).toBe('honey');
  });

  it('uses honey for an invalid stored selection', () => {
    localStorage.setItem(storageKey, 'invalid');
    const accent = createAccentStore();
    expect(get(accent)).toBe('honey');
    expect(document.documentElement.dataset.accent).toBe('honey');
  });

  it('applies an explicit selection when storage reads and writes fail', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('Storage blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('Storage blocked'); });
    const accent = createAccentStore();

    expect(get(accent)).toBe('honey');
    accent.set('sky');
    expect(get(accent)).toBe('sky');
    expect(document.documentElement.dataset.accent).toBe('sky');
  });
});
