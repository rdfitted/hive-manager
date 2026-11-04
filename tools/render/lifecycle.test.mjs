import test from 'node:test';
import assert from 'node:assert/strict';
import { pendingRequests } from './gate.mjs';
import { sameProcess, inside, liveUnchanged } from './windows.mjs';
import { JSDOM } from 'jsdom';
import { runInContext } from 'node:vm';
import { pressOutsideTerminal } from './keyboard.mjs';
import { htmlProbeSource, htmlProbeMarker } from './html-probe.mjs';

test('packaged HTML probe separates author-only execution from the allowed Rust read', async () => {
  for (const mode of ['author', 'ipc', 'rejected']) {
    const dom = new JSDOM('<body><script src="http://tauri.localhost/html-ipc-probe.js"></script></body>', { runScripts: 'outside-only' });
    try {
      const script = dom.window.document.querySelector('script');
      Object.defineProperty(dom.window.document, 'currentScript', { value: script });
      const session = 'fixture / orbital';
      if (mode !== 'author') script.dataset.ipcSession = encodeURIComponent(session);
      const calls = [];
      dom.window.__TAURI_INTERNALS__ = { invoke: (command, args) => {
        calls.push({ command, id: args.id });
        return mode === 'rejected' ? Promise.reject('opaque origin') : Promise.resolve({ id: session, created_at: 'synthetic-time' });
      } };
      runInContext(htmlProbeSource, dom.getInternalVMContext());
      await new Promise(resolve => setImmediate(resolve));
      assert.equal(script.dataset.htmlProbe, htmlProbeMarker);
      assert.equal(dom.window.document.body.dataset.htmlAuthor, 'ran');
      assert.deepEqual(calls, mode === 'author' ? [] : [{ command: 'get_session', id: session }]);
      if (mode === 'ipc') {
        assert.deepEqual(JSON.parse(dom.window.document.body.dataset.htmlRust), { id: session, createdAt: 'synthetic-time' });
        assert.equal(dom.window.__htmlIpcResult, 'resolved');
      } else {
        assert.equal(dom.window.document.body.dataset.htmlRust, undefined);
        assert.equal(dom.window.__htmlIpcResult, mode === 'author' ? undefined : 'rejected: opaque origin');
      }
    } finally { dom.window.close(); }
  }
});

test('keyboard guard fails by name and blocks terminal input when focus changes before or during dispatch', async () => {
  for (const phase of ['safe', 'before', 'dispatch']) {
    const dom = new JSDOM('<button>Target</button><div class="xterm"><textarea class="xterm-helper-textarea"></textarea></div>', { runScripts: 'outside-only' });
    const context = dom.getInternalVMContext(), target = dom.window.document.querySelector('button'), terminal = dom.window.document.querySelector('textarea');
    let writes = 0, presses = 0, disposed = false;
    terminal.addEventListener('keydown', () => writes++);
    const evaluate = (fn, value, arg) => {
      dom.window.guardValue = value; dom.window.guardArg = arg;
      return runInContext(`(${fn.toString()})(guardValue, guardArg)`, context);
    };
    const locator = {
      focus: async () => (phase === 'before' ? terminal : target).focus(),
      evaluateHandle: async fn => {
        const value = evaluate(fn, target);
        return { evaluate: async (callback, arg) => evaluate(callback, value, arg), dispose: async () => { disposed = true; } };
      },
    };
    const page = { keyboard: { press: async () => {
      presses++;
      if (phase === 'dispatch') terminal.focus();
      dom.window.document.activeElement.dispatchEvent(new dom.window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
    } } };
    const rows = [];
    try {
      const action = () => pressOutsideTerminal(page, locator, 'Enter', (name, passed) => rows.push({ name, passed }));
      if (phase === 'safe') await action();
      else await assert.rejects(action, new RegExp(phase === 'before' ? 'keyboard-focus-outside-terminal' : 'keyboard-target-outside-terminal'));
      assert.equal(writes, 0, phase);
      assert.equal(presses, phase === 'before' ? 0 : 1);
      assert.equal(rows.every(row => row.passed), phase === 'safe');
      assert.equal(disposed, true);
      // A failed driver key must not leave a listener suppressing later app input.
      terminal.dispatchEvent(new dom.window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
      assert.equal(writes, 1);
    } finally { dom.window.close(); }
  }
});
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


test('tracked-byte guard covers the render tree and catches a planted in-place adapter write', async () => {
  const { mkdtemp, mkdir, writeFile, rm } = await import('node:fs/promises');
  const { execFileSync } = await import('node:child_process');
  const { tmpdir } = await import('node:os');
  const path = await import('node:path');
  const { trackedSnapshot, trackedChanges } = await import('./integrity.mjs');
  const root = await mkdtemp(path.join(tmpdir(), 'render-integrity-'));
  const git = args => execFileSync('git', args, { cwd: root, windowsHide: true });
  try {
    await mkdir(path.join(root, 'tools/render/fixtures'), { recursive: true });
    const files = ['tools/render/updater.mjs', 'tools/render/fixtures/input.json', 'outside.txt'];
    for (const name of files) await writeFile(path.join(root, name), 'original\r\n');
    git(['init', '--quiet']);
    git(['add', '--', ...files]);
    const before = await trackedSnapshot(root);
    assert.deepEqual([...before.keys()].sort(), files.sort());
    assert.deepEqual(await trackedChanges(root, before), []);
    for (const name of files) {
      await writeFile(path.join(root, name), 'planted write\n');
      assert.deepEqual(await trackedChanges(root, before), [name]);
      await writeFile(path.join(root, name), 'original\r\n');
      assert.deepEqual(await trackedChanges(root, before), []);
    }
  } finally { await rm(root, { recursive: true, force: true }); }
});
