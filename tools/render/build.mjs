import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { spawn } from 'node:child_process';
import { readdir, readFile, stat, writeFile, open } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { identity, killOwned } from './windows.mjs';
export const repo = fileURLToPath(new URL('../../', import.meta.url));
export const target = path.join(process.env.LOCALAPPDATA ?? '', 'hive-manager-target/loop1/debug/hive-manager.exe');
const folders = ['src', 'src-tauri/src', 'src-tauri/capabilities', 'src-tauri/permissions', 'src-tauri/icons', 'static'];
const files = ['package.json', 'package-lock.json', 'svelte.config.js', 'vite.config.js', 'tsconfig.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', 'src-tauri/tauri.conf.json', 'src-tauri/build.rs'];
async function walk(relative) {
  const entries = await readdir(path.join(repo, relative), { withFileTypes: true }).catch(error => error.code === 'ENOENT' ? [] : Promise.reject(error));
  const output = [];
  for (const entry of entries) {
    const next = path.join(relative, entry.name);
    if (entry.isDirectory()) output.push(...await walk(next));
    else if (entry.isFile()) output.push(next);
    else throw new Error('Build inputs may not contain links');
  }
  return output;
}
export async function inputHash() {
  const inputs = [...files];
  for (const folder of folders) inputs.push(...await walk(folder));
  const hash = createHash('sha256');
  for (const relative of inputs.sort()) {
    try { hash.update(relative.replaceAll('\\', '/')); hash.update('\0'); hash.update(await readFile(path.join(repo, relative))); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  return hash.digest('hex');
}
const executableHash = async () => {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(target)) hash.update(chunk);
  return hash.digest('hex');
};
export const spawnBuildScript = (script, args, options) => spawn('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', script, ...args], options);
export async function ensureBuild(out, gate, signal) {
  const receiptPath = path.join(out, 'build-state.json');
  const fingerprint = await inputHash();
  let previous;
  try { previous = JSON.parse(await readFile(receiptPath, 'utf8')); } catch {}
  if (previous?.mode === 'tauri-debug-custom-protocol' && previous.inputHash === fingerprint && previous.exe === target && await stat(target).catch(() => null) && previous.exeHash === await executableHash()) return { ...previous, reused: true, elapsedSeconds: 0 };
  const log = await open(path.join(out, 'build.log'), 'w');
  const started = Date.now();
  console.log('render:check building the guarded custom-protocol executable');
  const child = spawnBuildScript(path.join(repo, 'tools/render/build.ps1'), ['-Repo', repo, '-GateJson', JSON.stringify(gate)], { cwd: repo, windowsHide: true, stdio: ['ignore', log.fd, log.fd] });
  const owned = identity(child.pid);
  const abort = () => { if (owned) void killOwned(owned).catch(() => {}); };
  signal.addEventListener('abort', abort, { once: true });
  try {
    const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); });
    if (code !== 0 || signal.aborted) throw new Error('Guarded build failed or was interrupted; see build.log');
  } finally { signal.removeEventListener('abort', abort); await log.close(); }
  if (await inputHash() !== fingerprint) throw new Error('Build inputs changed during the build; refusing to launch');
  const receipt = { mode: 'tauri-debug-custom-protocol', inputHash: fingerprint, exe: target, exeHash: await executableHash(), builtAt: new Date().toISOString(), elapsedSeconds: (Date.now() - started) / 1000 };
  await writeFile(receiptPath, JSON.stringify(receipt, null, 2));
  return { ...receipt, reused: false };
}
