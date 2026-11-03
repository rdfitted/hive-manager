import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, unlink, rm, stat } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { acquireGate, pendingRequests } from './gate.mjs';
import { ps, quote } from './windows.mjs';

test('FIFO grants also exclude other sessions and non-session runs from the shared pool', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'render-gates-'));
  const original = process.env.HIVE_SESSION_ID;
  const pool = path.join(root, 'pool'), lock = path.join(pool, 'render-check.lock');
  const signal = new AbortController().signal;
  const queues = ['session-a', 'session-b'].map(session => path.join(root, '.hive-manager', session, 'gate-queue.log'));
  const pending = async index => pendingRequests(await readFile(queues[index], 'utf8'));
  try {
    execFileSync('git', ['init', '--quiet', root], { windowsHide: true });
    for (const queue of queues) {
      await mkdir(path.dirname(queue), { recursive: true });
      await writeFile(queue, '');
    }
    process.env.HIVE_SESSION_ID = 'session-a';
    const a = await acquireGate(root, pool, signal);
    try {
      assert.equal(a.kind, 'fifo');
      assert.equal(a.path, queues[0]);
      assert.deepEqual(await pending(0), [{ lane: a.lane, stamp: a.stamp }]);
      assert.equal((await stat(lock)).isFile(), true, 'FIFO grant must own the shared lock');
      process.env.HIVE_SESSION_ID = 'session-b';
      await assert.rejects(acquireGate(root, pool, signal), /Another render check owns the local gate/);
      assert.deepEqual(await pending(1), [], 'failed second acquisition must release FIFO');
      delete process.env.HIVE_SESSION_ID;
      await assert.rejects(acquireGate(root, pool, signal), /Another render check owns the local gate/);
    } finally { await a.release(); }
    assert.deepEqual(await pending(0), []);
    process.env.HIVE_SESSION_ID = 'session-b';
    const b = await acquireGate(root, pool, signal);
    await b.release();
    delete process.env.HIVE_SESSION_ID;
    const local = await acquireGate(root, pool, signal);
    assert.equal(local.kind, 'local');
    await local.release();

    // A filesystem failure in the second acquisition must not strand the queue.
    process.env.HIVE_SESSION_ID = 'session-a';
    const blockedPool = path.join(root, 'not-a-directory');
    await writeFile(blockedPool, 'fixture');
    await assert.rejects(acquireGate(root, blockedPool, signal), /EEXIST|ENOTDIR/);
    assert.deepEqual(await pending(0), []);

    const failingRelease = await acquireGate(root, pool, signal);
    await unlink(lock);
    await assert.rejects(failingRelease.release(), /ENOENT/);
    assert.deepEqual(await pending(0), [], 'local release failure must still release FIFO');
    const recovered = await acquireGate(root, pool, signal);
    await recovered.release();
  } finally {
    if (original === undefined) delete process.env.HIVE_SESSION_ID;
    else process.env.HIVE_SESSION_ID = original;
    await rm(root, { recursive: true, force: true });
  }
});

const persistentPolicy = () => JSON.parse(ps('Get-ExecutionPolicy -List | Where-Object {$_.Scope -ne "Process"} | Select-Object Scope,ExecutionPolicy | ConvertTo-Json -Compress'));
for (const form of ['command', 'file']) {
  test(`PowerShell ${form} launch executes its fixture under inherited Restricted policy`, async () => {
    const root = await mkdtemp(path.join(tmpdir(), 'render-policy-'));
    const before = persistentPolicy();
    try {
      const fixture = path.join(root, 'fixture.ps1');
      await writeFile(fixture, 'param([string]$Marker)\n@{marker=$Marker;policy=(Get-ExecutionPolicy -Scope Process).ToString()} | ConvertTo-Json -Compress\n');
      const runner = path.join(root, 'runner.mjs');
      await writeFile(runner, `
import assert from 'node:assert/strict';
import { ps, quote } from ${JSON.stringify(new URL('./windows.mjs', import.meta.url).href)};
import { spawnBuildScript } from ${JSON.stringify(new URL('./build.mjs', import.meta.url).href)};
assert.equal(process.env.PSExecutionPolicyPreference, 'Restricted');
let output;
if (${JSON.stringify(form)} === 'command') {
  output = ps('& ' + quote(${JSON.stringify(fixture)}) + ' -Marker command');
} else {
  const child = spawnBuildScript(${JSON.stringify(fixture)}, ['-Marker', 'file'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = ''; output = '';
  child.stdout.on('data', data => { output += data; });
  child.stderr.on('data', data => { stderr += data; });
  const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
  assert.equal(code, 0, stderr);
}
assert.deepEqual(JSON.parse(output), { marker: ${JSON.stringify(form)}, policy: 'Bypass' });
console.log('fixture-${form}-passed');
`);
      const output = execFileSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Restricted', '-Command',
        `if((Get-ExecutionPolicy -Scope Process) -ne 'Restricted'){throw 'Restricted parent required'}; & ${quote(process.execPath)} ${quote(runner)}; exit $LASTEXITCODE`], { encoding: 'utf8', windowsHide: true });
      assert.equal(output.trim(), `fixture-${form}-passed`);
    } finally {
      assert.deepEqual(persistentPolicy(), before, 'persistent execution policy must be unchanged');
      await rm(root, { recursive: true, force: true });
    }
  });
}
