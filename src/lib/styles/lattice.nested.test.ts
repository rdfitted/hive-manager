// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';

// Read real sources; Vitest's disabled CSS pipeline stubs CSS ?raw imports.
// A narrow type avoids adding Node ambient types to this browser project.
const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');

const surfaces = readFileSync('src/lib/styles/globals/surfaces.css', 'utf8');
const tokens = Object.fromEntries(
  Array.from(readFileSync('src/lib/styles/lattice-tokens.css', 'utf8').matchAll(/(--[\w-]+):\s*([^;]+);/g),
    ([, name, value]) => [name, value]),
);

// jsdom does not resolve custom properties. Resolve the actual production
// tokens in this fixture so an accidentally transparent overlay is observable.
function resolveTokens(css: string): string {
  return css.replace(/var\((--[\w-]+)\)/g, (reference, name: string) =>
    tokens[name] ? resolveTokens(tokens[name]) : reference);
}

/** Apply the production rules for one media condition. jsdom does not emulate
 * forced colours, so select that media block explicitly; this is a CSS cascade
 * fixture, not a claim about the desktop's high-contrast rendering. */
function installStyles(forcedColors = false) {
  const source = document.createElement('style');
  source.textContent = resolveTokens(surfaces);
  document.head.append(source);
  const media: string[] = [];
  const rules = Array.from(source.sheet!.cssRules).flatMap((rule) => {
    if (rule instanceof CSSMediaRule) {
      if (forcedColors) media.push(...Array.from(rule.cssRules).map((child) => child.cssText));
      return [];
    }
    return [rule.cssText];
  });
  // Put the important media declarations last: jsdom does not implement
  // !important precedence between stylesheets like the browser does.
  source.textContent = [...rules,
    resolveTokens(readFileSync('src/lib/styles/globals/controls.css', 'utf8')),
    ...media].join('\n');
  return source;
}

afterEach(() => {
  document.head.querySelectorAll('style').forEach((style) => style.remove());
  document.body.replaceChildren();
});

describe('nested Lattice surfaces', () => {
  it('removes the second surface and boundary through wrappers and deeper panels', () => {
    installStyles();
    document.body.innerHTML = `
      <section class="lattice-panel" id="outer">
        <div><section class="lattice-panel lattice-panel--active" id="nested">
          <div class="lattice-panel" id="deep"></div>
        </section></div>
      </section>`;

    for (const id of ['nested', 'deep']) {
      const style = getComputedStyle(document.getElementById(id)!);
      expect(style.borderTopWidth).toBe('0px');
      expect(style.boxShadow).toBe('none');
      expect(style.backgroundColor).toBe('rgba(0, 0, 0, 0)');
    }
    expect(getComputedStyle(document.getElementById('outer')!).boxShadow).toContain('0 1px 2px');
  });

  it('keeps the reset authoritative over component-local elevation recipes', () => {
    const source = installStyles();
    const nested = Array.from(source.sheet!.cssRules).find(
      (rule) => rule instanceof CSSStyleRule && rule.selectorText.startsWith('.lattice-panel .lattice-panel'),
    ) as CSSStyleRule;
    // Real components add scoped shadow recipes after the global stylesheet.
    // The priorities are necessary even when a component selector is more specific.
    expect(nested.style.getPropertyPriority('box-shadow')).toBe('important');
    expect(nested.style.getPropertyPriority('background-color')).toBe('important');
    expect(nested.style.getPropertyPriority('border')).toBe('');
  });

  it.each(['context-menu', 'search-bar', 'drop-card', 'mention-menu', 'slash-menu', 'color-picker', 'update-banner'])(
    'keeps the floating %s opaque and elevated inside a terminal panel', (overlay) => {
      installStyles();
      document.body.innerHTML = `<section class="lattice-panel">
        <div class="terminal-item lattice-panel"><div id="overlay" class="${overlay} lattice-panel"></div></div>
      </section>`;
      const style = getComputedStyle(document.getElementById('overlay')!);
      expect(style.backgroundColor).toBe('rgb(22, 22, 22)');
      expect(style.boxShadow).toContain('0 1px 2px');
    },
  );

  it('preserves owner boundaries in forced colours, including nested controls', () => {
    installStyles(true);
    document.body.innerHTML = `
      <section class="lattice-panel" id="outer">
        <section class="lattice-panel" id="nested">
          <button class="lattice-btn" id="action">Focus terminal</button>
          <button class="lattice-forced-colors-boundary" id="command">Open Activity</button>
          <input class="lattice-input" id="input" aria-label="Find in terminal" />
          <label class="lattice-selectable" id="choice"><input type="radio" />Shared workspace</label>
          <button class="lattice-tab" id="tab">Overview</button>
        </section>
      </section>`;

    for (const id of ['outer', 'nested', 'action', 'command', 'input', 'choice']) {
      const style = getComputedStyle(document.getElementById(id)!);
      expect(style.borderTopWidth).toBe('1px');
      expect(style.borderTopStyle).toBe('solid');
    }
    expect(getComputedStyle(document.getElementById('tab')!).borderBottomWidth).toBe('2px');
    expect(getComputedStyle(document.getElementById('nested')!).boxShadow).toBe('none');
  });
});
