import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { readManifest, syncWorkflows } from './sync-workflows.mjs';

const digest = (text) => createHash('sha256').update(text).digest('hex');

test('sync check detects a stale generated copy without writing; sync repairs it', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'workflow-sync-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const skill = '# Canonical skill\n';
  const roster = '# Roster\n';
  const source = path.join(root, 'workflow-pack/skills/example/SKILL.md');
  const copy = path.join(root, '.claude/skills/example/SKILL.md');
  const claudeRoster = path.join(root, '.claude/agent-roster.md');
  const codexRoster = path.join(root, '.agents/agent-roster.md');
  await mkdir(path.dirname(source), { recursive: true });
  await mkdir(path.dirname(copy), { recursive: true });
  await writeFile(source, skill);
  await writeFile(copy, '# Stale\n');
  await writeFile(path.join(root, 'workflow-pack/agent-roster.md'), roster);
  await writeFile(path.join(root, 'workflow-pack/manifest.json'), JSON.stringify({
    schema_version: 1,
    skills: [{ name: 'example', tier: 'core', harnesses: ['claude', 'codex'], path: 'workflow-pack/skills/example/SKILL.md', sha256: digest(skill) }],
    roster: { path: 'workflow-pack/agent-roster.md', sha256: digest(roster) },
  }));
  const stale = await syncWorkflows({ root, check: true });
  assert.deepEqual(stale.sort(), [
    path.join('.agents', 'agent-roster.md'),
    path.join('.agents', 'skills/example/SKILL.md'),
    path.join('.claude', 'agent-roster.md'),
    path.join('.claude', 'skills/example/SKILL.md'),
  ]);
  assert.equal(await readFile(copy, 'utf8'), '# Stale\n');
  await assert.rejects(readFile(claudeRoster));
  await assert.rejects(readFile(codexRoster));
  assert.equal((await syncWorkflows({ root })).length, 4);
  assert.equal(await readFile(copy, 'utf8'), skill);
  assert.equal(await readFile(claudeRoster, 'utf8'), roster);
  assert.equal(await readFile(codexRoster, 'utf8'), roster);
  assert.deepEqual(await syncWorkflows({ root, check: true }), []);

  await writeFile(codexRoster, '# Stale roster\n');
  assert.deepEqual(await syncWorkflows({ root, check: true }), [path.join('.agents', 'agent-roster.md')]);
  assert.equal(await readFile(codexRoster, 'utf8'), '# Stale roster\n');
  assert.deepEqual(await syncWorkflows({ root }), [path.join('.agents', 'agent-roster.md')]);
  assert.equal(await readFile(codexRoster, 'utf8'), roster);
});

async function multiFixture(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'workflow-multi-sync-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const prefix = 'workflow-pack/skills/example/';
  const contents = { 'SKILL.md': '# Skill\n', 'scripts/check.py': 'print("check")\n', 'fixtures/demo.css': '.demo {}\n' };
  for (const [relative, content] of Object.entries(contents)) {
    const source = path.join(root, prefix, relative);
    await mkdir(path.dirname(source), { recursive: true });
    await writeFile(source, content);
  }
  const manifest = { schema_version: 2,
    skills: [{ name: 'example', harnesses: ['claude', 'codex'],
      files: Object.entries(contents).map(([relative, content]) => ({ path: prefix + relative, sha256: digest(content) })) }],
    roster: { path: 'workflow-pack/agent-roster.md', sha256: digest('# Roster\n') } };
  await writeFile(path.join(root, 'workflow-pack/agent-roster.md'), '# Roster\n');
  const manifestPath = path.join(root, 'workflow-pack/manifest.json');
  await writeFile(manifestPath, JSON.stringify(manifest));
  return { root, contents, manifest, manifestPath };
}

test('v2 sync copies every file to both harnesses and check is read-only', async (t) => {
  const { root, contents } = await multiFixture(t);
  assert.equal((await syncWorkflows({ root, check: true })).length, 8);
  await assert.rejects(readFile(path.join(root, '.claude/skills/example/SKILL.md')), { code: 'ENOENT' });
  assert.equal((await syncWorkflows({ root })).length, 8);
  for (const target of ['.claude', '.agents']) {
    for (const [relative, content] of Object.entries(contents)) {
      assert.equal(await readFile(path.join(root, target, 'skills/example', relative), 'utf8'), content);
    }
  }
  assert.deepEqual(await syncWorkflows({ root, check: true }), []);
  const nested = path.join(root, '.agents/skills/example/scripts/check.py');
  await writeFile(nested, 'edited\n');
  assert.deepEqual(await syncWorkflows({ root, check: true }), [path.relative(root, nested)]);
  assert.equal(await readFile(nested, 'utf8'), 'edited\n');
  await syncWorkflows({ root });
  assert.equal(await readFile(nested, 'utf8'), contents['scripts/check.py']);
});

test('v2 accepts unchanged legacy entries', async (t) => {
  const { root, manifest, manifestPath } = await multiFixture(t);
  const skill = manifest.skills[0];
  skill.path = skill.files[0].path;
  skill.sha256 = skill.files[0].sha256;
  delete skill.files;
  await writeFile(manifestPath, JSON.stringify(manifest));
  assert.deepEqual((await readManifest(root)).skills[0], skill);
  assert.equal((await syncWorkflows({ root })).length, 4);
});

test('multi-file manifest refuses traversal, foreign roots, aliases, and missing entrypoint', async (t) => {
  const { root, manifest, manifestPath } = await multiFixture(t);
  for (const unsafe of ['workflow-pack/skills/example/../other.py', '/tmp/SKILL.md',
    'workflow-pack/skills/other/check.py', 'workflow-pack/skills/example/scripts\\check.py',
    'workflow-pack/skills/example/./check.py', 'workflow-pack/skills/example/scripts/check.py:stream']) {
    const broken = structuredClone(manifest);
    broken.skills[0].files[1].path = unsafe;
    await writeFile(manifestPath, JSON.stringify(broken));
    await assert.rejects(readManifest(root), /Invalid manifest file/);
  }
  for (const change of [
    (value) => value.skills[0].files.push({ ...value.skills[0].files[0] }),
    (value) => value.skills[0].files.shift(),
    (value) => value.skills[0].files[0].sha256 = 'bad',
    (value) => value.schema_version = 1,
  ]) {
    const broken = structuredClone(manifest);
    change(broken);
    await writeFile(manifestPath, JSON.stringify(broken));
    await assert.rejects(readManifest(root), /Invalid|Missing/);
  }
});

test('v2 checks nested source hashes before copying that file', async (t) => {
  const { root } = await multiFixture(t);
  await writeFile(path.join(root, 'workflow-pack/skills/example/scripts/check.py'), 'tampered\n');
  await assert.rejects(syncWorkflows({ root }), /Manifest hash is stale/);
  await assert.rejects(readFile(path.join(root, '.claude/skills/example/scripts/check.py')), { code: 'ENOENT' });
});
