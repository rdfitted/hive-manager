import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdir, readFile, readdir, writeFile, rm, lstat, realpath } from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import { seed, manifest } from './seed.mjs';
import { repo, target, ensureBuild } from './build.mjs';
import { acquireGate } from './gate.mjs';
import { ps, quote, identity, sameProcess, tree, killOwned, liveState, liveUnchanged, desktopState, environment, inside, plainPath, sleep } from './windows.mjs';

if (process.platform !== 'win32') throw new Error('render:check requires Windows WebView2');
const pool = path.join(process.env.LOCALAPPDATA, 'hmr');
const out = path.join(repo, 'tools/render/out');
await mkdir(out, { recursive: true });
const controller = new AbortController();
let browser;
const interrupt = () => { controller.abort(); void browser?.close().catch(() => {}); };
process.once('SIGINT', interrupt);
process.once('SIGTERM', interrupt);
const report = { version: 1, phase: 'lifecycle', startedAt: new Date().toISOString(), assertions: [], screenshots: [], mutations: [] };
const assertion = (name, passed, details) => { report.assertions.push({ name, passed: !!passed, details }); return !!passed; };
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
let gate, state, root, app;
try {
  report.liveBefore = await liveState();
  report.desktopBefore = desktopState();
  gate = await acquireGate(repo, pool, controller.signal);
  report.staleRunsReaped = await reapStale();
  report.build = await ensureBuild(out, gate, controller.signal);
  if (controller.signal.aborted) throw new Error('Render check interrupted');
  root = path.join(pool, randomBytes(4).toString('hex'));
  const seeded = await seed(root, new Date());
  report.seededAt = seeded.seededAt;
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
  await writeFile(stateFile(root), JSON.stringify(state, null, 2));
  let ready = false;
  for (let i = 0; i < 120; i++) {
    if (controller.signal.aborted || app.exitCode !== null || spawnError) throw new Error('Guarded instance stopped before CDP readiness');
    try { if ((await fetch(`http://127.0.0.1:${cdp}/json/version`, { signal: AbortSignal.timeout(1000) })).ok) { ready = true; break; } } catch {}
    await sleep(500);
  }
  if (!ready) throw new Error('Guarded CDP endpoint did not become ready');
  state.processes = tree(app.pid);
  await writeFile(stateFile(root), JSON.stringify(state, null, 2));
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdp}`, { timeout: 15000 });
  const context = browser.contexts()[0];
  const requests = [], consoleErrors = [];
  context.on('request', request => { try { if (new URL(request.url()).port === '18800') requests.push(request.url()); } catch {} });
  await context.route(/https?:\/\/[^/]*:18800(?:\/|$)/, route => route.abort());
  const page = context.pages()[0];
  if (!page) throw new Error('No guarded webview page');
  page.setDefaultTimeout(10000);
  page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()); });
  page.on('pageerror', error => consoleErrors.push(error.message));
  report.resourcesBeforeReload = await page.evaluate(() => performance.getEntriesByType('resource').map(entry => entry.name));
  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.waitForFunction(name => document.body.innerText.includes(name), manifest.sessions[0].name);
  assertion('custom-protocol-origin', new URL(page.url()).hostname === 'tauri.localhost', page.url());
  const marker = await page.evaluate(() => window.__HIVE_INSTANCE__);
  assertion('isolated-api-base', marker?.isolated && marker.apiBase === `http://127.0.0.1:${api}`, marker);
  const roots = environment(state.app, repo);
  assertion('environment-contained', Object.values(roots).length === 9 && Object.values(roots).every(value => inside(root, value)), roots);
  if (process.argv.includes('--force-failure')) throw new Error('forced-mid-run-failure');
  for (const viewport of [{ width: 1440, height: 900 }, { width: 800, height: 900 }]) {
    await page.setViewportSize(viewport);
    await page.waitForTimeout(150);
    const measured = await page.evaluate(() => ({ width: innerWidth, height: innerHeight, devicePixelRatio, text: document.body.innerText }));
    assertion(`viewport-${viewport.width}`, measured.width === viewport.width && measured.height === viewport.height, measured);
    assertion(`launch-not-empty-${viewport.width}`, measured.text.includes(manifest.sessions[0].name) && measured.text.includes('What would you like to work on?'));
    const png = path.join(out, `launch-${viewport.width}.png`);
    await page.screenshot({ path: png });
    report.screenshots.push({ screen: 'launch', viewport, path: path.relative(repo, png).replaceAll('\\', '/') });
  }
  report.resourceUrls = await page.evaluate(() => performance.getEntriesByType('resource').map(entry => entry.name));
  assertion('no-live-api-request', ![...requests, ...report.resourcesBeforeReload, ...report.resourceUrls].some(url => { try { return new URL(url).port === '18800'; } catch { return false; } }), requests);
  assertion('no-console-errors', consoleErrors.length === 0, consoleErrors);
} catch (error) { report.error = error.stack; assertion(error.message === 'forced-mid-run-failure' ? 'forced-mid-run-failure' : 'lifecycle', false, error.message); }
finally {
  if (browser) await browser.close().catch(() => {});
  try {
    if (state) await stopRecord(state);
    if (root) await removeProfile(root);
    assertion('cleanup-no-process-or-profile', true);
  } catch (error) { assertion('cleanup-no-process-or-profile', false, error.message); }
  try {
    report.liveAfter = await liveState();
    assertion('G42-live-app-unchanged', liveUnchanged(report.liveBefore, report.liveAfter), { before: report.liveBefore, after: report.liveAfter });
    report.desktopAfter = desktopState();
    report.foregroundPreserved = report.desktopBefore.foreground && report.desktopAfter.foreground ? report.desktopBefore.foreground === report.desktopAfter.foreground : null;
    assertion('input-desktop-unchanged', report.desktopBefore.inputDesktop === report.desktopAfter.inputDesktop, { before: report.desktopBefore, after: report.desktopAfter });
    if (report.foregroundPreserved !== null) assertion('foreground-preserved', report.foregroundPreserved);
  } catch (error) { assertion('G42-live-app-unchanged', false, error.message); }
  if (gate) { try { await gate.release(); } catch (error) { assertion('gate-release', false, error.message); } }
  report.completedAt = new Date().toISOString();
  report.passed = report.assertions.every(item => item.passed);
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
  console.log(`render:check ${report.passed ? 'PASS' : 'FAIL'} (${report.phase}); build ${report.build?.reused ? 'reused' : 'built'}`);
  if (!report.passed) process.exitCode = 1;
}
