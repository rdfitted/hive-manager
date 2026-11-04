import { mutationProver, appendNode, removeNode } from './mutations.mjs';

// R-C3 retains html-ipc's live assertion and positive control, while the
// independent html-script mutation remains gating. The stopped two-token IPC
// mutation is explicitly excluded; its recorded evidence is printed below.
export const securityProofNames = ['csp-inline', 'csp-console', 'html-closed', 'html-open', 'html-sandbox', 'html-style', 'html-attr', 'html-inert', 'html-parent', 'html-script', 'html-api'];

// These probes run only in the disposable WebView. DevTools evaluation can run
// even when page scripts cannot, which lets us measure the actual opaque frame.
export async function runSecurityAssertions({ page, pane, report, assert, consoleErrors, capture, sessionId }) {
  if (report.security) return;
  report.security = { violations: [] };
  const prove = mutationProver(report, assert);
  const check = async (name, measure) => { const result = await measure(); assert(name, result.passed, result.details); };
  const expectedViolations = async (operation, kind, endpoints = []) => {
    const start = consoleErrors.length;
    const attributed = [];
    const observe = message => {
      const text = message.text();
      const source = message.location().url ?? '';
      const sandbox = /Blocked script execution in 'about:srcdoc'.*sandboxed.*allow-scripts/i.test(text)
        || kind === 'blank' && /Blocked script execution in 'about:blank'.*sandboxed.*allow-scripts/i.test(text);
      // WebView2 can attribute the inherited-policy handler diagnostic to the
      // app URL. This scope contains the one explicit frame onerror dispatch.
      const frameInline = (source === 'about:srcdoc' || kind === 'handler')
        && /Executing inline event handler.*Content Security Policy/i.test(text);
      // Chromium omits source URLs on fetch rejection diagnostics; the unique
      // probe endpoint plus CSP wording still identifies these exact attempts.
      const frameNetwork = endpoints.some(endpoint => text.includes(endpoint)) && /Content Security Policy/i.test(text);
      if (sandbox || frameInline || frameNetwork) attributed.push({ text, source });
    };
    page.on('console', observe);
    try { return await operation(); }
    finally {
      await page.waitForTimeout(100);
      page.off('console', observe);
      // Remove exactly one occurrence per attributed frame message. Application
      // CSP errors and unmatched frame errors remain in the all-screen gate.
      for (const item of attributed) {
        const index = consoleErrors.indexOf(item.text, start);
        if (index >= 0) report.security.violations.push({ kind, source: item.source, message: consoleErrors.splice(index, 1)[0] });
      }
    }
  };

  const cleanConsole = async () => ({ passed: consoleErrors.length === 0, details: [...consoleErrors] });
  await check('csp-console', cleanConsole);
  await prove('csp-console', cleanConsole, async () => {
    const start = consoleErrors.length;
    await page.evaluate(() => {
      const script = document.createElement('script'); script.id = 'csp-probe';
      script.textContent = 'window.__cspProbe = true'; document.head.append(script);
    });
    await page.waitForFunction(() => !document.getElementById('csp-probe') || window.__cspProbe !== true).catch(() => {});
    await page.waitForTimeout(100);
    report.security.inlineViolations = consoleErrors.slice(start);
    report.security.inlineExecuted = await page.evaluate(() => window.__cspProbe === true);
    return async () => {
      await page.evaluate(() => { document.getElementById('csp-probe')?.remove(); delete window.__cspProbe; });
      for (let i = start; i < consoleErrors.length; i++) {
        if (/(?:Executing inline script|Refused to execute inline script).*Content Security Policy/i.test(consoleErrors[i])) {
          report.security.violations.push({ kind: 'inline', message: consoleErrors.splice(i, 1)[0] });
          break;
        }
      }
    };
  });
  const inline = async () => {
    const executed = await page.evaluate(() => window.__cspProbe === true);
    return { passed: !executed && !report.security.inlineExecuted && report.security.inlineViolations?.some(message => /inline script.*Content Security Policy/i.test(message)), details: { executed, violations: report.security.inlineViolations } };
  };
  await check('csp-inline', inline);
  await prove('csp-inline', inline, async () => {
    await page.evaluate(() => { window.__cspProbe = true; });
    return () => page.evaluate(() => { delete window.__cspProbe; });
  });

  const preview = pane.locator('iframe[title="HTML preview"]');
  const literal = pane.locator('pre[aria-label="Code block: html"] code');
  const closed = async () => ({ passed: await preview.count() === 0 && (await literal.textContent())?.includes('<h1>Orbital HTML report</h1>'), details: { frames: await preview.count(), source: await literal.textContent() } });
  await literal.scrollIntoViewIfNeeded();
  await check('html-closed', closed);
  await prove('html-closed', closed, () => expectedViolations(() => appendNode(pane.locator('.html-preview').first(), { tag: 'iframe', attributes: { title: 'HTML preview', sandbox: '' } }), 'blank'));
  await pane.getByRole('button', { name: 'Preview HTML', exact: true }).click();
  await preview.scrollIntoViewIfNeeded();
  const opened = async () => ({ passed: await preview.count() === 1 && await preview.isVisible() && (await preview.getAttribute('srcdoc'))?.includes('Orbital HTML report'), details: { frames: await preview.count() } });
  await check('html-open', opened);
  if (capture) {
    await pane.getByRole('button', { name: 'Close HTML preview', exact: true }).scrollIntoViewIfNeeded();
    await capture();
  }
  await prove('html-open', opened, () => removeNode(preview));
  const sandbox = async () => {
    const details = await preview.evaluate(node => ({ sandbox: node.getAttribute('sandbox'), src: node.getAttribute('src'), source: node.srcdoc, referrer: node.referrerPolicy }));
    return { passed: details.sandbox === '' && details.src === null && details.referrer === 'no-referrer' && details.source.includes('<head><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'none\';') && details.source.includes("connect-src 'none'") && !/<script|onerror=|href=/i.test(details.source), details };
  };
  await check('html-sandbox', sandbox);
  await prove('html-sandbox', sandbox, async () => {
    await preview.evaluate(node => node.removeAttribute('sandbox'));
    return () => preview.evaluate(node => node.setAttribute('sandbox', ''));
  });
  const getFrame = async () => {
    const frame = await (await preview.elementHandle()).contentFrame();
    await frame.waitForSelector('h1');
    return frame;
  };
  let frame = await getFrame();
  const reloadFrame = async (change, value) => {
    const navigation = frame.waitForNavigation({ waitUntil: 'domcontentloaded' });
    await preview.evaluate(change, value);
    await navigation;
    frame = await getFrame();
  };
  const styled = async () => {
    const details = await frame.evaluate(() => ({ heading: getComputedStyle(document.querySelector('h1')).fontSize, padding: getComputedStyle(document.body).paddingTop }));
    return { passed: details.heading === '20px' && details.padding === '16px', details };
  };
  await check('html-style', styled);
  await prove('html-style', styled, async () => {
    const original = await frame.locator('h1').getAttribute('style');
    await frame.locator('h1').evaluate(node => { node.style.fontSize = '42px'; });
    return () => frame.locator('h1').evaluate((node, value) => value === null ? node.removeAttribute('style') : node.setAttribute('style', value), original);
  });
  const styledAttribute = async () => {
    const spacing = await frame.locator('p').first().evaluate(node => getComputedStyle(node).letterSpacing);
    return { passed: spacing === '2px', details: { spacing } };
  };
  await check('html-attr', styledAttribute);
  await prove('html-attr', styledAttribute, async () => {
    const paragraph = frame.locator('p').first();
    const original = await paragraph.getAttribute('style');
    await paragraph.evaluate(node => node.removeAttribute('style'));
    return () => paragraph.evaluate((node, value) => node.setAttribute('style', value), original);
  });
  await expectedViolations(() => frame.evaluate(() => {
    window.__htmlScript = false; window.__htmlError = false;
    const script = document.createElement('script'); script.textContent = 'window.__htmlScript = true'; document.body.append(script);
    const image = document.createElement('img'); image.setAttribute('onerror', 'window.__htmlError = true');
    document.body.append(image);
    image.dispatchEvent(new Event('error'));
  }), 'handler');
  report.security.handlerDispatched = true;
  const inert = async () => {
    const details = await frame.evaluate(() => ({ script: window.__htmlScript, handler: window.__htmlError, scripts: document.scripts.length, handlers: document.querySelectorAll('[onerror]').length }));
    return { passed: details.script === false && details.handler === false && details.scripts === 1 && details.handlers === 1, details };
  };
  await check('html-inert', inert);
  await prove('html-inert', inert, async () => {
    await frame.evaluate(() => { window.__htmlScript = true; window.__htmlError = true; });
    return () => frame.evaluate(() => { window.__htmlScript = false; window.__htmlError = false; });
  });
  const parent = async () => {
    const details = await frame.evaluate(() => {
      try { return { blocked: false, title: parent.document.title }; }
      catch (error) { return { blocked: error.name === 'SecurityError', name: error.name }; }
    });
    return { passed: details.blocked, details };
  };
  await check('html-parent', parent);
  await prove('html-parent', parent, async () => {
    await reloadFrame(node => { node.setAttribute('sandbox', 'allow-same-origin'); node.srcdoc = node.srcdoc; });
    return async () => {
      await reloadFrame(node => { node.setAttribute('sandbox', ''); node.srcdoc = node.srcdoc; });
    };
  });
  const ipcUrl = 'http://ipc.localhost/html-probe-ipc';
  const ipcRequests = [];
  const ipcRequest = request => { if (request.url() === ipcUrl) ipcRequests.push(request.url()); };
  const ipcRoute = route => route.fulfill({ status: 200, headers: { 'Access-Control-Allow-Origin': '*' }, body: 'synthetic' });
  page.on('request', ipcRequest);
  await page.route(ipcUrl, ipcRoute);
  try {
    await expectedViolations(() => frame.evaluate(async endpoint => {
      window.__htmlIpc = false;
      try { await fetch(endpoint); window.__htmlIpc = true; } catch {}
    }, ipcUrl), 'frame', [ipcUrl]);
  } finally { page.off('request', ipcRequest); await page.unroute(ipcUrl, ipcRoute); }
  const bridge = await frame.evaluate(() => ({ globals: ['__TAURI_INTERNALS__', '__TAURI__', '__TAURI_IPC__', 'ipc'].filter(name => typeof window[name] !== 'undefined'), native: { webview: typeof window.chrome?.webview?.postMessage, webkit: typeof window.webkit?.messageHandlers }, protocolSucceeded: window.__htmlIpc }));
  report.security.nativeBridge = bridge;
  // Host-injected globals exist in WebView2 even with sandbox="". An allowed
  // read reply supplies a fresh Rust value without enabling an isolated write.
  const args = { id: sessionId };
  const productionSource = await preview.getAttribute('srcdoc');
  const origin = new URL(page.url()).origin;
  const probeUrl = `${origin}/html-ipc-probe.js`;
  const authorScript = "document.body.dataset.htmlAuthor = 'ran';";
  const ipcScript = `${authorScript} window.__htmlIpcResult = 'pending'; try { Promise.resolve(window.__TAURI_INTERNALS__.invoke('get_session', ${JSON.stringify(args)})).then(reply => { document.body.dataset.htmlRust = JSON.stringify({ id: reply.id, createdAt: reply.created_at }); window.__htmlIpcResult = 'resolved'; }, error => { window.__htmlIpcResult = 'rejected: ' + String(error).slice(0, 200); }); } catch (error) { window.__htmlIpcResult = 'threw: ' + String(error).slice(0, 200); }`;
  let probeScript = authorScript;
  const probeSource = productionSource.replace("script-src 'none'", `script-src ${origin}`).replace("connect-src 'none'", 'connect-src ipc: http://ipc.localhost').replace('</body>', `<script src="${probeUrl}"></script></body>`);
  const scriptRoute = route => route.fulfill({ status: 200, contentType: 'application/javascript', body: probeScript });
  await page.route(probeUrl, scriptRoute);
  try {
    const reply = await page.evaluate(payload => window.__TAURI_INTERNALS__.invoke('get_session', payload), args);
    if (reply?.id !== sessionId || typeof reply?.created_at !== 'string') throw new Error('Allowed Rust read positive control did not return the seeded session');
    const expectedReceipt = JSON.stringify({ id: reply.id, createdAt: reply.created_at });
    report.security.rustIpcControl = { id: reply.id, createdAt: reply.created_at };
    // The parent CSP permits this same-origin, driver-fulfilled script. Only the
    // controlled test frame grants it; production srcdoc remains script-src none.
    // First isolate the script restriction without making an opaque-origin IPC
    // attempt: the separate IPC mutation removes both forbidden sandbox tokens.
    await expectedViolations(() => reloadFrame((node, source) => { node.srcdoc = source; }, probeSource), 'frame');
    const authorInert = async () => {
      const author = await frame.evaluate(() => document.body.dataset.htmlAuthor ?? null);
      return { passed: author === null, details: { author, sandbox: await preview.getAttribute('sandbox') } };
    };
    await check('html-script', authorInert);
    await prove('html-script', authorInert, async () => {
      const undo = () => expectedViolations(() => reloadFrame(node => { node.setAttribute('sandbox', ''); node.srcdoc = node.srcdoc; }), 'frame');
      try {
        await reloadFrame(node => { node.setAttribute('sandbox', 'allow-scripts'); node.srcdoc = node.srcdoc; });
        await frame.waitForFunction(() => document.body.dataset.htmlAuthor === 'ran');
      } catch (error) { await undo(); throw error; }
      return undo;
    });
    probeScript = ipcScript;
    await expectedViolations(() => reloadFrame(node => { node.srcdoc = node.srcdoc; }), 'frame');
    const ipc = async () => {
      const state = await frame.evaluate(() => ({ executed: document.body.dataset.htmlAuthor === 'ran', receipt: document.body.dataset.htmlRust ?? null, result: window.__htmlIpcResult ?? 'not-run' }));
      const details = { ...state, expectedReceipt, rustControl: report.security.rustIpcControl, bridge, protocolRequests: [...ipcRequests], sandbox: await preview.getAttribute('sandbox') };
      return { passed: !details.executed && details.receipt === null && !bridge.protocolSucceeded && ipcRequests.length === 0 && details.sandbox === '', details };
    };
    await check('html-ipc', ipc);
    report.security.ipcEvidence = {
      ruling: 'R-C3',
      provisional: true,
      liveBaseline: await ipc(),
      liveRustControl: report.security.rustIpcControl,
      recordedOpaqueOrigin: {
        source: 'https://github.com/rdfitted/hive-manager/issues/400#issuecomment-6634048695',
        run: 'render3', sandbox: 'allow-scripts', author: 'ran', receipt: null,
        result: 'rejected: Origin header is not a valid URL',
      },
      gatingMutation: 'html-script',
      stoppedMutation: 'html-ipc two-token mutation; no further attempts after repair3',
    };
    console.log(`render:check R-C3 IPC evidence ${JSON.stringify(report.security.ipcEvidence)}`);
  } finally {
    await page.unroute(probeUrl, scriptRoute);
    await expectedViolations(() => reloadFrame((node, source) => { node.setAttribute('sandbox', ''); node.srcdoc = source; }, productionSource), 'frame');
  }
  const apiBase = await page.evaluate(() => window.__HIVE_INSTANCE__.apiBase);
  const url = `${apiBase}/html-probe`;
  const liveUrl = 'http://127.0.0.1:18800/html-probe';
  const network = [];
  const reached = [];
  const onRequest = request => { if (request.url().includes('/html-probe')) network.push(request.url()); };
  page.on('request', onRequest);
  const blockRoute = route => { reached.push(route.request().url()); return route.fulfill({ status: 200, headers: { 'Access-Control-Allow-Origin': '*', 'Content-Type': 'text/plain' }, body: 'synthetic' }); };
  await page.route('**/html-probe*', blockRoute);
  try {
    await expectedViolations(() => frame.evaluate(async endpoints => {
      window.__htmlApi = false;
      for (const endpoint of endpoints) {
        try { await fetch(endpoint); window.__htmlApi = true; } catch {}
        if (!endpoint.includes(':18800/')) {
          const style = document.createElement('style'); style.textContent = `body { background-image: url("${endpoint}?css"); }`; document.head.append(style);
          const image = document.createElement('img'); image.src = `${endpoint}?img`; document.body.append(image);
        }
      }
    }, [url, liveUrl]), 'frame', [url, liveUrl]);
    await page.waitForTimeout(100);
    const api = async () => {
      const details = { succeeded: await frame.evaluate(() => window.__htmlApi), attempted: [...network], reachedTransport: [...reached], url: frame.url() };
      return { passed: details.succeeded === false && details.reachedTransport.length === 0 && details.url === 'about:srcdoc', details };
    };
    await check('html-api', api);
    await prove('html-api', api, async () => {
      // A fresh srcdoc without its own CSP inherits the parent's loopback
      // allowance. Fulfill the real request in the driver, never at the API.
      const original = await preview.getAttribute('srcdoc');
      await reloadFrame(node => { node.srcdoc = node.srcdoc.replace(/<meta http-equiv="Content-Security-Policy"[^>]*>/, ''); });
      await frame.evaluate(async endpoint => { window.__htmlApi = false; try { await fetch(endpoint); window.__htmlApi = true; } catch {} }, url);
      return async () => {
        network.length = 0;
        reached.length = 0;
        await reloadFrame((node, source) => { node.srcdoc = source; }, original);
        await frame.evaluate(() => { window.__htmlApi = false; });
      };
    });
  } finally {
    page.off('request', onRequest);
    await page.unroute('**/html-probe*', blockRoute);
    await pane.getByRole('button', { name: 'Close HTML preview', exact: true }).click();
    await page.waitForTimeout(100);
    // WebView2 can deliver this one controlled onerror CSP diagnostic after the
    // evaluation call returns. Capture one late occurrence, then exact replays.
    const handlerIndex = report.security.handlerDispatched ? consoleErrors.findIndex(message => /^Executing inline event handler.*Content Security Policy/i.test(message)) : -1;
    if (handlerIndex >= 0) report.security.violations.push({ kind: 'handler-late', message: consoleErrors.splice(handlerIndex, 1)[0] });
    // CDP can replay these exact frame diagnostics when a new execution context
    // is enabled during a navigation mutation. Keep the replay as probe evidence;
    // no generic CSP filter applies to the rest of the application's console.
    const expected = new Set(report.security.violations.map(item => item.message));
    for (let i = consoleErrors.length - 1; i >= 0; i--) {
      if (expected.has(consoleErrors[i])) report.security.violations.push({ kind: 'probe-replay', message: consoleErrors.splice(i, 1)[0] });
    }
  }
}
