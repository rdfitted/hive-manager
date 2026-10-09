import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = path.resolve(fileURLToPath(new URL('..', import.meta.url)));

function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function inside(root, relative) {
  if (path.isAbsolute(relative) || relative.split(/[\\/]/).includes('..')) {
    throw new Error(`Unsafe manifest path: ${relative}`);
  }
  return path.join(root, relative);
}

export function skillFiles(skill) {
  return skill.files ?? [{ path: skill.path, sha256: skill.sha256 }];
}

export function skillRelativePath(skill, file) {
  return file.path.slice(`workflow-pack/skills/${skill.name}/`.length);
}

export async function readManifest(root = REPO_ROOT) {
  const manifest = JSON.parse(await readFile(path.join(root, 'workflow-pack/manifest.json'), 'utf8'));
  if (![1, 2].includes(manifest.schema_version) || !Array.isArray(manifest.skills) || !manifest.roster) {
    throw new Error('Invalid workflow pack manifest');
  }
  const names = new Set();
  for (const skill of manifest.skills) {
    if (!/^[a-z][a-z0-9-]*$/.test(skill.name) || names.has(skill.name)) {
      throw new Error('Invalid or duplicate skill name in manifest');
    }
    names.add(skill.name);
    if (!Array.isArray(skill.harnesses) || skill.harnesses.length === 0 ||
        skill.harnesses.some((harness) => !['claude', 'codex'].includes(harness))) {
      throw new Error(`Invalid manifest entry for ${skill.name}`);
    }
    const prefix = `workflow-pack/skills/${skill.name}/`;
    if (skill.files !== undefined) {
      if (manifest.schema_version !== 2 || !Array.isArray(skill.files) || !skill.files.length ||
          skill.path !== undefined || skill.sha256 !== undefined) {
        throw new Error(`Invalid manifest files for ${skill.name}`);
      }
    } else if (skill.path !== `${prefix}SKILL.md`) {
      throw new Error(`Invalid manifest entry for ${skill.name}`);
    }
    const files = new Set();
    for (const file of skillFiles(skill)) {
      if (typeof file.path !== 'string' || !file.path.startsWith(prefix) ||
          file.path.includes('\\') || file.path.includes(':') ||
          file.path.split('/').some((part) => !part || part === '.' || part === '..') ||
          files.has(file.path.toLowerCase()) || !/^[a-f0-9]{64}$/.test(file.sha256)) {
        throw new Error(`Invalid manifest file for ${skill.name}`);
      }
      inside(root, file.path);
      files.add(file.path.toLowerCase());
    }
    if (!skillFiles(skill).some((file) => file.path === `${prefix}SKILL.md`)) {
      throw new Error(`Missing SKILL.md for ${skill.name}`);
    }
  }
  if (manifest.roster.path !== 'workflow-pack/agent-roster.md' ||
      !/^[a-f0-9]{64}$/.test(manifest.roster.sha256)) {
    throw new Error('Invalid roster entry in manifest');
  }
  return manifest;
}

export async function syncWorkflows({ root = REPO_ROOT, check = false } = {}) {
  const manifest = await readManifest(root);
  const stale = [];
  for (const skill of manifest.skills) {
    for (const file of skillFiles(skill)) {
      const source = await readFile(inside(root, file.path));
      if (hash(source) !== file.sha256) throw new Error(`Manifest hash is stale for ${skill.name}: ${file.path}`);
      for (const [harness, targetRoot] of [['claude', '.claude/skills'], ['codex', '.agents/skills']]) {
        if (!skill.harnesses.includes(harness)) continue;
        const target = inside(root, `${targetRoot}/${skill.name}/${skillRelativePath(skill, file)}`);
        let current;
        try { current = await readFile(target); }
        catch (error) { if (error.code !== 'ENOENT') throw error; }
        if (current && current.equals(source)) continue;
        stale.push(path.relative(root, target));
        if (!check) {
          await mkdir(path.dirname(target), { recursive: true });
          await writeFile(target, source);
        }
      }
    }
  }
  const roster = await readFile(inside(root, manifest.roster.path));
  if (hash(roster) !== manifest.roster.sha256) throw new Error('Manifest hash is stale for roster');
  for (const targetRoot of ['.claude', '.agents']) {
    const target = inside(root, `${targetRoot}/agent-roster.md`);
    let current;
    try { current = await readFile(target); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (current && current.equals(roster)) continue;
    stale.push(path.relative(root, target));
    if (!check) {
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, roster);
    }
  }
  return stale;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.some((arg) => arg !== '--check')) {
    console.error('Usage: node scripts/sync-workflows.mjs [--check]');
    process.exitCode = 2;
  } else {
    try {
      const stale = await syncWorkflows({ check: args.includes('--check') });
      if (args.includes('--check') && stale.length) {
        for (const file of stale) console.error(`Stale workflow copy: ${file}`);
        process.exitCode = 1;
      } else {
        console.log(args.includes('--check') ? 'Workflow copies current' : `Synchronized ${stale.length} workflow copies`);
      }
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
