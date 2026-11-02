// Mutations affect only the disposable WebView DOM and are restored in finally.
// The same measurement functions used by the gate are supplied by the caller.
export function mutationProver(report, assert) {
  const seen = new Set();
  return async (name, measure, change, context = {}) => {
    if (seen.has(name)) return;
    seen.add(name);
    let undo, before, mutant, restored, error;
    try {
      before = await measure();
      undo = await change();
      mutant = await measure();
    } catch (failure) { error = failure.message; }
    finally {
      try { if (undo) await undo(); restored = await measure(); }
      catch (failure) { error = failure.message; }
    }
    const passed = before?.passed === true && mutant?.passed === false && restored?.passed === true && !error;
    report.mutations.push({ name, context, before, mutant, restored, error, passed, failedByName: mutant?.passed === false ? name : null });
    assert('mutation-proof', passed, { name, before, mutant, restored, error }, context);
  };
}

export async function styleChange(locator, properties) {
  const original = await locator.getAttribute('style');
  await locator.evaluate((node, values) => { for (const [key, value] of Object.entries(values)) node.style.setProperty(key, value, 'important'); }, properties);
  return async () => locator.evaluate((node, value) => value === null ? node.removeAttribute('style') : node.setAttribute('style', value), original);
}

export async function removeNode(locator) {
  const handle = await locator.evaluateHandle(node => {
    const marker = document.createComment('render-mutation-restore');
    node.replaceWith(marker);
    return { node, marker };
  });
  return async () => { try { await handle.evaluate(({ node, marker }) => marker.replaceWith(node)); } finally { await handle.dispose(); } };
}

export async function textChange(locator, value) {
  const handle = await locator.evaluateHandle((node, text) => {
    const children = [...node.childNodes];
    node.textContent = text;
    return { node, children };
  }, value);
  return async () => { try { await handle.evaluate(({ node, children }) => node.replaceChildren(...children)); } finally { await handle.dispose(); } };
}

export async function appendNode(locator, { tag = 'span', attributes = {}, text = '' } = {}) {
  const handle = await locator.evaluateHandle((parent, spec) => {
    const node = document.createElement(spec.tag);
    for (const [key, value] of Object.entries(spec.attributes)) node.setAttribute(key, value);
    node.textContent = spec.text;
    parent.append(node);
    return node;
  }, { tag, attributes, text });
  return async () => { try { await handle.evaluate(node => node.remove()); } finally { await handle.dispose(); } };
}
