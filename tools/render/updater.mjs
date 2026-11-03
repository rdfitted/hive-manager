// Shared by the real IPC observation and in-memory mutation proofs.
export function classifyUpdater(outcome) {
  return { ...outcome, rejected: outcome.error === 'plugin updater not found' };
}

export async function updaterState(page) {
  const outcome = await page.evaluate(async () => {
    if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function') return { error: 'No real Tauri invoke bridge' };
    try { await window.__TAURI_INTERNALS__.invoke('plugin:updater|check'); return {}; }
    catch (error) { return { error: String(error) }; }
  });
  return classifyUpdater(outcome);
}
