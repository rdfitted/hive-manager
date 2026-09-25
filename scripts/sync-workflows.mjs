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

export async function readManifest(root = REPO_ROOT) {
  const manifest = JSON.parse(await readFile(path.join(root, 'workflow-pack/manifest.json'), 'utf8'));
  if (manifest.schema_version !== 1 || !Array.isArray(manifest.skills) || !manifest.roster) {
    throw new Error('Invalid workflow pack manifest');
  }
  const names = new Set();
  for (const skill of manifest.skills) {
    if (!/^[a-z][a-z0-9-]*$/.test(skill.name) || names.has(skill.name)) {
      throw new Error('Invalid or duplicate skill name in manifest');
    }
    names.add(skill.name);
    if (skill.path !== `workflow-pack/skills/${skill.name}/SKILL.md` ||
        !Array.isArray(skill.harnesses) || skill.harnesses.length === 0 ||
        skill.harnesses.some((harness) => !['claude', 'codex'].includes(harness)) ||
        !/^[a-f0-9]{64}$/.test(skill.sha256)) {
      throw new Error(`Invalid manifest entry for ${skill.name}`);
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
    const source = await readFile(inside(root, skill.path));
    if (hash(source) !== skill.sha256) throw new Error(`Manifest hash is stale for ${skill.name}`);
    for (const [harness, targetRoot] of [['claude', '.claude/skills'], ['codex', '.agents/skills']]) {
      if (!skill.harnesses.includes(harness)) continue;
      const target = inside(root, `${targetRoot}/${skill.name}/SKILL.md`);
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
  const roster = await readFile(inside(root, manifest.roster.path));
  if (hash(roster) !== manifest.roster.sha256) throw new Error('Manifest hash is stale for roster');
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
