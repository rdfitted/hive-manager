import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdir, readFile, readdir, writeFile, rm, lstat, realpath } from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import { seed, manifest } from './seed.mjs';
import { repo, target, ensureBuild } from './build.mjs';
import { trackedSnapshot, trackedChanges } from './integrity.mjs';
import { acquireGate } from './gate.mjs';
import { runRenderAssertions, diagnoseReview } from './assertions.mjs';
import { classifyFinding } from './findings.mjs';
import { failureDiagnostic } from './diagnostics.mjs';
import { mutationProver, reviewProofNames, consoleProofMessage } from './mutations.mjs';
import { keyboardProofNames } from './keyboard.mjs';
import { ps, quote, identity, sameProcess, tree, killOwned, liveState, liveUnchanged, desktopState, environment, inside, plainPath, sleep } from './windows.mjs';

if (process.platform !== 'win32') throw new Error('render:check requires Windows WebView2');
const pool = path.join(process.env.LOCALAPPDATA, 'hmr');
const out = path.join(repo, 'tools/render/out');
await mkdir(out, { recursive: true });
const runId = `${new Date().toISOString().replaceAll(':', '-')}-${randomBytes(2).toString('hex')}`;
const captureOut = path.join(out, 'runs', runId);
await mkdir(captureOut, { recursive: true });
const controller = new AbortController();
let browser, page;
const interrupt = () => { controller.abort(); void browser?.close().catch(() => {}); };
process.once('SIGINT', interrupt);
process.once('SIGTERM', interrupt);
const report = { version: 1, runId, phase: 'lifecycle', startedAt: new Date().toISOString(), assertions: [], screenshots: [], mutations: [] };
const requests = [], consoleErrors = [];
const errorReads = [];
const knownFindings = JSON.parse(await readFile(path.join(repo, 'tools/render/known-findings.json'), 'utf8'));
const assertion = (name, passed, details, context = {}) => {
  const result = classifyFinding(name, passed, details, knownFindings);
  report.assertions.push({ name, ...result, details, ...context });
  if (!result.accepted) console.log(failureDiagnostic({ name, status: result.status, details, context }, { instanceRoot: root }));
  return result.accepted;
};
const stateFile = root => path.join(root, 'render-run.json');
async function validateRoot(root) {
  if (path.dirname(root) !== pool || !/^[a-f0-9]{8}$/.test(path.basename(root))) throw new Error('Unsafe instance cleanup path');
  if ((await lstat(root)).isSymbolicLink() || plainPath(await realpath(root)).toLowerCase() !== root.toLowerCase()) throw new Error('Refusing a linked instance directory');
  await readFile(path.join(root, '.hive-instance'));
}
async function removeProfile(root) {
  await validateRoot(root);
  for (let i = 0; i < 20; i++) {
    try { await rm(root, { recursive: true }); return; }
    catch (error) { if (i === 19) throw error; await sleep(500); }
  }
}
async function stopRecord(state) {
  if (state.exe !== target || state.app && state.app.exe?.toLowerCase() !== target.toLowerCase()) throw new Error('Run record does not own the expected guarded executable');
  if (state.app) await killOwned(state.app);
  for (const child of state.processes ?? []) {
    if (child.pid === state.app?.pid) continue;
    const current = identity(child.pid);
    if (!sameProcess(child, current)) continue;
    if (path.basename(current.exe).toLowerCase() !== 'msedgewebview2.exe' || !current.commandLine?.toLowerCase().includes(`${state.root}${path.sep}`.toLowerCase())) throw new Error('Refusing to reap an unverified orphan child');
    await killOwned(child);
  }
  const raw = ps(`@(Get-CimInstance Win32_Process|Where-Object{$_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like ${quote(`*${state.root}*`)}}|Select-Object -ExpandProperty ProcessId)-join ','`);
  if (raw) throw new Error(`Instance browser process remains: ${raw}`);
}
async function reapStale() {
  await mkdir(pool, { recursive: true });
  const reaped = [];
  for (const entry of await readdir(pool, { withFileTypes: true })) {
    if (!entry.isDirectory() || !/^[a-f0-9]{8}$/.test(entry.name)) continue;
    const root = path.join(pool, entry.name);
    let state;
    try { state = JSON.parse(await readFile(stateFile(root), 'utf8')); } catch { continue; }
    if (state.marker !== 'hm-render-v1' || state.root !== root || state.exe !== target) continue;
    await validateRoot(root);
    if (sameProcess(state.driver, identity(state.driver.pid))) throw new Error('An active render run still owns an instance');
    await stopRecord(state);
    await removeProfile(root);
    reaped.push(entry.name);
  }
  return reaped;
}
const freePort = () => new Promise((resolve, reject) => { const server = net.createServer(); server.on('error', reject); server.listen(0, '127.0.0.1', () => { const number = server.address().port; server.close(() => number === 18800 ? freePort().then(resolve, reject) : resolve(number)); }); });
let gate, state, root, app, trackedBefore;
try {
  report.liveBefore = await liveState();
  report.desktopBefore = desktopState();
  gate = await acquireGate(repo, pool, controller.signal);
  trackedBefore = await trackedSnapshot(repo);
  report.staleRunsReaped = await reapStale();
  report.build = await ensureBuild(out, gate, controller.signal);
  if (controller.signal.aborted) throw new Error('Render check interrupted');
  root = path.join(pool, randomBytes(4).toString('hex'));
  const seeded = await seed(root, new Date());
  report.seededAt = seeded.seededAt;
  report.profileDirectoryBeforeLaunch = !!(await lstat(path.join(root, 'home/AppData/Local')).catch(() => null));
  state = { marker: 'hm-render-v1', root, exe: target, driver: identity(process.pid), app: null, processes: [] };
  await writeFile(stateFile(root), JSON.stringify(state, null, 2));
  const api = await freePort();
  let cdp = await freePort();
  while (cdp === api) cdp = await freePort();
  report.ports = { api, cdp };
  report.root = root;
  app = spawn(target, [], { env: { ...process.env, HIVE_MANAGER_INSTANCE_DIR: root, HIVE_MANAGER_INSTANCE_PORT: String(api), HIVE_MANAGER_INSTANCE_CDP_PORT: String(cdp) }, windowsHide: true, stdio: 'ignore' });
  let spawnError;
  app.on('error', error => { spawnError = error; });
  state.app = identity(app.pid);
  report.ownedApp = state.app;
  await writeFile(stateFile(root), JSON.stringify(state, null, 2));
  let ready = false;
  for (let i = 0; i < 120; i++) {
    if (controller.signal.aborted || app.exitCode !== null || spawnError) throw new Error('Guarded instance stopped before CDP readiness');
    try { if ((await fetch(`http://127.0.0.1:${cdp}/json/version`, { signal: AbortSignal.timeout(1000) })).ok) { ready = true; break; } } catch {}
    await sleep(500);
  }
  if (!ready) throw new Error('Guarded CDP endpoint did not become ready');
  state.processes = tree(app.pid);
  report.ownedProcesses = state.processes;
  await writeFile(stateFile(root), JSON.stringify(state, null, 2));
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdp}`, { timeout: 15000 });
  const context = browser.contexts()[0];
  context.on('request', request => { try { if (new URL(request.url()).port === '18800') requests.push(request.url()); } catch {} });
  report.httpErrors = [];
  context.on('response', response => {
    if (response.status() >= 400) errorReads.push(response.text().catch(() => '').then(body => report.httpErrors.push({ url: response.url(), status: response.status(), body: body.slice(0, 4000) })));
  });
  await context.route(/https?:\/\/[^/]*:18800(?:\/|$)/, route => route.abort());
  page = context.pages()[0];
  if (!page) throw new Error('No guarded webview page');
  page.setDefaultTimeout(10000);
  report.initialPageUrl = page.url();
  page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()); });
  page.on('pageerror', error => consoleErrors.push(error.message));
  // CDP can expose the initial about:blank target before Tauri navigates it.
  // Reloading that target races the custom-protocol navigation.
  await page.waitForURL(url => url.hostname === 'tauri.localhost', { timeout: 30000, waitUntil: 'domcontentloaded' });
  report.resourcesBeforeReload = await page.evaluate(() => performance.getEntriesByType('resource').map(entry => entry.name));
  await page.reload({ waitUntil: 'domcontentloaded' });
  // The default 250 entries fill during the 66-second observation. Keep late
  // requests observable, and fail through the console gate if capacity is hit.
  report.resourceTimingBufferSize = 20000;
  await page.evaluate(size => {
    performance.setResourceTimingBufferSize(size);
    performance.addEventListener('resourcetimingbufferfull', () => console.error('render:check resource timing buffer full'));
  }, report.resourceTimingBufferSize);
  await page.waitForFunction(name => document.body.innerText.includes(name), manifest.sessions[0].name);
  assertion('custom-protocol-origin', new URL(page.url()).hostname === 'tauri.localhost', page.url());
  const marker = await page.evaluate(() => window.__HIVE_INSTANCE__);
  assertion('isolated-api-base', marker?.isolated && marker.apiBase === `http://127.0.0.1:${api}`, marker);
  const roots = environment(state.app, repo);
  const consoleHosts = state.processes.filter(child => path.basename(child.exe ?? '').toLowerCase() === 'conhost.exe');
  const exceptions = consoleHosts.map(child => ({ name: 'console-host-inherited-environment', process: child, roots: environment(child, repo), reason: 'Bootstrap redirects the app after startup; pre-rooted HOME is refused by resolve. Console host has no profile path argument; no external read or write was observed.' }));
  for (const exception of exceptions) {
    const command = (exception.process.commandLine ?? exception.process.CommandLine ?? '').toLowerCase();
    exception.noProfilePathArgument = !!command && Object.values(exception.roots).every(value => !command.includes(value.toLowerCase()));
  }
  assertion('environment-contained', Object.values(roots).length === 9 && Object.values(roots).every(value => inside(root, value)) && exceptions.every(item => item.noProfilePathArgument), { appRoots: roots, exceptions });
  report.environmentNonVerbatim = Object.values(roots).every(value => !value.startsWith('\\\\?\\'));
  report.profileDirectoryAfterBootstrap = !!(await lstat(path.join(root, 'home/AppData/Local')).catch(() => null));
  if (process.argv.includes('--force-failure')) throw new Error('forced-mid-run-failure');
  if (process.argv.includes('--verify-terminal-input-fix')) {
    report.phase = 'diagnostic';
    const { verifyTerminalInputFix } = await import('./terminal-input.mjs');
    await verifyTerminalInputFix({ page, manifest, report, consoleErrors, assert: assertion });
  } else if (process.argv.includes('--diagnose-review')) {
    report.phase = 'diagnostic';
    await diagnoseReview({ page, manifest, report, out: captureOut });
  } else {
    report.phase = 'render';
    await runRenderAssertions({ page, manifest, report, assert: assertion, out: captureOut, repo, signal: controller.signal });
  }
  await Promise.all(errorReads);
  report.resourceUrls = await page.evaluate(() => performance.getEntriesByType('resource').map(entry => entry.name));
  assertion('no-live-api-request', ![...requests, ...report.resourcesBeforeReload, ...report.resourceUrls].some(url => { try { return new URL(url).port === '18800'; } catch { return false; } }), requests);
  assertion('no-console-errors', consoleErrors.length === 0, [...consoleErrors]);
  if (report.phase === 'render') {
    const prove = mutationProver(report, assertion);
    const message = consoleProofMessage(runId);
    await prove('no-console-errors', async () => ({ passed: consoleErrors.length === 0, details: [...consoleErrors] }), async () => {
      await page.evaluate(value => console.error(value), message);
      await page.waitForTimeout(100);
      return async () => {
        const index = consoleErrors.indexOf(message);
        if (index < 0) throw new Error('Deliberate console error was not observed');
        consoleErrors.splice(index, 1);
      };
    });
    const noLive = async () => {
      const resources = await page.evaluate(() => performance.getEntriesByType('resource').map(entry => entry.name));
      const observed = [...requests, ...report.resourcesBeforeReload, ...resources];
      return { passed: !observed.some(url => { try { return new URL(url).port === '18800'; } catch { return false; } }), details: { requests: requests.filter(url => url.includes(':18800')), resources: resources.filter(url => url.includes(':18800')) } };
    };
    await prove('no-live-api-request', noLive, async () => {
      // Fulfill this one synthetic URL inside Playwright: never contact the
      // live service. A completed local response also exercises ResourceTiming;
      // an aborted fetch is visible only to the request listener in WebView2.
      const url = 'http://127.0.0.1:18800/render-mutation-blocked';
      const requestStart = requests.length;
      let fulfilled = false;
      const handler = async route => {
        fulfilled = true;
        await route.fulfill({ status: 200, headers: { 'Access-Control-Allow-Origin': '*', 'Timing-Allow-Origin': '*', 'Content-Type': 'application/json' }, body: '{"synthetic":true}' });
      };
      await context.route(url, handler);
      await page.evaluate(async value => { await (await fetch(value)).text(); }, url);
      await page.waitForTimeout(100);
      const resources = await page.evaluate(() => performance.getEntriesByType('resource').map(entry => entry.name));
      report.blockedNetworkMutation = { fulfilledInsideDriver: fulfilled, requests: requests.slice(requestStart), resources: resources.filter(value => value === url) };
      return async () => {
        await context.unroute(url, handler);
        requests.splice(requestStart);
        await page.evaluate(() => performance.clearResourceTimings());
      };
    });
    assertion('network-mutation-both-observers', report.blockedNetworkMutation?.fulfilledInsideDriver && report.blockedNetworkMutation.requests.length > 0 && report.blockedNetworkMutation.resources.length > 0, report.blockedNetworkMutation);
    const required = ['screen-not-empty', 'screenshot-has-pixels', 'version-badge-unobscured', 'grid-eight-panes', 'grid-composer-with-attention', 'grid-composer-without-attention', 'finished-attention-card', 'finished-needs-you-count', 'stalled-attention-card', 'stalled-needs-you-count', 'exited_during_startup-attention-card', 'exited_during_startup-needs-you-count', 'chat-table', 'chat-code', 'chat-literal', 'kanban-fixture-present', 'sidebar-fixture-present', 'viewport-1440', 'viewport-800', 'updater-invoke-rejected', 'all-screens-captured', 'no-live-api-request', 'no-console-errors'];
    required.push(...reviewProofNames, ...keyboardProofNames);
    required.push('launch-project-verbatim-prefix');
    assertion('all-required-mutations-proved', required.every(name => report.mutations.some(item => item.name === name && item.passed)), { required, proved: report.mutations.filter(item => item.passed).map(item => item.name) });
  }
} catch (error) {
  report.error = error.stack;
  if (page) report.failurePage = await page.evaluate(() => ({ url: location.href, readyState: document.readyState, text: document.body?.innerText.slice(0, 1500) })).catch(() => null);
  assertion(error.message === 'forced-mid-run-failure' ? 'forced-mid-run-failure' : report.phase === 'render' ? 'render-execution' : 'lifecycle', false, error.message);
  if (page) await page.screenshot({ path: path.join(captureOut, 'failure.png') }).catch(() => {});
}
finally {
  await Promise.all(errorReads);
  if (report.phase === 'render' && !report.assertions.some(item => item.name === 'all-screens-captured')) assertion('all-screens-captured', false, report.screenshots.map(item => `${item.screen}-${item.viewport.width}`));
  if (page && !report.assertions.some(item => item.name === 'no-live-api-request')) {
    try {
      report.resourceUrls = await page.evaluate(() => performance.getEntriesByType('resource').map(entry => entry.name));
      assertion('no-live-api-request', ![...requests, ...(report.resourcesBeforeReload ?? []), ...report.resourceUrls].some(url => { try { return new URL(url).port === '18800'; } catch { return false; } }), requests);
    } catch (error) { assertion('no-live-api-request', false, error.message); }
  }
  if (page && !report.assertions.some(item => item.name === 'no-console-errors')) assertion('no-console-errors', consoleErrors.length === 0, consoleErrors);
  if (browser) await browser.close().catch(() => {});
  try {
    if (state) await stopRecord(state);
    if (root) await removeProfile(root);
    assertion('cleanup-no-process-or-profile', true, { app: state?.app, recordedPids: state?.processes.map(item => item.pid), instanceRemoved: !!root, remainingOwnedProcesses: [] });
  } catch (error) { assertion('cleanup-no-process-or-profile', false, error.message); }
  try {
    report.liveAfter = await liveState();
    assertion('G42-live-app-unchanged', liveUnchanged(report.liveBefore, report.liveAfter), { before: report.liveBefore, after: report.liveAfter });
    report.desktopAfter = desktopState();
    report.foregroundPreserved = report.desktopBefore.foreground && report.desktopAfter.foreground ? report.desktopBefore.foreground === report.desktopAfter.foreground : null;
    assertion('input-desktop-unchanged', report.desktopBefore.inputDesktop === report.desktopAfter.inputDesktop, { before: report.desktopBefore, after: report.desktopAfter });
    if (report.foregroundPreserved !== null) assertion('foreground-preserved', report.foregroundPreserved);
  } catch (error) { assertion('G42-live-app-unchanged', false, error.message); }
  if (trackedBefore) {
    try {
      const changed = await trackedChanges(repo, trackedBefore);
      assertion('tracked-files-unchanged', changed.length === 0, { trackedCount: trackedBefore.size, changed });
    } catch (error) { assertion('tracked-files-unchanged', false, error.message); }
  }
  if (gate) { try { await gate.release(); } catch (error) { assertion('gate-release', false, error.message); } }
  report.completedAt = new Date().toISOString();
  report.passed = report.assertions.every(item => item.accepted);
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
  await writeFile(path.join(captureOut, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`render:check ${report.passed ? 'PASS' : 'FAIL'} (${report.phase}); build ${report.build?.reused ? 'reused' : 'built'}`);
  if (!report.passed) process.exitCode = 1;
}
