export async function updaterState(page) {
  return page.evaluate(async () => {
    if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function') return { rejected: false, error: 'No real Tauri invoke bridge' };
    try { await window.__TAURI_INTERNALS__.invoke('plugin:updater|check'); return { rejected: false }; }
    catch (error) { return { rejected: true, error: String(error) }; }
  });
}
