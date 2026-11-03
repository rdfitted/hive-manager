export const keyboardProofNames = ['keyboard-focus-terminal', 'keyboard-target-terminal'];

// Keep the guard in the webview through dispatch: terminal startup can change
// focus after a successful focus() and before CDP delivers the key.
export async function keyboardBoundary(target) {
  return target.evaluateHandle(node => {
    const describe = element => ({ tag: element?.tagName, class: element?.className });
    const outsideTerminal = element => element instanceof Element && !element.closest('.xterm') && !element.matches('textarea.xterm-helper-textarea');
    const state = () => ({ passed: document.activeElement === node && outsideTerminal(document.activeElement), details: { active: describe(document.activeElement), expected: describe(node) } });
    const events = [];
    const listener = event => {
      const safe = event.target === node && state().passed && outsideTerminal(event.target);
      events.push({ key: event.key, safe, target: describe(event.target) });
      if (!safe) { event.preventDefault(); event.stopImmediatePropagation(); }
    };
    window.addEventListener('keydown', listener, true);
    return { state, events, dispose: () => window.removeEventListener('keydown', listener, true) };
  });
}

export async function keyTargetState(boundary, key) {
  return boundary.evaluate((guard, expected) => ({
    passed: guard.events.some(event => event.key === expected) && guard.events.every(event => event.safe),
    details: { key: expected, events: guard.events },
  }), key.split('+').at(-1));
}

export async function disposeKeyboardBoundary(boundary) {
  try { await boundary.evaluate(guard => guard.dispose()); } finally { await boundary.dispose(); }
}

export async function pressOutsideTerminal(page, target, key, assert = () => {}) {
  await target.focus();
  const boundary = await keyboardBoundary(target);
  try {
    const focus = await boundary.evaluate(guard => guard.state());
    assert('keyboard-focus-outside-terminal', focus.passed, focus.details, { key });
    if (!focus.passed) throw new Error('keyboard-focus-outside-terminal');
    await page.keyboard.press(key);
    const dispatched = await keyTargetState(boundary, key);
    assert('keyboard-target-outside-terminal', dispatched.passed, dispatched.details, { key });
    if (!dispatched.passed) throw new Error('keyboard-target-outside-terminal');
  } finally { await disposeKeyboardBoundary(boundary); }
}

export async function proveTerminalFocus({ page, target, terminal, prove }) {
  await target.focus();
  const boundary = await keyboardBoundary(target);
  try {
    const measure = () => boundary.evaluate(guard => guard.state());
    await prove('keyboard-focus-terminal', measure, async () => {
      await terminal.focus();
      return () => target.focus();
    }, { key: 'Enter', boundary: 'real mounted xterm before key dispatch' });
    // Exercise the late-focus guard as well: its capture listener rejects the
    // key before xterm sees it, and the same target predicate fails by name.
    await page.keyboard.press('Enter');
    await prove('keyboard-target-terminal', () => keyTargetState(boundary, 'Enter'), async () => {
      await terminal.focus();
      await page.keyboard.press('Enter');
      return async () => {
        await target.focus();
        await boundary.evaluate(guard => { guard.events.length = 0; });
        await page.keyboard.press('Enter');
      };
    }, { key: 'Enter', boundary: 'capture guard blocks terminal-targeted key' });
  } finally { await disposeKeyboardBoundary(boundary); }
}
