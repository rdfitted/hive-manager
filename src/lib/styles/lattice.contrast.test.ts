import { describe, expect, it, vi } from 'vitest';

// Match lattice.nested.test.ts: the disabled CSS pipeline stubs ?raw imports,
// and a narrow native-module type keeps Node ambient types out of the app.
const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');

const tokens = Object.fromEntries(
  Array.from(readFileSync('src/lib/styles/lattice-tokens.css', 'utf8').matchAll(/(--[\w-]+):\s*([^;]+);/g),
    ([, name, value]) => [name, value.trim()]),
);

function resolveToken(name: string, seen = new Set<string>()): string {
  if (seen.has(name)) throw new Error(`Circular token alias: ${name}`);
  seen.add(name);
  const value = tokens[name];
  if (!value) throw new Error(`Missing token: ${name}`);
  const alias = /^var\((--[\w-]+)\)$/.exec(value);
  return alias ? resolveToken(alias[1], seen) : value;
}

function luminance(name: string): number {
  const color = resolveToken(name);
  if (!/^#[\da-f]{6}$/i.test(color)) throw new Error(`Unsupported color for ${name}: ${color}`);
  const channels = [1, 3, 5].map((offset) => {
    const channel = parseInt(color.slice(offset, offset + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrast(foreground: string, background: string): number {
  const a = luminance(foreground);
  const b = luminance(background);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

describe('canceled status text contrast', () => {
  it.each(['void', 'chrome', 'panel', 'raised', 'elevated'])('has at least 4.5:1 contrast on %s', (surface) => {
    expect(contrast('--status-canceled', `--bg-${surface}`)).toBeGreaterThanOrEqual(4.5);
  });

  it('remains dimmer than queued status text', () => {
    expect(luminance('--status-canceled')).toBeLessThan(luminance('--status-queued'));
  });
});
