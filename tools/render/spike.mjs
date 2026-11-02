import { chromium } from 'playwright-core';
import { seed, manifest } from './seed.mjs';
import { spawn, execFileSync } from 'node:child_process';
import { mkdir, writeFile, rm, stat, readdir } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import net from 'node:net';
import path from 'node:path';

const ps = script => execFileSync('powershell.exe', ['-NoProfile', '-Command', script], { encoding: 'utf8', windowsHide: true }).trim();
const desktop = () => JSON.parse(ps(`Add-Type -TypeDefinition 'using System; using System.Text; using System.Runtime.InteropServices; public class RenderFocus { [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow(); [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access); [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr handle); [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern bool GetUserObjectInformation(IntPtr h,int index,StringBuilder buffer,int length,out int needed); public static string InputName() { var h=OpenInputDesktop(0,false,1); if(h==IntPtr.Zero)return "unavailable"; try {var s=new StringBuilder(256);int n;return GetUserObjectInformation(h,2,s,512,out n)?s.ToString():"unavailable";}finally{CloseDesktop(h);} } }'; @{foreground=[RenderFocus]::GetForegroundWindow().ToInt64();inputDesktop=[RenderFocus]::InputName()} | ConvertTo-Json -Compress`));
const foreground = () => desktop().foreground;
const live = async () => ({ health: await (await fetch('http://127.0.0.1:18800/health')).text(), pid: ps('(Get-NetTCPConnection -State Listen -LocalPort 18800 | Select-Object -ExpandProperty OwningProcess -Unique)') });
const port = () => new Promise((resolve, reject) => { const s = net.createServer(); s.on('error', reject); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => p === 18800 ? port().then(resolve, reject) : resolve(p)); }); });
const pause = ms => new Promise(r => setTimeout(r, ms));
const root = path.join(process.env.LOCALAPPDATA, 'hmr', randomBytes(4).toString('hex'));
const out = path.resolve('tools/render/out');
await mkdir(out, { recursive: true });
const desktopBefore = desktop();
const report = { started: new Date().toISOString(), root, before: await live(), inputDesktopBefore: desktopBefore.inputDesktop, foregroundBefore: desktopBefore.foreground, pageLoaded: 'unobserved' };
let child, browser;
try {
  await seed(root, new Date());
  const api = await port(), cdp = await port();
  report.ports = { api, cdp };
  const exe = path.join(process.env.LOCALAPPDATA, 'hive-manager-target/loop1/debug/hive-manager.exe');
  const env = { ...process.env, HIVE_MANAGER_INSTANCE_DIR: root, HIVE_MANAGER_INSTANCE_PORT: String(api), HIVE_MANAGER_INSTANCE_CDP_PORT: String(cdp) };
  if (process.argv.includes('--webdriver')) {
    const driverPort = await port(), nativePort = await port();
    child = spawn(path.join(out, 'driver/bin/tauri-driver.exe'), ['--port', String(driverPort), '--native-port', String(nativePort), '--native-driver', path.join(out, 'edge/msedgedriver.exe')], { env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    report.pid = child.pid;
    report.logs = '';
    child.stdout.on('data', chunk => { report.logs += chunk; });
    child.stderr.on('data', chunk => { report.logs += chunk; });
    child.on('error', e => { report.spawnError = e.message; });
    const command = async (route, body, method = 'POST') => {
      const response = await fetch(`http://127.0.0.1:${driverPort}${route}`, { method, body: body ? JSON.stringify(body) : undefined, headers: { 'Content-Type': 'application/json' }, signal: AbortSignal.timeout(75000) });
      const result = await response.json();
      if (!response.ok || result.value?.error) throw new Error(JSON.stringify(result));
      return result.value;
    };
    for (let i = 0; i < 40; i++) { try { await command('/status', undefined, 'GET'); break; } catch { await pause(250); } }
    report.launch = 'tauri-driver matched Edge driver';
    const session = await command('/session', { capabilities: { alwaysMatch: { 'tauri:options': { application: exe, webviewOptions: { userDataFolder: path.join(root, 'webview'), additionalBrowserArguments: [`--remote-debugging-port=${cdp}`] } } } } });
    report.webdriverSession = session;
    const route = `/session/${session.sessionId}`;
    report.viewports = [];
    for (const width of [1440, 800]) {
      const rect = await command(`${route}/window/rect`, { width: width + 16, height: 939 });
      const measured = await command(`${route}/execute/sync`, { script: 'return {innerWidth,innerHeight,devicePixelRatio,origin:location.origin,bodyText:document.body.innerText.slice(0,2000)}', args: [] });
      report.viewports.push({ rect, ...measured });
    }
    report.foregroundAfterAttach = foreground();
    report.webviewProcesses = ps(`Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like '*${root.replaceAll("'", "''")}*' } | Select-Object ProcessId,ParentProcessId,CommandLine | ConvertTo-Json -Depth 3`);
    report.attached = true;
    report.pageLoaded = report.viewports.every(v => v.origin.includes('tauri.localhost') && v.bodyText.length > 0);
    await command(route, undefined, 'DELETE');
  } else {
  if (process.argv.includes('--native-launch')) {
    const pid = execFileSync('powershell.exe', ['-NoProfile', '-Command', `$info = New-Object System.Diagnostics.ProcessStartInfo; $info.FileName = '${exe.replaceAll("'", "''")}'; $info.UseShellExecute = $false; $info.CreateNoWindow = $true; $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true; $p = [System.Diagnostics.Process]::Start($info); $p.Id`], { env, windowsHide: true, encoding: 'utf8' }).trim();
    child = { pid: Number(pid), exitCode: null };
    report.launch = 'ProcessStartInfo CreateNoWindow, no WindowStyle override';
  } else {
    child = spawn(exe, [], { env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    report.launch = 'Node windowsHide';
  }
  report.logs = '';
  child.stdout?.on('data', chunk => { report.logs += chunk; });
  child.stderr?.on('data', chunk => { report.logs += chunk; });
  report.pid = child.pid;
  child.on?.('error', e => { report.spawnError = e.message; });
  let ready = false;
  for (let n = 0; n < 120; n++) {
    if (child.exitCode !== null || report.spawnError) throw new Error(`Instance failed: ${child.exitCode ?? report.spawnError}`);
    try { if ((await fetch(`http://127.0.0.1:${cdp}/json/version`, { signal: AbortSignal.timeout(1000) })).ok) { ready = true; break; } } catch {}
    await pause(500);
  }
  if (!ready) {
    report.devtoolsFiles = (await readdir(path.join(root, 'webview'), { recursive: true })).filter(p => /DevTools|debug\.log/i.test(p));
    report.webviewProcesses = ps(`Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like '*${root.replaceAll("'", "''")}*' } | Select-Object ProcessId,ParentProcessId,CommandLine | ConvertTo-Json -Depth 3`);
    report.listeners = ps('Get-NetTCPConnection -State Listen | Select-Object LocalAddress,LocalPort,OwningProcess | ConvertTo-Json');
    throw new Error('CDP endpoint did not become ready');
  }
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdp}`);
  const context = browser.contexts()[0];
  report.requests18800 = [];
  context.on('request', request => { try { if (new URL(request.url()).port === '18800') report.requests18800.push(request.url()); } catch {} });
  await context.route(/https?:\/\/[^/]*:18800(?:\/|$)/, route => route.abort());
  let page = context.pages()[0];
  for (let n = 0; n < 40 && (!page || !page.url().includes('tauri.localhost')); n++) { await pause(250); page = context.pages()[0]; }
  if (!page) throw new Error('No webview page');
  await page.waitForLoadState('domcontentloaded');
  report.resourcesBeforeReload = await page.evaluate(() => performance.getEntriesByType('resource').map(e => e.name));
  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.waitForFunction(name => document.body.innerText.includes(name), manifest.sessions[0].name, { timeout: 10000 });
  report.url = page.url();
  report.pageLoaded = await page.evaluate(() => location.hostname === 'tauri.localhost' && document.body.innerText.length > 0);
  report.viewports = [];
  for (const size of [{ width: 1440, height: 900 }, { width: 800, height: 900 }]) {
    await page.setViewportSize(size);
    await pause(1000);
    report.viewports.push(await page.evaluate(() => ({ innerWidth, innerHeight, devicePixelRatio, origin: location.origin, instance: window.__HIVE_INSTANCE__, bodyText: document.body.innerText.slice(0, 2000) })));
    await page.screenshot({ path: path.join(out, `spike-${size.width}.png`) });
  }
  report.foregroundAfterAttach = foreground();
  report.resourceUrls = await page.evaluate(() => performance.getEntriesByType('resource').map(e => e.name));
  report.noRequest18800 = ![...report.requests18800, ...report.resourcesBeforeReload, ...report.resourceUrls].some(url => { try { return new URL(url).port === '18800'; } catch { return false; } });
  const identity = JSON.parse(ps(`$p=Get-CimInstance Win32_Process -Filter 'ProcessId=${child.pid}';@{exe=$p.ExecutablePath;started=$p.CreationDate.ToFileTimeUtc().ToString()}|ConvertTo-Json -Compress`));
  report.environmentRoots = JSON.parse(ps(`& '${path.resolve('tools/render/environment.ps1').replaceAll("'", "''")}' -ProcessId ${child.pid} -ExpectedExe '${identity.exe.replaceAll("'", "''")}' -ExpectedStart '${identity.started}'`));
  report.environmentContained = ['USERPROFILE', 'HOME', 'APPDATA', 'LOCALAPPDATA', 'TEMP', 'TMP', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'WEBVIEW2_USER_DATA_FOLDER'].every(key => {
    const value = report.environmentRoots[key]?.replace(/^\\\\\?\\/, '');
    return value && path.relative(root, value).split(path.sep)[0] !== '..' && !path.isAbsolute(path.relative(root, value));
  });
  report.webviewProcesses = JSON.parse(ps(`Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like '*${root.replaceAll("'", "''")}*' } | Select-Object ProcessId,ParentProcessId,CommandLine | ConvertTo-Json -Depth 3`));
  await page.screenshot({ path: path.join(out, 'spike.png') });
  report.attached = true;
  }
} catch (e) { report.error = e.stack; process.exitCode = 1; }
finally {
  const processes = ps(`@(Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like '*${root.replaceAll("'", "''")}*' } | Select-Object ProcessId,ParentProcessId,CommandLine) | ConvertTo-Json -Depth 3`);
  report.processesBeforeCleanup = processes ? JSON.parse(processes) : [];
  report.rendererStarted = [report.processesBeforeCleanup].flat().some(p => p.CommandLine?.includes('--type=renderer'));
  if (browser) await browser.close().catch(() => {});
  if (child?.pid) { try { ps(`& taskkill.exe /PID ${child.pid} /T /F | Out-Null`); } catch {} }
  report.leftoverPids = ps(`@(Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -eq ${child?.pid || 0} -or ($_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like '*${root.replaceAll("'", "''")}*') } | Select-Object -ExpandProperty ProcessId) -join ','`);
  if (report.leftoverPids) process.exitCode = 1;
  if (path.dirname(root) !== path.join(process.env.LOCALAPPDATA, 'hmr') || !/^[a-f0-9]{8}$/.test(path.basename(root))) throw new Error('Unsafe cleanup target');
  for (let i = 0; i < 20; i++) { try { await rm(root, { recursive: true, force: true }); break; } catch { await pause(500); } }
  report.profileRemoved = !(await stat(root).catch(() => null));
  if (!report.profileRemoved) process.exitCode = 1;
  report.after = await live();
  const desktopAfter = desktop();
  report.foregroundAfter = desktopAfter.foreground;
  report.inputDesktopAfter = desktopAfter.inputDesktop;
  report.foregroundPreserved = report.foregroundBefore && report.foregroundAfterAttach ? report.foregroundBefore === report.foregroundAfterAttach : null;
  report.inputDesktopUnchanged = report.inputDesktopBefore === report.inputDesktopAfter;
  report.liveUnchanged = JSON.stringify(report.before) === JSON.stringify(report.after);
  if (!report.liveUnchanged) process.exitCode = 1;
  report.phase = 'spike';
  await writeFile(path.join(out, 'spike.json'), JSON.stringify(report, null, 2));
  await writeFile(path.join(out, `spike-${path.basename(root)}.json`), JSON.stringify(report, null, 2));
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ attached: report.attached, error: report.error, viewports: report.viewports?.map(({ bodyText, ...v }) => v), foregroundPreserved: report.foregroundPreserved, inputDesktopBefore: report.inputDesktopBefore, inputDesktopAfter: report.inputDesktopAfter, rendererStarted: report.rendererStarted, pageLoaded: report.pageLoaded, liveUnchanged: report.liveUnchanged, pid: report.pid }));
}
