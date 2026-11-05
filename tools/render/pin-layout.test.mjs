import test from 'node:test';
import assert from 'node:assert/strict';
import { pinGeometry, scrollColumn } from './assertions.mjs';

async function withWall(run) {
  const previous = Object.fromEntries(['document', 'getComputedStyle', 'requestAnimationFrame'].map(key => [key, globalThis[key]]));
  let active;
  const column = (id, left, width, pinned = false, clip = 0) => {
    const node = { dataset: { agentId: id, pinned: pinned ? 'true' : undefined }, left, width, clip,
      classList: { contains: name => name === 'queen-column' ? id === 'queen' : name === 'attention-column' && id === 'attention' },
      getBoundingClientRect() { return { left: this.left, right: this.left + this.width, width: this.width }; },
      querySelector() { active = this.control; return this.control; },
    };
    node.control = { getBoundingClientRect: () => ({ x: node.left + 10, y: 20, width: 20, height: 20 }), contains: other => other === node.control };
    return node;
  };
  const rows = [column('queen', 291, 420, true), column('attention', 723, 280, true), column('worker-5', 532, 280, false, 280), column('worker-6', 824, 280), column('worker-7', 1116, 280)];
  const grid = { scrollLeft: 1000, clientLeft: 0, querySelectorAll: () => rows, getBoundingClientRect: () => ({ left: 283, right: 1400, width: 1117 }) };
  const page = { locator: () => ({ evaluate: callback => callback(grid) }) };
  for (const node of rows) node.closest = () => grid;
  const pane = { page: () => page, scrollIntoViewIfNeeded: async () => { grid.scrollLeft = 1000; }, evaluate: callback => callback(rows.at(-1)) };
  const fonts = { status: 'loaded' };
  globalThis.document = { fonts, elementFromPoint: () => active };
  globalThis.getComputedStyle = node => ({ position: node.dataset?.pinned ? 'sticky' : 'static', paddingLeft: '4px', getPropertyValue: key => key === '--pin-space' ? '728' : String(node.clip ?? 0) });
  try { await run({ page, pane, rows, fonts }); }
  finally { for (const [key, value] of Object.entries(previous)) { if (value === undefined) delete globalThis[key]; else globalThis[key] = value; } }
}

test('wall pin readiness waits for scroll clipping and stable geometry, without accepting an overlapping snapshot', () => withWall(async ({ page, pane, rows, fonts }) => {
  let frames = 0;
  fonts.status = 'loading';
  globalThis.requestAnimationFrame = callback => queueMicrotask(() => {
    frames++;
    if (frames === 2) fonts.status = 'loaded';
    // Geometry can be stable before the queued scroll handler has clipped it.
    if (frames === 5) rows[3].clip = 187;
    if (frames === 6) { rows[3].left = 823; rows[3].clip = 188; }
    callback();
  });
  await scrollColumn(pane);
  const result = await pinGeometry(page, 'worker-7');
  assert.equal(result.noOverlap, true);
  assert.equal(frames, 8);
  assert.deepEqual(result.overlapPairs, []);
}));

test('wall pin diagnostics identify the exact scrolling and pinned pair while preserving the overlap rule', () => withWall(async ({ page, rows }) => {
  const stale = await pinGeometry(page, 'worker-7');
  assert.equal(stale.noOverlap, false);
  assert.deepEqual(stale.overlapPairs.map(pair => [pair.first, pair.second, pair.kind]), [['worker-6', 'attention', 'scrolling']]);
  assert.deepEqual(stale.overlapPairs[0].firstRect, { left: 824, right: 1104 });
  assert.equal(stale.rows.length, 5);
  rows[3].clip = 187;
  assert.equal((await pinGeometry(page, 'worker-7')).noOverlap, true);
  // A persistent real overlap remains a failure; readiness never wraps mutants.
  rows[1].left = 600;
  assert.equal((await pinGeometry(page, 'worker-7')).noOverlap, false);
}));
