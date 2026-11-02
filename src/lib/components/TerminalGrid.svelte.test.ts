import { afterEach, describe, expect, it, vi } from 'vitest';

const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');

afterEach(() => document.querySelectorAll('[data-terminal-frame-test]').forEach((element) => element.remove()));

describe('terminal frame styles', () => {
  it('CR-9: retained non-chat attention has a bounded separate row above the live terminal', () => {
    const source = readFileSync('src/lib/components/TerminalGrid.svelte', 'utf8');
    const styles = source.split('<style>')[1].split('</style>')[0];
    expect(source).toContain('class:non-chat-attention={gridAttention && !chatEligible && hasTerminal}');
    const container = styles.match(/\.terminal-container\.non-chat-attention\s*\{([^{}]*)\}/)?.[1] ?? '';
    expect(container, 'CR-9: reserve a bounded notice row and remaining terminal height')
      .toMatch(/grid-template-rows:\s*minmax\(0,\s*min\(7rem,\s*35%\)\)\s+minmax\(0,\s*1fr\)\s*;/);
    const notice = styles.match(/\.non-chat-attention\s*>\s*\.agent-info\s*\{([^{}]*)\}/)?.[1] ?? '';
    expect(notice).toMatch(/grid-area:\s*1\s*\/\s*1\s*;/);
    expect(notice).toMatch(/max-height:\s*none\s*;/);
    expect(notice).toMatch(/align-self:\s*stretch\s*;/);
    const terminal = styles.match(/\.non-chat-attention\s*>\s*\.terminal-layer\s*\{([^{}]*)\}/)?.[1] ?? '';
    expect(terminal, 'CR-9: the notice must not paint in the live terminal cell').toMatch(/grid-area:\s*2\s*\/\s*1\s*;/);
  });

  it('G31: Grid attention shares the content cell and cannot consume composer height', () => {
    const source = readFileSync('src/lib/components/TerminalGrid.svelte', 'utf8');
    const styles = source.split('<style>')[1].split('</style>')[0];
    const containerStart = source.indexOf('<div class="terminal-container"');
    const beforeTerminal = source.slice(containerStart, source.indexOf('{#if hasTerminal}', containerStart));
    expect(source).toContain('const gridAttention = !focusMode && visible && showAttention');
    expect(beforeTerminal).toContain('{#if gridAttention');
    expect(beforeTerminal).toContain('<div class="agent-info">');
    expect(beforeTerminal).toContain('<AttentionCard');
    const card = styles.match(/\.agent-info\s*\{([^{}]*)\}/)?.[1] ?? '';
    expect(card).toMatch(/grid-area:\s*1\s*\/\s*1\s*;/);
    expect(card).toMatch(/align-self:\s*start\s*;/);
    expect(card).toMatch(/min-height:\s*0\s*;/);
    expect(card).toMatch(/max-height:\s*min\(7rem,\s*35%\)\s*;/);
    expect(card).toMatch(/overflow:\s*auto\s*;/);
    expect(styles).toMatch(/\.terminal-layer,\s*\.chat-layer\s*\{\s*grid-area:\s*1\s*\/\s*1/);
    expect(styles).toMatch(/\.terminal-container\s*\{[^{}]*grid-template:\s*minmax\(0,\s*1fr\)\s*\/\s*minmax\(0,\s*1fr\)/);
  });
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
