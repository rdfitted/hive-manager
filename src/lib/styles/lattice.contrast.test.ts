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
  if (alias) return resolveToken(alias[1], seen);
  const mix = /^color-mix\(in srgb, var\((--[\w-]+)\) (\d+)%, var\((--[\w-]+)\)\)$/.exec(value);
  if (mix) {
    const foreground = resolveToken(mix[1], new Set(seen));
    const background = resolveToken(mix[3], new Set(seen));
    const weight = Number(mix[2]) / 100;
    return '#' + [1, 3, 5].map((offset) => Math.round(
      parseInt(foreground.slice(offset, offset + 2), 16) * weight
      + parseInt(background.slice(offset, offset + 2), 16) * (1 - weight),
    ).toString(16).padStart(2, '0')).join('');
  }
  return value;
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

// These are the actual UI surfaces, including field, track and compatibility roles.
const surfaces = [
  '--canvas', '--chrome', '--toolbar', '--toolbar-control', '--surface',
  '--surface-raised', '--surface-overlay', '--input', '--track', '--track-active',
  '--accent-tint', '--danger-tint', '--bg-surface', '--bg-sunken', '--color-bg', '--color-surface', '--color-surface-hover',
];

describe('canceled status text contrast', () => {
  // UI status roles deliberately replace terminal literals; D1 stays unchanged.
  it.each(surfaces)('has at least 4.5:1 contrast on %s', (surface) => {
    expect(contrast('--state-canceled', surface)).toBeGreaterThanOrEqual(4.5);
    expect(contrast('--state-queued', surface)).toBeGreaterThanOrEqual(4.5);
  });

  it('remains dimmer than queued status text', () => {
    expect(luminance('--state-canceled')).toBeLessThan(luminance('--state-queued'));
  });
});

describe('UI role text contrast', () => {
  it.each(surfaces)('keeps primary, muted, accent and status text readable on %s', (surface) => {
    for (const foreground of [
      '--text-primary', '--text-muted', '--accent',
      '--status-success', '--status-running', '--status-warning', '--status-error', '--status-blocked',
    ]) expect(contrast(foreground, surface), foreground).toBeGreaterThanOrEqual(4.5);
  });

  it('keeps filled accent labels at 4.5:1', () => {
    expect(contrast('--on-accent', '--accent')).toBeGreaterThanOrEqual(4.5);
  });

  it.each(surfaces)('keeps the focus outline at 3:1 on %s', (surface) => {
    expect(contrast('--focus', surface)).toBeGreaterThanOrEqual(3);
  });

  it('reserves the main accent for shapes and checks its white-label regression', () => {
    expect(contrast('--fitted-whitesmoke', '--fitted-stroke')).toBeGreaterThanOrEqual(4.5);
    expect(contrast('--accent-shape', '--canvas')).toBeGreaterThanOrEqual(3);
  });
});

describe('shared control boundary contrast', () => {
  // Inputs use the canvas inside; outlined buttons/choices can sit on any neutral
  // shell or overlay surface, and card controls use the chrome surface inside.
  it.each(surfaces)
    ('has at least 3:1 contrast against the adjacent %s surface', (surface) => {
      expect(contrast('--border-control', surface)).toBeGreaterThanOrEqual(3);
    });

  it.each([
    ['.lattice-input', 'border'],
    ['.lattice-selectable', 'border'],
    ['.lattice-btn--secondary', '--lattice-btn-ring'],
    ['.lattice-btn--card', '--lattice-btn-ring'],
    ['.lattice-btn--chip', '--lattice-btn-ring'],
  ])('uses the control role for the unfocused %s boundary', (selector, property) => {
    const controls = readFileSync('src/lib/styles/globals/controls.css', 'utf8');
    const rule = controls.split('}').filter((block) => block.includes(`${selector} {`))
      .map((block) => block.slice(block.indexOf(`${selector} {`))).join('\n');
    expect(rule).toContain(`${property}: ${property === 'border' ? '1px solid ' : ''}var(--border-control);`);
  });
});
