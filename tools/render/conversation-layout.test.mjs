import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const source = await readFile(new URL('./assertions.mjs', import.meta.url), 'utf8');
const start = source.indexOf('    // Exercise the persisted result envelope through the shipped conversation UI.');
const end = source.indexOf("    if (await toggle.getAttribute('aria-expanded') === 'true') await toggle.click();", start);
assert.ok(start >= 0 && end > start);
// Execute the actual narrow sequence, including its awaits and predicate, rather
// than a duplicate helper. The surrounding screen runner needs a real WebView.
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const exercise = new AsyncFunction('page', 'manifest', 'viewport', 'toggle', 'probe', 'prove', 'capture', 'visibleTargetState', source.slice(start, end));

async function model(mode = 'normal') {
  const previous = Object.fromEntries(['document', 'getComputedStyle', 'requestAnimationFrame'].map(key => [key, globalThis[key]]));
  let frames = 0, busy = true, opacity = 0, animating = true;
  const box = (y = 80) => ({ x: 1100, y, width: 280, height: 80 });
  const messages = { tagName: 'DIV', className: 'messages', scrollTop: 0, scrollHeight: 800, clientHeight: 500, parentElement: null, getBoundingClientRect: () => ({ ...box(100), height: 500 }) };
  const widget = { tagName: 'DIV', className: 'diff-widget', parentElement: messages, scrollTop: 0, scrollHeight: 80, clientHeight: 80, getBoundingClientRect: () => box(mode === 'offscreen' ? 2000 : messages.scrollTop ? 120 : 180) };
  const content = { getAnimations: () => [{ playState: animating ? 'running' : 'finished' }] };
  const skeleton = { getAttribute: () => String(busy), querySelector: () => content };
  const root = { querySelector: selector => ({ '.diff-widget':widget, '.messages':messages, '.conversation-loading':skeleton })[selector] };
  const diff = { first() { return this; }, count: async () => 1, waitFor: async () => {}, scrollIntoViewIfNeeded: async () => {}, evaluate: callback => callback(widget) };
  const button = { getByRole() { return this; }, getAttribute: async () => 'true', click: async () => {} };
  const viewer = { getByRole: () => button, evaluate: callback => callback(root), locator: selector => {
    if (selector === '.diff-widget') return diff;
    return { allTextContents: async () => [selector.includes('.add ') ? 'Amber navigation' : 'Old navigation'] };
  } };
  const page = { getByRole: () => button, locator: () => viewer };
  globalThis.document = { fonts: { status: 'loaded' } };
  globalThis.getComputedStyle = node => ({ display: 'block', visibility: mode === 'hidden' ? 'hidden' : 'visible', opacity: String(node === content ? opacity : 1), overflowX: 'hidden', overflowY: 'auto' });
  globalThis.requestAnimationFrame = callback => queueMicrotask(() => {
    frames++;
    if (frames === 3) { busy = false; opacity = 0.5; }
    if (frames === 5) { opacity = 1; animating = false; }
    // The app's queued autoscroll can land after loading and crossfade complete.
    if (frames === 6) messages.scrollTop = 100;
    callback();
  });
  const visibility = async () => ({ visible: !busy && opacity === 1 && !animating && mode === 'normal', visibleText: 'Old navigation Amber navigation', rect: widget.getBoundingClientRect() });
  let mutation;
  try {
    await exercise(page, { conversation: { added:'Amber navigation', removed:'Old navigation' } }, { width:1440, height:900 }, button,
      async (name, measure) => { const result = await measure(); assert.equal(result.passed, true, name); assert.ok(result.details.painted); assert.equal(result.details.ancestors.length, 2); },
      async name => { mutation = name; }, async () => {}, visibility);
    assert.ok(frames >= 8, 'readiness must outlast delayed autoscroll and repeated geometry frames');
    assert.equal(mutation, 'conv-diff-1440');
  } finally {
    for (const [key, value] of Object.entries(previous)) { if (value === undefined) delete globalThis[key]; else globalThis[key] = value; }
  }
}

test('conversation-diff-envelope waits for reveal and queued autoscroll before its unchanged strict read', () => model());
test('conversation-diff-envelope still rejects persistently hidden content after readiness', () => assert.rejects(model('hidden'), /conversation-diff-envelope/));
test('conversation-diff-envelope still rejects offscreen content after readiness', () => assert.rejects(model('offscreen'), /conversation-diff-envelope/));
