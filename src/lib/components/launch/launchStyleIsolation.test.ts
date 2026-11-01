// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { parse } from 'svelte/compiler';

const { readFileSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
}>('node:fs');

const marker = 'data-launch-owned';
const stylesheet = readFileSync('src/lib/components/launch/launchForm.css', 'utf8');
const wrappedStylesheet = `<style>${stylesheet}</style>`;
const css = parse(wrappedStylesheet, { modern: true }).css!;
type CssChild = typeof css.children[number];
type Rule = Extract<CssChild, { type: 'Rule' }>;
type Compound = Rule['prelude']['children'][number]['children'][number];

function collectRules(nodes: typeof css.children): Rule[] {
  return nodes.flatMap((node) => {
    if (node.type === 'Rule') return [node];
    return node.type === 'Atrule' && node.block
      ? collectRules(node.block.children.filter((child): child is CssChild => child.type === 'Rule' || child.type === 'Atrule'))
      : [];
  });
}

const rules = collectRules(css.children);
const selectors = rules.flatMap((rule) => rule.prelude.children.map((selector) => ({
  selector,
  text: wrappedStylesheet.slice(selector.start, selector.end),
})));

function ownedCompound(compound: Compound): boolean {
  return compound.selectors.some((selector) => {
    if (selector.type === 'AttributeSelector') return selector.name === marker;
    // A positive :where([ownership]) scopes later compounds without adding specificity.
    return selector.type === 'PseudoClassSelector' && selector.name === 'where'
      && selector.args !== null && selector.args.children.length === 1
      && selector.args.children[0].children.length === 1
      && selector.args.children[0].children[0].selectors.length === 1
      && selector.args.children[0].children[0].selectors[0].type === 'AttributeSelector'
      && selector.args.children[0].children[0].selectors[0].name === marker;
  });
}

const views = [
  'LaunchDialog.svelte',
  ...['LaunchAdvanced', 'LaunchHiveSettings', 'LaunchOrchestration', 'LaunchResearchSettings',
    'LaunchFusionSettings', 'LaunchDebateSettings', 'LaunchSoloSettings', 'LaunchPreview']
    .map((name) => `launch/${name}.svelte`),
];

type SourceNode = { type?: string; name?: string; attributes?: Array<{ type: string; name?: string }> };
function walk(value: unknown, visit: (node: SourceNode) => void): void {
  if (!value || typeof value !== 'object') return;
  if (Array.isArray(value)) { value.forEach((child) => walk(child, visit)); return; }
  visit(value as SourceNode);
  Object.values(value).forEach((child) => walk(child, visit));
}

afterEach(() => document.body.replaceChildren());

describe('launch stylesheet ownership', () => {
  it('scopes every selector compound, including descendant targets and media rules', () => {
    expect(rules.length).toBeGreaterThan(0);
    expect(selectors.some(({ text }) => text.startsWith('.field['))).toBe(true);
    expect(stylesheet).toContain('@media');
    for (const { selector, text } of selectors) {
      for (const [index, compound] of selector.children.entries()) {
        expect(ownedCompound(compound), `Unowned selector compound in ${text}`).toBe(true);
        const specificOwners = compound.selectors.filter((part) => part.type === 'AttributeSelector' && part.name === marker);
        expect(specificOwners.length, `Changed scoped specificity in ${text}`).toBe(index === 0 ? 1 : 0);
      }
    }
  });

  it('never selects nested AgentConfigEditor, TemplatePicker or Composer DOM', () => {
    document.body.innerHTML = `
      <section class="launch-content" ${marker}>
        <div id="owned-field" class="field form-section form-group path-picker role-selector checkbox-label" ${marker}>
          <label id="owned-label" ${marker}>Launch field</label>
          <div data-component="AgentConfigEditor" class="field">
            <label>Nested agent field</label><input class="lattice-input">
            <div class="error-message">Agent error</div>
          </div>
          <div data-component="TemplatePicker" class="form-section section-header">
            <h3>Templates</h3><h4>Template details</h4><p>Template description</p>
          </div>
          <div data-component="Composer" class="composer-host field">
            <label>Composer label</label><textarea class="lattice-input"></textarea>
            <div class="error-message">Composer error</div>
          </div>
        </div>
      </section>`;

    const fieldSelector = selectors.find(({ text }) => text === `.field[${marker}]`)!.text;
    const labelSelector = selectors.find(({ text }) => text.startsWith(`.field[${marker}] label`))!.text;
    expect(document.getElementById('owned-field')!.matches(fieldSelector)).toBe(true);
    expect(document.getElementById('owned-label')!.matches(labelSelector)).toBe(true);
    const nested = document.querySelectorAll('[data-component], [data-component] *');
    expect(nested.length).toBeGreaterThan(0);
    for (const node of nested) {
      for (const { text } of selectors) {
        expect(node.matches(text), `Nested ${node.closest('[data-component]')?.getAttribute('data-component')} matched ${text}`).toBe(false);
      }
    }
  });

  it('marks authored launch elements without forwarding ownership into nested components', () => {
    for (const path of views) {
      const ast = parse(readFileSync(`src/lib/components/${path}`, 'utf8'), { modern: true });
      walk(ast.fragment, (node) => {
        const marked = node.attributes?.some((attribute) => attribute.type === 'Attribute' && attribute.name === marker);
        if (node.type === 'RegularElement' || (node.type === 'Component' && node.name === 'Dialog')) {
          expect(marked, `${path}: ${node.name} lost launch ownership`).toBe(true);
        } else if (node.type === 'Component') {
          expect(marked, `${path}: ${node.name} must retain its own stylesheet boundary`).toBe(false);
        }
      });
    }
    for (const path of ['AgentConfigEditor.svelte', 'templates/TemplatePicker.svelte', 'composer/Composer.svelte', 'launch/LaunchComposer.svelte']) {
      const ast = parse(readFileSync(`src/lib/components/${path}`, 'utf8'), { modern: true });
      walk(ast.fragment, (node) => {
        const marked = node.attributes?.some((attribute) => attribute.type === 'Attribute' && attribute.name === marker) ?? false;
        expect(marked, `${path}: nested component DOM must remain outside launch ownership`).toBe(false);
      });
    }
  });
});
