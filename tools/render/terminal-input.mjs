import { selectSession, viewMode } from './assertions.mjs';
import { pressOutsideTerminal } from './keyboard.mjs';

// Replay the focus schedule that made the old row.press('Enter') write to PTY.
// The fixed driver must use its real activation and shortcut paths, error-free.
export async function verifyTerminalInputFix({ page, manifest, report, consoleErrors, assert }) {
  const session = manifest.sessions.find(item => item.id === manifest.screens.grid.sessionId);
  let release, trace;
  const ready = new Promise(resolve => { release = resolve; });
  const hold = async route => { await ready; await route.continue().catch(() => {}); };
  await page.route('**/agent-feeds', hold);
  try {
    await selectSession(page, session);
    await viewMode(page, 'Grid');
    const terminal = page.locator('.terminal-panel:not(.hidden) .xterm-helper-textarea').first();
    await terminal.focus();
    trace = await page.evaluateHandle(() => {
      const events = [];
      const listener = event => events.push({ type: event.type, key: event.key, target: event.target.className });
      for (const type of ['focusin', 'keydown', 'keyup']) document.addEventListener(type, listener, true);
      return { events, dispose: () => { for (const type of ['focusin', 'keydown', 'keyup']) document.removeEventListener(type, listener, true); } };
    });
    await page.waitForTimeout(500);
    const details = report.terminalInputDiagnostic = { beforeClick: [...consoleErrors] };
    const row = page.locator('.session-button').filter({ hasText: session.name }).first();
    await row.evaluate(node => node.addEventListener('focusin', () => {
      setTimeout(() => document.querySelector('.terminal-panel:not(.hidden) .xterm-helper-textarea')?.focus(), 0);
    }, { once: true }));
    await selectSession(page, session);
    await page.waitForTimeout(500);
    details.afterClick = [...consoleErrors];
    details.focusRaceInjected = await terminal.evaluate(node => document.activeElement === node);
    assert('terminal-focus-race-injected', details.focusRaceInjected, details);
    await pressOutsideTerminal(page, page.getByRole('link', { name: 'Sessions', exact: true }), 'Control+/', assert);
    const dialog = page.getByRole('dialog', { name: 'Keyboard shortcuts', exact: true });
    await dialog.waitFor();
    details.dialogOpened = await dialog.isVisible();
    details.afterShortcut = [...consoleErrors];
    details.events = await trace.evaluate(value => value.events);
    assert('terminal-input-fix-no-errors', consoleErrors.length === 0 && details.dialogOpened, details);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  } finally {
    if (trace) { try { await trace.evaluate(value => value.dispose()); } finally { await trace.dispose(); } }
    release();
    await page.unroute('**/agent-feeds', hold);
  }
}
