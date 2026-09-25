import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { syncWorkflows } from './sync-workflows.mjs';

const digest = (text) => createHash('sha256').update(text).digest('hex');

test('sync check detects a stale generated copy without writing; sync repairs it', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'workflow-sync-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const skill = '# Canonical skill\n';
  const roster = '# Roster\n';
  const source = path.join(root, 'workflow-pack/skills/example/SKILL.md');
  const copy = path.join(root, '.claude/skills/example/SKILL.md');
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
  assert.equal(stale.length, 2);
  assert.equal(await readFile(copy, 'utf8'), '# Stale\n');
  assert.equal((await syncWorkflows({ root })).length, 2);
  assert.equal(await readFile(copy, 'utf8'), skill);
  assert.deepEqual(await syncWorkflows({ root, check: true }), []);
});
