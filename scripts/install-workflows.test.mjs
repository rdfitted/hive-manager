import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { installWorkflows } from './install-workflows.mjs';

const digest = (text) => createHash('sha256').update(text).digest('hex');

async function fixture(t) {
  const base = await mkdtemp(path.join(os.tmpdir(), 'workflow install space '));
  t.after(() => rm(base, { recursive: true, force: true }));
  const root = path.join(base, 'repo space');
  const home = path.join(base, 'home space');
  const appConfigDir = path.join(base, 'config space');
  const codexHome = path.join(base, 'custom codex space');
  const skillSource = 'workflow-pack/skills/example/SKILL.md';
  const rosterSource = 'workflow-pack/agent-roster.md';
  await mkdir(path.join(root, 'workflow-pack/skills/example'), { recursive: true });
  const skill = '# First version\n';
  const roster = '# Example roster\n';
  await writeFile(path.join(root, skillSource), skill);
  await writeFile(path.join(root, rosterSource), roster);
  async function writeManifest(content = skill) {
    const manifest = {
      schema_version: 1,
      skills: [{ name: 'example', tier: 'core', harnesses: ['claude', 'codex'], path: skillSource, sha256: digest(content) }],
      roster: { path: rosterSource, sha256: digest(roster) },
    };
    await writeFile(path.join(root, 'workflow-pack/manifest.json'), JSON.stringify(manifest));
  }
  await writeManifest();
  const options = { root, home, appConfigDir, codexHome, harness: 'both' };
  return { options, writeManifest, skillSource, skillTarget: path.join(home, '.claude/skills/example/SKILL.md'), codexTarget: path.join(codexHome, 'skills/example/SKILL.md') };
}

test('first install records ownership; repeat install is an identical no-op', async (t) => {
  const { options, skillTarget } = await fixture(t);
  const first = await installWorkflows(options);
  assert.equal(first.filter(({ kind }) => kind === 'install').length, 4);
  assert.equal(await readFile(skillTarget, 'utf8'), '# First version\n');
  const record = JSON.parse(await readFile(path.join(options.appConfigDir, 'workflow-pack-install.json'), 'utf8'));
  assert.equal(record.files[skillTarget].sha256, digest('# First version\n'));
  const second = await installWorkflows(options);
  assert.deepEqual([...new Set(second.map(({ kind }) => kind))], ['identical']);
});

test('user-edited owned collision is preserved and reported', async (t) => {
  const { options, writeManifest, skillTarget } = await fixture(t);
  await installWorkflows(options);
  await writeFile(skillTarget, '# User edit\n');
  await writeFile(path.join(options.root, 'workflow-pack/skills/example/SKILL.md'), '# New upstream\n');
  await writeManifest('# New upstream\n');
  const actions = await installWorkflows(options);
  assert.ok(actions.some(({ kind, target }) => kind === 'collision' && target === skillTarget));
  assert.equal(await readFile(skillTarget, 'utf8'), '# User edit\n');
});

test('owned file updates only when its current hash matches the install record', async (t) => {
  const { options, writeManifest, skillTarget } = await fixture(t);
  await installWorkflows(options);
  const updated = '# New upstream\n';
  await writeFile(path.join(options.root, 'workflow-pack/skills/example/SKILL.md'), updated);
  await writeManifest(updated);
  const actions = await installWorkflows(options);
  assert.ok(actions.some(({ kind, target }) => kind === 'update' && target === skillTarget));
  assert.equal(await readFile(skillTarget, 'utf8'), updated);
});

test('dry run writes no destination or ownership manifest', async (t) => {
  const { options } = await fixture(t);
  const actions = await installWorkflows({ ...options, dryRun: true });
  assert.equal(actions.filter(({ kind }) => kind === 'install').length, 4);
  await assert.rejects(readdir(options.home), { code: 'ENOENT' });
  await assert.rejects(readdir(options.codexHome), { code: 'ENOENT' });
  await assert.rejects(readdir(options.appConfigDir), { code: 'ENOENT' });
});

test('paths with spaces and custom CODEX_HOME are respected', async (t) => {
  const { options, codexTarget } = await fixture(t);
  await installWorkflows({ ...options, harness: 'codex' });
  assert.equal(await readFile(codexTarget, 'utf8'), '# First version\n');
  await assert.rejects(readdir(path.join(options.home, '.codex')), { code: 'ENOENT' });
});

test('clean uninstall removes only recorded files and retains directories', async (t) => {
  const { options, skillTarget, codexTarget } = await fixture(t);
  await installWorkflows(options);
  const actions = await installWorkflows({ ...options, uninstall: true });
  assert.equal(actions.filter(({ kind }) => kind === 'remove').length, 4);
  await assert.rejects(readFile(skillTarget), { code: 'ENOENT' });
  await assert.rejects(readFile(codexTarget), { code: 'ENOENT' });
  assert.ok((await readdir(path.dirname(skillTarget))).length === 0);
  const record = JSON.parse(await readFile(path.join(options.appConfigDir, 'workflow-pack-install.json'), 'utf8'));
  assert.deepEqual(record.files, {});
});

test('uninstall preserves and reports edited and unowned files', async (t) => {
  const { options, skillTarget, codexTarget } = await fixture(t);
  await installWorkflows({ ...options, harness: 'claude' });
  await writeFile(skillTarget, '# User edit\n');
  await mkdir(path.dirname(codexTarget), { recursive: true });
  await writeFile(codexTarget, '# Unowned copy\n');
  const actions = await installWorkflows({ ...options, uninstall: true });
  assert.ok(actions.some(({ kind, target }) => kind === 'collision' && target === skillTarget));
  assert.ok(actions.some(({ kind, target }) => kind === 'unowned' && target === codexTarget));
  assert.equal(await readFile(skillTarget, 'utf8'), '# User edit\n');
  assert.equal(await readFile(codexTarget, 'utf8'), '# Unowned copy\n');
});

test('dry-run uninstall writes nothing', async (t) => {
  const { options, skillTarget } = await fixture(t);
  await installWorkflows(options);
  const recordPath = path.join(options.appConfigDir, 'workflow-pack-install.json');
  const beforeRecord = await readFile(recordPath);
  const beforeSkill = await readFile(skillTarget);
  const actions = await installWorkflows({ ...options, uninstall: true, dryRun: true });
  assert.equal(actions.filter(({ kind }) => kind === 'remove').length, 4);
  assert.deepEqual(await readFile(recordPath), beforeRecord);
  assert.deepEqual(await readFile(skillTarget), beforeSkill);
});
