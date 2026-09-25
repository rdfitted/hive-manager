import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { flagValue } from './cli-args.mjs';

function cargoVersion(text) {
  const match = text.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error('Cargo.toml has no package version');
  return match[1];
}

function lockVersion(text) {
  const block = text.split(/^\[\[package\]\]\s*$/m).find((entry) => /^name\s*=\s*"hive-manager"\s*$/m.test(entry));
  const match = block?.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error('Cargo.lock has no hive-manager package version');
  return match[1];
}

export function readVersions(root) {
  const json = (file) => JSON.parse(readFileSync(resolve(root, file), 'utf8'));
  const lock = json('package-lock.json');
  return {
    'package.json': json('package.json').version,
    'package-lock.json': lock.version,
    'package-lock.json packages[""]': lock.packages?.['']?.version,
    'tauri.conf.json': json('src-tauri/tauri.conf.json').version,
    'Cargo.toml': cargoVersion(readFileSync(resolve(root, 'src-tauri/Cargo.toml'), 'utf8')),
    'Cargo.lock': lockVersion(readFileSync(resolve(root, 'src-tauri/Cargo.lock'), 'utf8')),
  };
}

export function checkVersion({ root = '.', tag, dryRun = false } = {}) {
  const versions = readVersions(root);
  const values = Object.entries(versions);
  const expected = values[0][1];
  for (const [source, version] of values) {
    if (!version || version !== expected) throw new Error(`${source} version ${version ?? '(missing)'} differs from ${expected}`);
  }
  if (!dryRun) {
    if (!/^v\d+\.\d+\.\d+$/.test(tag ?? '')) throw new Error('An existing vX.Y.Z tag is required');
    if (tag.slice(1) !== expected) throw new Error(`Tag ${tag} differs from ${expected}`);
  }
  return expected;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  try {
    const version = checkVersion({
      root: flagValue(args, '--root') ?? '.',
      tag: flagValue(args, '--tag'),
      dryRun: args.includes('--dry-run'),
    });
    console.log(`Version sources agree: ${version}`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
