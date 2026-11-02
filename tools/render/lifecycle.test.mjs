import test from 'node:test';
import assert from 'node:assert/strict';
import { pendingRequests } from './gate.mjs';
import { sameProcess, inside, liveUnchanged } from './windows.mjs';
test('FIFO honors older lanes and supports existing release-time rows', () => {
  assert.deepEqual(pendingRequests('REQUEST a t1\nREQUEST b t2\nRELEASE a t3\n'), [{ lane: 'b', stamp: 't2' }]);
  assert.deepEqual(pendingRequests('REQUEST a t1\nREQUEST a t2\nRELEASE a t2\n'), [{ lane: 'a', stamp: 't1' }]);
});
test('PID reuse or an unverified executable never matches an owned process', () => {
  const owned = { pid: 42, exe: 'fixture.exe', started: '100' };
  assert.equal(sameProcess(owned, { ...owned }), true);
  for (const changed of [{ pid: 43 }, { exe: 'other.exe' }, { started: '101' }, { exe: null }]) assert.equal(sameProcess(owned, { ...owned, ...changed }), false);
  assert.equal(sameProcess(owned, null), false);
});
test('containment rejects a sibling whose name starts with the instance root', () => {
  const root = process.platform === 'win32' ? 'C:\\scratch\\instance' : '/scratch/instance';
  assert.equal(inside(root, `${root}-other`), false);
  assert.equal(inside(root, `${root}${process.platform === 'win32' ? '\\' : '/'}home`), true);
});
test('live comparison rejects restarts and health changes without depending on JSON key order', () => {
  const before = { running: true, pids: [42], health: 'ok', processes: [{ pid: 42, exe: 'live.exe', started: '1' }] };
  assert.equal(liveUnchanged(before, { processes: [{ started: '1', exe: 'live.exe', pid: 42 }], health: 'ok', pids: [42], running: true }), true);
  assert.equal(liveUnchanged(before, { ...before, health: 'changed' }), false);
  assert.equal(liveUnchanged(before, { ...before, processes: [{ ...before.processes[0], started: '2' }] }), false);
});
