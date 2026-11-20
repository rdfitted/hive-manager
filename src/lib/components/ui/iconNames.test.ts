import { parse, type AST } from 'svelte/compiler';
import { describe, expect, it } from 'vitest';

const sources = import.meta.glob('/src/**/*.svelte', { eager: true, query: '?raw', import: 'default' });
type Expression = AST.ExpressionTag['expression'];
type Element = Pick<AST.RegularElement, 'attributes'>;
type TextMode = 'visible' | 'accessible';

function attribute(node: Element, name: string) {
  return node.attributes.find((attr): attr is AST.Attribute => attr.type === 'Attribute' && attr.name === name);
}

function textValue(attr: AST.Attribute | undefined) {
  const value = attr?.value;
  if (Array.isArray(value) && value.every(piece => piece.type === 'Text')) {
    return value.map(piece => piece.data).join('');
  }
}

function hasWords(text: string) {
  return /[\p{L}\p{N}]/u.test(text.replace(/\\u[\da-f]{4}/gi, ''));
}

function expressionHasText(expression: Expression): boolean {
  switch (expression.type) {
    case 'Literal':
      return (typeof expression.value === 'string' || typeof expression.value === 'number')
        && hasWords(String(expression.value));
    case 'Identifier':
      return expression.name !== 'undefined';
    case 'ConditionalExpression':
      return expressionHasText(expression.consequent) && expressionHasText(expression.alternate);
    case 'LogicalExpression':
      return expression.operator !== '&&'
        && (expressionHasText(expression.left) || expressionHasText(expression.right));
    case 'TemplateLiteral':
      return expression.quasis.some(part => hasWords(part.value.raw)) || expression.expressions.some(expressionHasText);
    default:
      return true;
  }
}

function populated(node: Element, name: string) {
  const value = attribute(node, name)?.value;
  if (!value || value === true) return false;
  const pieces = Array.isArray(value) ? value : [value];
  return pieces.some(piece => piece.type === 'Text' ? hasWords(piece.data) : expressionHasText(piece.expression));
}

function fragments(node: AST.TemplateNode): Array<AST.Fragment | null | undefined> {
  if ('fragment' in node) return [node.fragment];
  switch (node.type) {
    case 'IfBlock': return [node.consequent, node.alternate];
    case 'EachBlock': return [node.body, node.fallback];
    case 'AwaitBlock': return [node.pending, node.then, node.catch];
    case 'SnippetBlock': return [node.body];
    default: return [];
  }
}

function hasText(fragment: AST.Fragment | null | undefined, mode: TextMode): boolean {
  return fragment?.nodes.some(node => nodeHasText(node, mode)) ?? false;
}

function nodeHasText(node: AST.TemplateNode, mode: TextMode): boolean {
  if (node.type === 'Text') return hasWords(node.data);
  if (node.type === 'ExpressionTag') return expressionHasText(node.expression);
  if (node.type === 'RegularElement' || node.type === 'SvelteElement') {
    if (['svg', 'img', 'canvas'].includes(node.name) || attribute(node, 'hidden')) return false;
    if (mode === 'accessible' && textValue(attribute(node, 'aria-hidden')) === 'true') return false;
    const classes = textValue(attribute(node, 'class'))?.split(/\s+/) ?? [];
    if (mode === 'visible' && classes.some(name => name === 'sr-only' || name === 'visually-hidden')) return false;
    return hasText(node.fragment, mode);
  }
  if (node.type === 'IfBlock' || node.type === 'EachBlock' || node.type === 'AwaitBlock' || node.type === 'KeyBlock') {
    return fragments(node).every(fragment => hasText(fragment, mode));
  }
  return false;
}

function componentNames(tree: AST.Root, component: 'IconButton' | 'Tooltip') {
  const names = new Set<string>([component]);
  for (const node of tree.instance?.content.body ?? []) {
    if (node.type !== 'ImportDeclaration') continue;
    const source = node.source.value;
    for (const specifier of node.specifiers) {
      if (specifier.type === 'ImportSpecifier' && specifier.imported.type === 'Identifier' && specifier.imported.name === component) {
        names.add(specifier.local.name);
      }
      if (specifier.type === 'ImportDefaultSpecifier' && typeof source === 'string' && source.endsWith(`/${component}.svelte`)) {
        names.add(specifier.local.name);
      }
    }
  }
  return names;
}

