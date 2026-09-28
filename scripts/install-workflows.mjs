import { createHash } from 'node:crypto';
import { lstat, mkdir, readFile, unlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readManifest } from './sync-workflows.mjs';

const REPO_ROOT = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
const RECORD_FILE = 'workflow-pack-install.json';

function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

async function readOptional(file) {
  try { return await readFile(file); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}

async function refuseSymlink(file) {
  const parsed = path.parse(path.resolve(file));
  let current = parsed.root;
  for (const part of path.relative(parsed.root, path.resolve(file)).split(path.sep)) {
    current = path.join(current, part);
    try {
      const stat = await lstat(current);
      if (stat.isSymbolicLink()) throw new Error(`Symlink in install path: ${current}`);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
}

function destinations({ home, codexHome, harness }) {
  const selected = harness === 'both' ? ['claude', 'codex'] : [harness];
  return selected.map((name) => ({
    name,
    root: name === 'claude' ? path.join(home, '.claude') : (codexHome || path.join(home, '.codex')),
  }));
}

function belongsToDestination(target, destination) {
  const relative = path.relative(destination.root, target);
  return relative && !relative.startsWith('..') && !path.isAbsolute(relative) &&
    (relative === 'agent-roster.md' || /^skills[/\\][a-z][a-z0-9-]*[/\\]SKILL\.md$/.test(relative));
}

export async function installWorkflows({
  root = REPO_ROOT,
  harness,
  dryRun = false,
  uninstall = false,
  home = os.homedir(),
  codexHome = process.env.CODEX_HOME,
  // Match src-tauri/src/storage/mod.rs::get_app_data_dir exactly.
  appConfigDir = process.platform === 'win32'
    ? path.join(process.env.APPDATA || path.join(home, 'AppData', 'Roaming'), 'hive-manager')
    : path.join(home, '.config', 'hive-manager'),
} = {}) {
  if (!['claude', 'codex', 'both'].includes(harness)) throw new Error('Choose --harness claude|codex|both');
  const manifest = await readManifest(root);
  const recordPath = path.join(appConfigDir, RECORD_FILE);
  await refuseSymlink(recordPath);
  const currentRecord = await readOptional(recordPath);
  const record = currentRecord ? JSON.parse(currentRecord.toString('utf8')) : { schema_version: 1, files: {} };
  if (record.schema_version !== 1 || !record.files || typeof record.files !== 'object') {
    throw new Error('Invalid workflow install ownership manifest');
  }
  const actions = [];
  let changed = false;
  const selected = destinations({ home, codexHome, harness });
  if (uninstall) {
    const candidates = new Set();
    for (const destination of selected) {
      for (const skill of manifest.skills.filter((item) => item.harnesses.includes(destination.name))) {
        candidates.add(path.join(destination.root, 'skills', skill.name, 'SKILL.md'));
      }
      candidates.add(path.join(destination.root, 'agent-roster.md'));
    }
    for (const target of Object.keys(record.files)) {
      if (selected.some((destination) => belongsToDestination(target, destination))) candidates.add(target);
    }
    for (const target of candidates) {
      await refuseSymlink(target);
      const existing = await readOptional(target);
      const installed = record.files[target];
      if (!installed) {
        if (existing) actions.push({ kind: 'unowned', target });
        continue;
      }
      if (!existing) {
        actions.push({ kind: 'missing', target });
        if (!dryRun) { delete record.files[target]; changed = true; }
      } else if (hash(existing) !== installed.sha256) {
        actions.push({ kind: 'collision', target });
      } else {
        actions.push({ kind: 'remove', target });
        if (!dryRun) { await unlink(target); delete record.files[target]; changed = true; }
      }
    }
  } else {
    for (const destination of selected) {
      const items = manifest.skills.filter((skill) => skill.harnesses.includes(destination.name)).map((skill) => ({
        source: skill.path,
        expected: skill.sha256,
        target: path.join(destination.root, 'skills', skill.name, 'SKILL.md'),
      }));
      items.push({
        source: manifest.roster.path,
        expected: manifest.roster.sha256,
        target: path.join(destination.root, 'agent-roster.md'),
      });
      for (const item of items) {
        const source = await readFile(path.join(root, item.source));
        if (hash(source) !== item.expected) throw new Error(`Manifest hash is stale for ${item.source}`);
        await refuseSymlink(item.target);
        const existing = await readOptional(item.target);
        const installed = record.files[item.target];
        if (existing?.equals(source)) {
          actions.push({ kind: 'identical', target: item.target });
          continue;
        }
        if (existing && (!installed || installed.sha256 !== hash(existing) || installed.source !== item.source)) {
          actions.push({ kind: 'collision', target: item.target });
          continue;
        }
        const kind = existing ? 'update' : 'install';
        actions.push({ kind, target: item.target });
        if (!dryRun) {
          await mkdir(path.dirname(item.target), { recursive: true });
          await writeFile(item.target, source);
          record.files[item.target] = { source: item.source, sha256: item.expected };
          changed = true;
        }
      }
    }
  }
  if (!dryRun && changed) {
    await mkdir(appConfigDir, { recursive: true });
    await writeFile(recordPath, `${JSON.stringify(record, null, 2)}\n`);
  }
  return actions;
}

function parseArgs(args) {
  let harness;
  let user = false;
  let dryRun = false;
  let uninstall = false;
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === '--user') user = true;
    else if (arg === '--dry-run') dryRun = true;
    else if (arg === '--uninstall') uninstall = true;
    else if (arg === '--harness' && args[index + 1]) harness = args[++index];
    else throw new Error('Usage: node scripts/install-workflows.mjs --user --harness claude|codex|both [--dry-run] [--uninstall]');
  }
  if (!user) throw new Error('Installation requires --user');
  return { harness, dryRun, uninstall };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const actions = await installWorkflows(parseArgs(process.argv.slice(2)));
    for (const action of actions) console.log(`${action.kind}: ${action.target}`);
    if (actions.some((action) => ['collision', 'unowned'].includes(action.kind))) process.exitCode = 1;
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
