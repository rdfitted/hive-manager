// Embedded only by the disposable render build, never by npm run build.
export const htmlProbeName = 'html-ipc-probe.js';
export const htmlProbeMarker = 'html-probe-v1';
export const htmlProbeSource = `(() => {
  const script = document.currentScript;
  script.dataset.htmlProbe = '${htmlProbeMarker}';
  document.body.dataset.htmlAuthor = 'ran';
  const session = script.dataset.ipcSession;
  if (session === undefined) return;
  window.__htmlIpcResult = 'pending';
  try {
    Promise.resolve(window.__TAURI_INTERNALS__.invoke('get_session', { id: decodeURIComponent(session) })).then(reply => {
      document.body.dataset.htmlRust = JSON.stringify({ id: reply.id, createdAt: reply.created_at });
      window.__htmlIpcResult = 'resolved';
    }, error => { window.__htmlIpcResult = 'rejected: ' + String(error).slice(0, 200); });
  } catch (error) { window.__htmlIpcResult = 'threw: ' + String(error).slice(0, 200); }
})();
`;