function triggerProps(snippet: AST.TemplateNode) {
  if (snippet.type !== 'SnippetBlock' || snippet.expression.name !== 'children') return undefined;
  const [parameter] = snippet.parameters;
  if (parameter?.type !== 'ObjectPattern') return undefined;
  for (const property of parameter.properties) {
    if (property.type === 'Property' && property.key.type === 'Identifier' && property.key.name === 'props'
      && property.value.type === 'Identifier') return property.value.name;
  }
}

function rawButtonIssues(node: AST.RegularElement | AST.SvelteElement, propsName?: string) {
  const visibleText = hasText(node.fragment, 'visible');
  const accessibleName = populated(node, 'aria-label') || populated(node, 'aria-labelledby') || hasText(node.fragment, 'accessible');
  // The native title supplies a hover hint. Keep the control name explicit.
  const tooltip = populated(node, 'title') || node.attributes.some(attr =>
    attr.type === 'SpreadAttribute' && attr.expression.type === 'Identifier' && attr.expression.name === propsName);
  return [accessibleName ? '' : 'accessible name', visibleText || tooltip ? '' : 'tooltip'].filter(Boolean);
}

function scan(source: string) {
  const tree = parse(source, { modern: true });
  const iconButtons = componentNames(tree, 'IconButton');
  const tooltips = componentNames(tree, 'Tooltip');
  const violations: string[] = [];

  function report(node: AST.TemplateNode, issues: string[]) {
    if (issues.length) violations.push(`${source.slice(0, node.start).split('\n').length} missing ${issues.join(' and ')}`);
  }

  function visit(node: AST.TemplateNode, propsName?: string) {
    if (node.type === 'Component' && iconButtons.has(node.name)) {
      report(node, [populated(node, 'label') ? '' : 'label', populated(node, 'tooltip') ? '' : 'tooltip'].filter(Boolean));
    }

    if ((node.type === 'RegularElement' && node.name === 'button')
      || (node.type === 'SvelteElement' && node.tag.type === 'Literal' && node.tag.value === 'button')) {
      report(node, rawButtonIssues(node, propsName));
    }

    const wrapsTrigger = node.type === 'Component' && tooltips.has(node.name) && populated(node, 'text');
    for (const fragment of fragments(node)) {
      for (const child of fragment?.nodes ?? []) {
        visit(child, wrapsTrigger ? triggerProps(child) : propsName);
      }
    }
  }

  for (const node of tree.fragment.nodes) visit(node);
  return violations;
}

describe('G-icon-names', () => {
  it('accepts text buttons and explicitly named, described icon controls', () => {
    expect(scan([
      '<button><svg aria-hidden="true"></svg><span>Open</span></button>',
      '<button aria-label="Close" title="Close"><svg><title>Close</title></svg></button>',
      '<button title="Open"><span class="sr-only">Open</span><svg></svg></button>',
      '<Tooltip text="Open">{#snippet children({ props: trigger })}<button {...trigger} aria-label="Open"><svg></svg></button>{/snippet}</Tooltip>',
      '<IconButton label="Open" tooltip="Open" icon={Plus} />',
    ].join('\n'))).toEqual([]);
  });

  it('finds missing names and hints across glyphs, branches and aliased primitives', () => {
    expect(scan([
      '<script>import { IconButton as Action } from "../ui";</script>',
      '<button title="Close">&times;</button>',
      '<button aria-label="Dismiss"><svg aria-hidden="true"><title>Dismiss</title></svg></button>',
      '<button>{#if busy}<svg></svg>{:else}Save{/if}</button>',
      '<Tooltip text="Open">{#snippet children({ props })}<button aria-label="Open"><svg></svg></button>{/snippet}</Tooltip>',
      '<Action label={undefined} tooltip=" " icon={Plus} />',
    ].join('\n'))).toEqual([
      '2 missing accessible name',
      '3 missing tooltip',
      '4 missing accessible name and tooltip',
      '5 missing tooltip',
      '6 missing label and tooltip',
    ]);
  });

  it('connects the shared IconButton name and tooltip to its native button', () => {
    const source = sources['/src/lib/components/ui/IconButton.svelte'] as string;
    expect(scan(source)).toEqual([]);
  });

  it('requires accessible names and tooltips across the Svelte component tree', () => {
    const violations = Object.entries(sources).sort(([a], [b]) => a.localeCompare(b))
      .flatMap(([path, source]) => scan(source as string).map(issue => `${path.slice(1)}:${issue}`));
    expect(violations).toEqual([]);
  });
});
