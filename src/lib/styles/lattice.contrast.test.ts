import { describe, expect, it, vi } from 'vitest';

// Match lattice.nested.test.ts: the disabled CSS pipeline stubs ?raw imports,
// and a narrow native-module type keeps Node ambient types out of the app.
const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');

function parseTokens(source: string) {
  return Object.fromEntries(Array.from(source.matchAll(/(--[\w-]+):\s*([^;]+);/g),
    ([, name, value]) => [name, value.trim()]));
}

const tokens = parseTokens(readFileSync('src/lib/styles/lattice-tokens.css', 'utf8'));
const accents = readFileSync('src/lib/styles/accent.css', 'utf8');
const palettes = [
  { name: 'default', tokens },
  ...['honey', 'sky'].map(name => {
    const block = new RegExp(`:root\\[data-accent="${name}"\\]\\s*\\{([^}]*)\\}`).exec(accents);
    if (!block) throw new Error(`Missing accent variant: ${name}`);
    return { name, tokens: { ...tokens, ...parseTokens(block[1]) } };
  }),
];

function resolveToken(name: string, palette = tokens, seen = new Set<string>()): string {
  if (seen.has(name)) throw new Error(`Circular token alias: ${name}`);
  seen.add(name);
  const value = palette[name];
  if (!value) throw new Error(`Missing token: ${name}`);
  const alias = /^var\((--[\w-]+)\)$/.exec(value);
  if (alias) return resolveToken(alias[1], palette, seen);
  const mix = /^color-mix\(in srgb, var\((--[\w-]+)\) (\d+)%, var\((--[\w-]+)\)\)$/.exec(value);
  if (mix) {
    const foreground = resolveToken(mix[1], palette, new Set(seen));
    const background = resolveToken(mix[3], palette, new Set(seen));
    const weight = Number(mix[2]) / 100;
    return '#' + [1, 3, 5].map((offset) => Math.round(
      parseInt(foreground.slice(offset, offset + 2), 16) * weight
      + parseInt(background.slice(offset, offset + 2), 16) * (1 - weight),
    ).toString(16).padStart(2, '0')).join('');
  }
  return value;
}

