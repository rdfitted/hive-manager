import { afterEach, describe, expect, it, vi } from 'vitest';

const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');

afterEach(() => document.querySelectorAll('[data-terminal-frame-test]').forEach((element) => element.remove()));

describe('terminal frame styles', () => {
  it('terminal_panel_does_not_inherit_shared_panel_padding', () => {
    const shared = readFileSync('src/lib/styles/globals/surfaces.css', 'utf8');
    const grid = readFileSync('src/lib/components/TerminalGrid.svelte', 'utf8').split('<style>')[1].split('</style>')[0];
    const panelRule = shared.match(/\.lattice-panel\s*\{([^{}]*)\}/)?.[1] ?? '';
    const frameRule = grid.match(/\.terminal-item\s*\{([^{}]*)\}/)?.[1] ?? '';
    const tokenSource = readFileSync('src/lib/styles/lattice-tokens.css', 'utf8');
    const space = tokenSource.match(/--space-4:\s*([^;]+);/)?.[1].trim();
    expect(space).toBe('16px');
    expect(panelRule).toMatch(/padding:\s*var\(--space-4\)\s*;/);

    const style = document.createElement('style');
    style.dataset.terminalFrameTest = '';
    // jsdom does not resolve custom properties. Resolve only this real token in
    // the extracted recipes and separately pin their actual padding declarations.
    style.textContent = `.lattice-panel { ${panelRule.replaceAll('var(--space-4)', space!)} } .terminal-item { ${frameRule} }`;
    document.head.append(style);
    const host = document.createElement('div');
    host.dataset.terminalFrameTest = '';
    host.innerHTML = '<div class="terminal-item lattice-panel"></div><div class="ordinary lattice-panel"></div>';
    document.body.append(host);
    const frame = getComputedStyle(host.querySelector('.terminal-item')!);
    const ordinary = getComputedStyle(host.querySelector('.ordinary')!);
    for (const side of ['paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft'] as const) {
      expect(frame[side], side).toBe('0px');
      expect(ordinary[side], side).toBe(space);
    }
    expect(frameRule).toMatch(/padding:\s*0\s*;/);
  });
});
