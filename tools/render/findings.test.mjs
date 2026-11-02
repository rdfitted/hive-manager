import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyFinding } from './findings.mjs';
import { runInNewContext } from 'node:vm';
import { updaterState } from './updater.mjs';
test('a known overflow still records failure and its issue, rather than turning green', () => {
  for (const name of ['grid-composer-with-attention', 'grid-composer-without-attention']) {
    const result = classifyFinding(name, false, { contained: false, card: name === 'grid-composer-with-attention', composer: { x: 1, y: 2, width: 50, height: 50, right: 51, bottom: 52 }, pane: { x: 0, y: 0, width: 60, height: 40, right: 60, bottom: 40 } }, { [name]: { issue: 414 } });
    assert.equal(result.passed, false);
    assert.equal(result.accepted, true);
    assert.equal(result.issue, 414);
  }
});
test('known findings fail closed if their issue disappears, they start passing, or the failure changes', () => {
  const name = 'grid-composer-with-attention', known = { [name]: { issue: 414 } };
  assert.equal(classifyFinding(name, true, {}, known).accepted, false);
  assert.equal(classifyFinding(name, true, {}, known).status, 'unexpected-pass');
  assert.equal(classifyFinding(name, false, {}, { [name]: {} }).accepted, false);
  assert.equal(classifyFinding(name, false, { error: 'composer missing' }, known).accepted, false);
  assert.equal(classifyFinding(name, false, { contained: false, card: false, composer: { x: 1, y: 2, width: 50, height: 50, right: 51, bottom: 52 }, pane: { x: 0, y: 0, width: 60, height: 40, right: 60, bottom: 40 } }, known).accepted, false);
});

test('only bounded bottom overflow with pane overlap matches #414', () => {
  const name = 'grid-composer-without-attention', known = { [name]: { issue: 414 } };
  const pane = { x: 100, y: 100, width: 200, height: 200, right: 300, bottom: 300 };
  const composer = { x: 108, y: 270, width: 184, height: 134, right: 292, bottom: 404 };
  const accepts = box => classifyFinding(name, false, { contained: false, card: false, pane, composer: box }, known).accepted;
  assert.equal(accepts(composer), true);
  for (const box of [
    { ...composer, x: -10000, right: -9816 },
    { ...composer, y: 80, bottom: 214 },
    { ...composer, y: 301, bottom: 435 },
    { ...composer, height: 151, bottom: 421 },
    { ...composer, x: NaN },
    { ...composer, right: Infinity },
    { ...composer, right: 400 },
  ]) assert.equal(accepts(box), false, JSON.stringify(box));
});

test('updater accepts only the omitted-plugin rejection', async () => {
  for (const [error, expected] of [['plugin updater not found', true], ['network failed after updater HTTP request', false], ['permission denied', false]]) {
    const result = await updaterState({ evaluate: fn => runInNewContext(`(${fn.toString()})()`, { window: { __TAURI_INTERNALS__: { invoke: async () => { throw error; } } } }) });
    assert.equal(result.rejected, expected, error);
    assert.equal(result.error, error);
  }
  const result = await updaterState({ evaluate: fn => runInNewContext(`(${fn.toString()})()`, { window: { __TAURI_INTERNALS__: { invoke: async () => null } } }) });
  assert.equal(result.rejected, false);
});