function luminance(name: string, palette = tokens): number {
  const color = resolveToken(name, palette);
  if (!/^#[\da-f]{6}$/i.test(color)) throw new Error(`Unsupported color for ${name}: ${color}`);
  const channels = [1, 3, 5].map((offset) => {
    const channel = parseInt(color.slice(offset, offset + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrast(foreground: string, background: string, palette = tokens): number {
  const a = luminance(foreground, palette);
  const b = luminance(background, palette);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function rgbLuminance(channels: number[]): number {
  const linear = channels.map(value => {
    const channel = value / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
}

function tintContrast(foreground: string, tint: number[], palette: typeof tokens, alpha = 0.10): number {
  const well = resolveToken('--chat-well', palette);
  const composite = tint.map((channel, index) =>
    channel * alpha + parseInt(well.slice(1 + index * 2, 3 + index * 2), 16) * (1 - alpha));
  const a = luminance(foreground, palette), b = rgbLuminance(composite);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

describe.each(palettes)('$name G60 chat text contrast', ({ name, tokens: palette }) => {
  it.each(['--chat-well', '--chat-text', '--chat-text-meta'])('defines %s as six-digit hex', token => {
    expect(palette[token]).toMatch(/^#[\da-f]{6}$/i);
  });
  it('uses the approved near-black chat well', () => {
    expect(palette['--chat-well']).toBe(name === 'sky' ? '#040506' : '#060504');
  });
  it('keeps chat body readable on the well', () => {
    expect(contrast('--chat-text', '--chat-well', palette)).toBeGreaterThanOrEqual(4.5);
  });
  it('keeps chat metadata readable on the well', () => {
    expect(contrast('--chat-text-meta', '--chat-well', palette)).toBeGreaterThanOrEqual(4.5);
  });
  // The Queen card is the accent at the --accent-bg alpha (.14) over the well.
  const queenTint = () => {
    const accent = resolveToken('--accent', palette);
    expect(palette['--accent-bg']).toMatch(/,\s*\.14\)$/);
    return [1, 3, 5].map(offset => parseInt(accent.slice(offset, offset + 2), 16));
  };
  it('keeps chat body readable on the composited Queen tint', () => {
    expect(tintContrast('--chat-text', queenTint(), palette, 0.14)).toBeGreaterThanOrEqual(4.5);
  });
  it('keeps chat metadata readable on the composited Queen tint', () => {
    expect(tintContrast('--chat-text-meta', queenTint(), palette, 0.14)).toBeGreaterThanOrEqual(4.5);
  });
  it('keeps the accent Queen label readable on the composited Queen tint', () => {
    expect(tintContrast('--accent', queenTint(), palette, 0.14)).toBeGreaterThanOrEqual(4.5);
  });
  it('keeps dim metadata readable on the well, the canvas and both glass fills', () => {
    for (const surface of ['--chat-well', '--canvas', '--surface', '--surface-raised']) {
      expect(contrast('--ink-dim', surface, palette), surface).toBeGreaterThanOrEqual(4.5);
    }
    expect(contrast('--ink', '--chat-well', palette)).toBeGreaterThanOrEqual(7);
  });
  it('keeps chat body readable on the composited warning tint', () => {
    expect(tintContrast('--chat-text', [236, 221, 165], palette)).toBeGreaterThanOrEqual(4.5);
  });
  it('keeps chat metadata readable on the composited warning tint', () => {
    expect(tintContrast('--chat-text-meta', [236, 221, 165], palette)).toBeGreaterThanOrEqual(4.5);
  });
});

// These are the actual UI surfaces, including field, track and compatibility roles.
const surfaces = [
  '--canvas', '--chrome', '--toolbar', '--toolbar-control', '--surface',
  '--surface-raised', '--surface-overlay', '--input', '--track', '--track-active',
  '--accent-tint', '--danger-tint', '--bg-surface', '--bg-sunken', '--color-bg', '--color-surface', '--color-surface-hover',
];

describe.each(palettes)('$name canceled status text contrast', ({ tokens: palette }) => {
  // UI status roles deliberately replace terminal literals; D1 stays unchanged.
  it.each(surfaces)('has at least 4.5:1 contrast on %s', (surface) => {
    expect(contrast('--state-canceled', surface, palette)).toBeGreaterThanOrEqual(4.5);
    expect(contrast('--state-queued', surface, palette)).toBeGreaterThanOrEqual(4.5);
  });

  it('remains dimmer than queued status text', () => {
    expect(luminance('--state-canceled', palette)).toBeLessThan(luminance('--state-queued', palette));
  });
});

describe.each(palettes)('$name UI role text contrast', ({ tokens: palette }) => {
  it.each(surfaces)('keeps primary, muted, accent and status text readable on %s', (surface) => {
    for (const foreground of [
      '--text-primary', '--text-muted', '--accent',
      '--status-success', '--status-running', '--status-warning', '--status-error', '--status-blocked',
    ]) expect(contrast(foreground, surface, palette), foreground).toBeGreaterThanOrEqual(4.5);
  });

  it('keeps filled accent labels at 4.5:1', () => {
    expect(contrast('--on-accent', '--accent', palette)).toBeGreaterThanOrEqual(4.5);
    expect(contrast('--accent-ink', '--accent', palette)).toBeGreaterThanOrEqual(4.5);
  });

  it.each(surfaces)('keeps the focus outline at 3:1 on %s', (surface) => {
    expect(contrast('--focus', surface, palette)).toBeGreaterThanOrEqual(3);
  });

  it('reserves the main accent for shapes and checks its white-label regression', () => {
    expect(contrast('--fitted-whitesmoke', '--fitted-stroke', palette)).toBeGreaterThanOrEqual(4.5);
    expect(contrast('--accent-shape', '--canvas', palette)).toBeGreaterThanOrEqual(3);
  });
});

describe.each(palettes)('$name shared control boundary contrast', ({ tokens: palette }) => {
  // Inputs use the canvas inside; outlined buttons/choices can sit on any neutral
  // shell or overlay surface, and card controls use the chrome surface inside.
  it.each(surfaces)
    ('has at least 3:1 contrast against the adjacent %s surface', (surface) => {
      expect(contrast('--border-control', surface, palette)).toBeGreaterThanOrEqual(3);
      expect(contrast('--accent', surface, palette)).toBeGreaterThanOrEqual(3);
    });
});

describe('shared control boundary styles', () => {
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

const glassComposites = [
  ['honey', '#13110f', '#1d1a16'],
  ['sky', '#0e1014', '#171b21'],
] as const;

describe.each(glassComposites)('%s glass contrast over the canvas', (name, glass, strong) => {
  const palette = palettes.find(variant => variant.name === name)!.tokens;

  it.each([
    ['--glass', glass], ['--glass-strong', strong],
  ])('keeps text and controls readable on %s', (token, composite) => {
    const rgba = /^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(palette[token]);
    expect(rgba).not.toBeNull();
    const [, red, green, blue, opacity] = rgba!;
    const canvas = resolveToken('--canvas', palette);
    const alpha = Number(opacity);
    const mixed = [red, green, blue].map((channel, index) => {
      const backdrop = parseInt(canvas.slice(1 + index * 2, 3 + index * 2), 16);
      return Math.round(Number(channel) * alpha + backdrop * (1 - alpha)).toString(16).padStart(2, '0');
    });
    expect(`#${mixed.join('')}`).toBe(composite);

    const resolved = { ...palette, [token]: composite };
    for (const ink of ['--text-primary', '--ink-muted', '--accent']) {
      expect(contrast(ink, token, resolved), ink).toBeGreaterThanOrEqual(4.5);
    }
    for (const boundary of ['--border-control', '--accent', '--focus']) {
      expect(contrast(boundary, token, resolved), boundary).toBeGreaterThanOrEqual(3);
    }
  });

  it('keeps muted ink at 8:1 on the canvas', () => {
    expect(contrast('--ink-muted', '--canvas', palette)).toBeGreaterThanOrEqual(8);
  });
});
