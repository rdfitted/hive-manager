import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { scan, normalize, findDeniedNgram } from './content-gate.mjs';

const digest = (value) => createHash('sha256').update(value).digest('hex');

async function fixture(t, text) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'content-gate-'));
  t.after(async () => { await import('node:fs/promises').then(({ rm }) => rm(root, { recursive: true, force: true })); });
  const pack = path.join(root, 'workflow-pack');
  await mkdir(pack);
  const denylist = path.join(pack, 'content-denylist.sha256');
  await writeFile(denylist, `# synthetic fixture\n${digest('sample restricted')}\n`);
  const target = path.join(pack, 'example.md');
  await writeFile(target, text);
  return { root, denylist, target };
}

test('normalization collapses punctuation and finds a one-based n-gram position', () => {
  assert.equal(normalize('  Sample---Restricted!! '), 'sample restricted');
  assert.deepEqual(findDeniedNgram('ordinary sample--restricted text', new Set([digest('sample restricted')])), { start: 2, length: 2 });
});

test('hashes one through four words, never a five-word phrase', () => {
  const line = 'alpha beta gamma delta epsilon';
  for (let length = 1; length <= 4; length += 1) {
    assert.deepEqual(findDeniedNgram(line, new Set([digest(line.split(' ').slice(0, length).join(' '))])), { start: 1, length });
  }
  assert.equal(findDeniedNgram(line, new Set([digest(line)])), null);
});

test('synthetic hashed denylist fails without printing matched words', async (t) => {
  const options = await fixture(t, 'ordinary Sample--Restricted text\n');
  const findings = await scan({ ...options, targets: [options.target] });
  assert.equal(findings.length, 1);
  assert.match(findings[0], /example\.md:1: denied n-gram at word 2, length 2/);
  assert.doesNotMatch(findings[0], /sample|restricted/i);
});

test('external fixture diagnostics hide parent directories', async (t) => {
  const options = await fixture(t, 'sample restricted\n');
  const findings = await scan({ root: path.join(options.root, 'different-root'), denylist: options.denylist, targets: [options.target] });
  assert.match(findings[0], /^example\.md:1: denied n-gram/);
  assert.doesNotMatch(findings[0], /content-gate-/);
});

test('plaintext pattern fails and hash file is excluded', async (t) => {
  const options = await fixture(t, 'C:\\Users\\Example\n');
  const findings = await scan({ ...options, targets: [path.join(options.root, 'workflow-pack')] });
  assert.equal(findings.length, 1);
  assert.match(findings[0], /restricted plaintext pattern/);
  assert.doesNotMatch(findings[0], /Example/);
});

test('PowerShell script reference needs a POSIX alternative', async (t) => {
  const options = await fixture(t, 'Run helper.ps1\n');
  assert.equal((await scan({ ...options, targets: [options.target] })).length, 1);
  await writeFile(options.target, 'Run helper.ps1 or helper.sh\n');
  assert.deepEqual(await scan({ ...options, targets: [options.target] }), []);
});

test('clean content passes', async (t) => {
  const options = await fixture(t, 'Generic instructions for a local project.\n');
  assert.deepEqual(await scan({ ...options, targets: [options.target] }), []);
});
