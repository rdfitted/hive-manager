import test from 'node:test';
import assert from 'node:assert/strict';
import { scrollColumn, stageGeometry } from './assertions.mjs';

function rect(left, top, width, height) {
  return { x: left, y: top, left, top, width, height, right: left + width, bottom: top + height };
}

async function withStage(run) {
  const { document: previousDocument, getComputedStyle: previousStyle } = globalThis;
  const viewport = {
    scrollTop: 0, clientHeight: 340, scrollHeight: 940, clientWidth: 320, scrollWidth: 320,
    style: { overflowX: 'hidden', overflowY: 'auto' },
    getBoundingClientRect: () => rect(560, 100, 320, 340),
    contains: node => node.parent === viewport,
  };
  const queen = { dataset: { agentId: 'queen' }, getBoundingClientRect: () => rect(110, 100, 440, 340) };
  const far = {
    dataset: { agentId: 'far' }, parent: viewport,
    getBoundingClientRect: () => rect(560, 810 - viewport.scrollTop, 320, 220),
    closest: selector => selector === '.worker-viewport' ? viewport : null,
  };
  const rows = [queen, far];
  for (const row of rows) {
    const header = {
      getBoundingClientRect() {
        const { left, top } = row.getBoundingClientRect();
        return rect(left + 8, top + 10, 60, 20);
      },
      contains: node => node === header,
    };
    row.querySelector = () => header;
  }
  const stage = {
    clientWidth: 800, scrollWidth: 800, style: { overflowX: 'clip' },
    getBoundingClientRect: () => rect(100, 90, 800, 360),
    querySelector: selector => selector === '.queen-column' ? queen : viewport,
    querySelectorAll: () => rows,
  };
  const page = { locator: () => ({ evaluate: (callback, arg) => callback(stage, arg) }) };
  const pane = { evaluate: callback => callback(far) };
  globalThis.document = {
    elementFromPoint(x, y) {
      for (const row of rows) {
        const header = row.querySelector();
        const box = header.getBoundingClientRect();
        if (x >= box.left && x <= box.right && y >= box.top && y <= box.bottom && y >= 100 && y <= 440) return header;
      }
      return null;
    },
  };
  globalThis.getComputedStyle = node => node.style;
  try { await run({ page, pane, queen, stage, viewport }); }
  finally {
    if (previousDocument === undefined) delete globalThis.document;
    else globalThis.document = previousDocument;
    if (previousStyle === undefined) delete globalThis.getComputedStyle;
    else globalThis.getComputedStyle = previousStyle;
  }
}

test('worker reveal scrolls the viewport vertically and leaves the Queen anchored', () => withStage(async ({ page, pane, viewport }) => {
  assert.equal((await stageGeometry(page, 'far')).hits, false);
  await scrollColumn(pane);
  const top = viewport.scrollTop;
  assert.equal(top, 400);
  const details = await stageGeometry(page, 'far');
  assert.equal(details.scrollDelta, 400);
  assert.equal(details.passed, true);
  assert.equal(viewport.scrollTop, top);
}));

test('stage geometry rejects a scrolling Queen and horizontal scrolling', () => withStage(async ({ page, pane, queen, stage, viewport }) => {
  await scrollColumn(pane);
  queen.parent = viewport;
  queen.getBoundingClientRect = () => rect(110, 500 - viewport.scrollTop, 440, 340);
  const released = await stageGeometry(page, 'far');
  assert.equal(released.queenOutside, false);
  assert.equal(released.anchored, false);
  assert.equal(released.passed, false);

  queen.parent = stage;
  queen.getBoundingClientRect = () => rect(110, 100, 440, 340);
  stage.style.overflowX = 'auto';
  const scrollable = await stageGeometry(page, 'far');
  assert.equal(scrollable.scrollPolicy, false);
  assert.equal(scrollable.passed, false);

  stage.style.overflowX = 'clip';
  viewport.scrollWidth += 2;
  const overflow = await stageGeometry(page, 'far');
  assert.equal(overflow.horizontalOverflow, true);
  assert.equal(overflow.passed, false);
}));
