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

async function multiFixture(t) {
  const result = await fixture(t);
  const { root } = result.options;
  const manifestPath = path.join(root, 'workflow-pack/manifest.json');
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  manifest.schema_version = 2;
  const skill = manifest.skills[0];
  skill.files = [{ path: skill.path, sha256: skill.sha256 },
    { path: 'workflow-pack/skills/example/scripts/check.py', sha256: digest('print("check")\n') }];
  delete skill.path;
  delete skill.sha256;
  await mkdir(path.join(root, 'workflow-pack/skills/example/scripts'), { recursive: true });
  await writeFile(path.join(root, skill.files[1].path), 'print("check")\n');
  await writeFile(manifestPath, JSON.stringify(manifest));
  return { ...result, manifest, manifestPath,
    scriptTarget: path.join(path.dirname(result.skillTarget), 'scripts/check.py') };
}

test('multi-file skill installs and updates every declared file', async (t) => {
  const { options, manifest, manifestPath, skillTarget, scriptTarget } = await multiFixture(t);
  assert.equal((await installWorkflows(options)).filter(({ kind }) => kind === 'install').length, 6);
  for (const [index, content] of ['# Next version\n', 'print("next")\n'].entries()) {
    const file = manifest.skills[0].files[index];
    await writeFile(path.join(options.root, file.path), content);
    file.sha256 = digest(content);
  }
  await writeFile(manifestPath, JSON.stringify(manifest));
  assert.equal((await installWorkflows(options)).filter(({ kind }) => kind === 'update').length, 4);
  assert.equal(await readFile(skillTarget, 'utf8'), '# Next version\n');
  assert.equal(await readFile(scriptTarget, 'utf8'), 'print("next")\n');
});

test('unowned SKILL.md collision skips the whole multi-file skill', async (t) => {
  const { options, skillTarget, scriptTarget } = await multiFixture(t);
  await mkdir(path.dirname(skillTarget), { recursive: true });
  await writeFile(skillTarget, '# Local skill\n');
  const actions = await installWorkflows({ ...options, harness: 'claude' });
  assert.ok(actions.some(({ kind, target }) => kind === 'collision' && target === skillTarget));
  assert.equal(await readFile(skillTarget, 'utf8'), '# Local skill\n');
  await assert.rejects(readFile(scriptTarget), { code: 'ENOENT' });
  assert.ok(!actions.some(({ target }) => target === scriptTarget));
  const record = JSON.parse(await readFile(path.join(options.appConfigDir, 'workflow-pack-install.json')));
  assert.ok(!Object.keys(record.files).some((file) => file.startsWith(path.dirname(skillTarget) + path.sep)));
});

test('a later nested collision prevents an earlier SKILL.md write', async (t) => {
  const { options, skillTarget, scriptTarget } = await multiFixture(t);
  await mkdir(path.dirname(scriptTarget), { recursive: true });
  await writeFile(scriptTarget, 'local script\n');
  await installWorkflows({ ...options, harness: 'claude' });
  await assert.rejects(readFile(skillTarget), { code: 'ENOENT' });
  assert.equal(await readFile(scriptTarget, 'utf8'), 'local script\n');
});

test('edited owned nested file prevents sibling updates and ownership changes', async (t) => {
  const { options, manifest, manifestPath, skillTarget, scriptTarget } = await multiFixture(t);
  await installWorkflows({ ...options, harness: 'claude' });
  await writeFile(scriptTarget, 'user edit\n');
  const file = manifest.skills[0].files[0];
  await writeFile(path.join(options.root, file.path), '# Updated\n');
  file.sha256 = digest('# Updated\n');
  await writeFile(manifestPath, JSON.stringify(manifest));
  const recordPath = path.join(options.appConfigDir, 'workflow-pack-install.json');
  const before = await readFile(recordPath);
  const actions = await installWorkflows({ ...options, harness: 'claude' });
  assert.ok(actions.some(({ kind, target }) => kind === 'collision' && target === scriptTarget));
  assert.equal(await readFile(skillTarget, 'utf8'), '# First version\n');
  assert.equal(await readFile(scriptTarget, 'utf8'), 'user edit\n');
  assert.deepEqual(await readFile(recordPath), before);
});

test('an identical unowned v1 file remains unowned after install', async (t) => {
  const { options, skillTarget } = await fixture(t);
  await mkdir(path.dirname(skillTarget), { recursive: true });
  await writeFile(skillTarget, '# First version\n');
  const actions = await installWorkflows({ ...options, harness: 'claude' });
  assert.ok(actions.some(({ kind, target }) => kind === 'identical' && target === skillTarget));
  const record = JSON.parse(await readFile(path.join(options.appConfigDir, 'workflow-pack-install.json')));
  assert.equal(record.files[skillTarget], undefined);
  await installWorkflows({ ...options, harness: 'claude', uninstall: true });
  assert.equal(await readFile(skillTarget, 'utf8'), '# First version\n');
});

test('multi-file dry run changes neither files nor ownership', async (t) => {
  const { options } = await multiFixture(t);
  assert.equal((await installWorkflows({ ...options, dryRun: true })).filter(({ kind }) => kind === 'install').length, 6);
  await assert.rejects(readdir(options.home), { code: 'ENOENT' });
  await assert.rejects(readdir(options.codexHome), { code: 'ENOENT' });
  await assert.rejects(readdir(options.appConfigDir), { code: 'ENOENT' });
});

test('multi-file uninstall removes recorded obsolete files and preserves edited/unowned siblings', async (t) => {
  const { options, manifest, manifestPath, skillTarget, scriptTarget } = await multiFixture(t);
  await installWorkflows({ ...options, harness: 'claude' });
  const sibling = path.join(path.dirname(skillTarget), 'local.txt');
  await writeFile(sibling, 'local\n');
  await writeFile(skillTarget, '# User edit\n');
  manifest.skills = [];
  await writeFile(manifestPath, JSON.stringify(manifest));
  const recordPath = path.join(options.appConfigDir, 'workflow-pack-install.json');
  const before = await readFile(recordPath);
  await installWorkflows({ ...options, harness: 'claude', uninstall: true, dryRun: true });
  assert.deepEqual(await readFile(recordPath), before);
  assert.equal(await readFile(scriptTarget, 'utf8'), 'print("check")\n');
  await installWorkflows({ ...options, harness: 'claude', uninstall: true });
  await assert.rejects(readFile(scriptTarget), { code: 'ENOENT' });
  assert.equal(await readFile(skillTarget, 'utf8'), '# User edit\n');
  assert.equal(await readFile(sibling, 'utf8'), 'local\n');
});
