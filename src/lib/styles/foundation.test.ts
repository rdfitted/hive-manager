import { describe, expect, it, vi } from 'vitest';

const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');
const source = readFileSync('src/lib/styles/lattice-tokens.css', 'utf8');
const tokens = Object.fromEntries(Array.from(source.matchAll(/(--[\w-]+):\s*([^;]+);/g),
  ([, name, value]) => [name, value.trim()]));

describe('visual foundation contract', () => {
  it('pins every terminal palette input to its unchanged literal hex (D1)', () => {
    const palette = {
      '--bg-void': '#060606', '--text-primary': '#E8E8E8', '--text-secondary': '#969696',
      '--ansi-blue': '#658FEC', '--ansi-blue-bright': '#B3CCFF',
      '--ansi-magenta': '#CA67C7', '--ansi-magenta-bright': '#F18BEE',
      '--status-success': '#A0F3BC', '--status-running': '#B5DAEC',
      '--status-warning': '#ECDDA5', '--status-error': '#F39CA9',
      '--status-blocked': '#EDA68C', '--status-queued': '#969696', '--status-canceled': '#888888',
    };
    for (const [name, value] of Object.entries(palette)) expect(tokens[name], name).toBe(value);
    expect(Object.keys(tokens).filter((name) => /^--(?:status|ansi)-/.test(name)).sort())
      .toEqual(Object.keys(palette).filter((name) => /^--(?:status|ansi)-/.test(name)).sort());
  });

  it('exposes additive UI roles and compact geometry without reversing palette aliases', () => {
    for (const [role, target] of Object.entries({
      canvas: 'bg-void', chrome: 'bg-chrome', toolbar: 'bg-chrome', 'toolbar-control': 'bg-raised',
      surface: 'bg-panel', 'surface-raised': 'bg-raised', 'surface-overlay': 'bg-elevated',
      'text-muted': 'text-secondary', border: 'border-structural', input: 'bg-void',
      focus: 'accent-cyan', accent: 'accent-cyan',
    })) expect(tokens[`--${role}`]).toBe(`var(--${target})`);
    for (const [name, value] of Object.entries({
      'control-radius': '8px', 'control-height': '32px', 'control-height-sm': '28px',
      'toolbar-height': '44px', 'sidebar-inset': '8px',
    })) expect(tokens[`--${name}`]).toBe(value);
    expect(tokens['--accent-cyan']).toBe('#B5DAEC');
  });

  it('uses sans-serif sentence case controls without tracking or hover glow', () => {
    const controls = readFileSync('src/lib/styles/globals/controls.css', 'utf8');
    expect(controls).not.toMatch(/font-mono|text-transform:\s*uppercase|letter-spacing:\s*[\d.]|text-shadow:\s*(?!none)[\d]/);
    const hovers = Array.from(controls.matchAll(/[^{}]*:hover[^{}]*\{([^{}]*)\}/g), ([, body]) => body);
    expect(hovers.length).toBeGreaterThan(10);
    for (const body of hovers) {
      const shadow = /box-shadow:\s*([^;]+);/.exec(body)?.[1];
      if (shadow && shadow !== 'none') expect(shadow).toMatch(/^inset 0 0 0 1px (?:var\([^)]*\)|rgba\(var\([^)]*\), [\d.]+\))$/);
    }
    expect(controls).toContain('var(--motion-duration-instant)');
  });

  it.each(['status', 'table', 'tooltip'])('uses sans-serif sentence case chrome in %s.css without tracking', (name) => {
    const chrome = readFileSync(`src/lib/styles/globals/${name}.css`, 'utf8');
    expect(chrome).toContain('font-family: var(--font-body);');
    expect(chrome).not.toMatch(/font-mono|text-transform:\s*uppercase|letter-spacing:\s*[\d.]/);
  });
});
