import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyFinding } from './findings.mjs';
test('a known overflow still records failure and its issue, rather than turning green', () => {
  for (const name of ['grid-composer-with-attention', 'grid-composer-without-attention']) {
    const result = classifyFinding(name, false, { contained: false, card: name === 'grid-composer-with-attention', composer: { x: 1, y: 2, width: 50, height: 50, right: 51, bottom: 52 }, pane: { x: 0, y: 0, right: 60, bottom: 40 } }, { [name]: { issue: 414 } });
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
  assert.equal(classifyFinding(name, false, { contained: false, card: false, composer: { x: 1, y: 2, width: 50, height: 50, right: 51, bottom: 52 }, pane: { x: 0, y: 0, right: 60, bottom: 40 } }, known).accepted, false);
});
