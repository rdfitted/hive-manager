import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyFinding } from './findings.mjs';
import { runInNewContext } from 'node:vm';
import { updaterState } from './updater.mjs';
import { scrubDiagnostic, failureDiagnostic } from './diagnostics.mjs';
import { mutationProver, reviewProofNames, consoleProofMessage } from './mutations.mjs';

const diagnosticOptions = {
  instanceRoot: String.raw`Z:\scratch\instance`, home: '/synthetic-home',
  env: { HOME: '/synthetic-home', USERPROFILE: String.raw`Z:\Users\sample-person`, USERNAME: 'sample-person', HIVE_AGENT_TOKEN: 'synthetic-environment-secret' },
};

test('console proof marker remains readable while adjacent credentials stay scrubbed', () => {
  const message = consoleProofMessage('2026-11-03T02-00-00.121Z-5690');
  const opaque = 'aB12'.repeat(12), credential = 'sk-' + 'c'.repeat(32);
  const details = [message, opaque, credential];
  for (const output of [
    scrubDiagnostic(details, diagnosticOptions),
    failureDiagnostic({ name: 'mutation-proof', status: 'fail', details: { name: 'no-console-errors', before: { passed: true }, mutant: { passed: false, details }, restored: { passed: true } } }, diagnosticOptions),
  ]) {
    assert.ok(output.includes(message), output);
    assert.equal(output.includes(opaque), false);
    assert.equal(output.includes(credential), false);
    assert.match(output, /\[opaque-token\]/);
    assert.match(output, /\[token\]/);
  }
});

test('CI diagnostics scrub instance, home, user paths and credential shapes before bounding', () => {
  const root = diagnosticOptions.instanceRoot;
  const paths = [root, root.replaceAll('\\', '/'), root.replaceAll('\\', '\\\\'), '/synthetic-home/private', String.raw`Z:\Users\sample-person\private`, '/home/other-person/private', '/Users/another-person/private', String.raw`C:\Users\runner-person\private`, String.raw`C:\\Users\\escaped-person\\private`];
  const tokens = ['sk-' + 'a'.repeat(32), 'sk-proj-' + 'a'.repeat(32), 'sk-ant-' + 'a'.repeat(32), 'ghp_' + 'b'.repeat(36), 'github_pat_' + 'c'.repeat(30), 'AKIA' + 'A'.repeat(16), 'eyJ' + 'd'.repeat(12) + '.' + 'e'.repeat(12) + '.' + 'f'.repeat(12), 'ab'.repeat(24), 'synthetic-environment-secret', 'Bearer short-value', 'token=short-value', 'api_key:short-value'];
  const output = scrubDiagnostic({ text: [...paths, ...tokens, 'sample-person'].join('\n'), access_token: 'short-hidden-value', nested: { password: 'other-hidden-value' } }, diagnosticOptions);
  for (const forbidden of ['scratch', 'synthetic-home', 'sample-person', 'other-person', 'another-person', 'runner-person', 'escaped-person', 'short-value', 'short-hidden-value', 'other-hidden-value', ...tokens.slice(0, -3)]) assert.equal(output.includes(forbidden), false, forbidden);
  assert.equal(scrubDiagnostic({ error: 'plugin updater not found', port: 18800, passed: false }, diagnosticOptions), '{"error":"plugin updater not found","port":18800,"passed":false}');
  const row = failureDiagnostic({ status: 'fail', name: 'no-console-errors', details: [`${tokens[0]} ${'diagnostic text '.repeat(500)}`] }, diagnosticOptions);
  assert.ok(row.length <= 2000);
  assert.match(row, /truncated/);
  assert.equal(row.includes(tokens[0]), false);
});

test('CI diagnostics omit PNG and binary payloads without reading output files', () => {
  const base64 = 'iVBORw0KGgo' + 'A'.repeat(80);
  const output = scrubDiagnostic({ png: [137, 80, 78, 71], screenshot: 'private image payload', buffer: Buffer.from('private binary payload'), typed: new Uint8Array([137, 80, 78, 71]), raw: '\u0089PNG\r\n\u001a\nprivate PNG payload', data: `data:image/png;base64,${base64}`, bare: base64 }, diagnosticOptions);
  for (const forbidden of ['137', 'private', 'iVBOR', 'data:image/png', base64]) assert.equal(output.includes(forbidden), false, forbidden);
  const circular = {}; circular.self = circular;
  assert.match(scrubDiagnostic(circular, diagnosticOptions), /circular/);
});

test('failed mutation diagnostics retain the name and all three phase results within one row', async () => {
  const report = { mutations: [] }, rows = [];
  const prove = mutationProver(report, (name, passed, details, context) => {
    if (!passed) rows.push(failureDiagnostic({ name, status: 'fail', details, context }, diagnosticOptions));
  });
  await prove('no-console-errors', async () => ({ details: ['observed console error ' + 'details '.repeat(1000)], passed: false }), async () => async () => {});
  assert.equal(rows.length, 1);
  assert.ok(rows[0].length <= 2000);
  assert.match(rows[0], /name="no-console-errors"/);
  for (const phase of ['before', 'mutant', 'restored']) assert.ok(rows[0].includes(`${phase}={"passed":false,`), phase);
  assert.match(rows[0], /error=null/);
  for (const name of reviewProofNames) assert.ok(failureDiagnostic({ name: 'mutation-proof', status: 'fail', details: { name } }, diagnosticOptions).includes(`name="${name}"`), name);
});
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


test('badge paint detects maximal 1-4px bands and ignores border, shadow and blank background', async () => {
  const { deflateSync } = await import('node:zlib');
  const { badgePaint } = await import('./png.mjs');
  const png = (thickness = 0, ink = [0, 0, 0]) => {
    const width = 60, height = 30;
    const chunk = (type, data) => {
      const result = Buffer.alloc(data.length + 12);
      result.writeUInt32BE(data.length); result.write(type, 4); data.copy(result, 8);
      return result;
    };
    const header = Buffer.alloc(13);
    header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 2;
    const pixels = Buffer.alloc(height * (width * 3 + 1));
    for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
      const rgb = y >= 14 && y < 14 + thickness ? ink : y < 2 || y >= height - 2 ? [20, 20, 20] : [40, 40, 40];
      pixels.set(rgb, y * (width * 3 + 1) + 1 + x * 3);
    }
    return Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]), chunk('IHDR', header), chunk('IDAT', deflateSync(pixels)), chunk('IEND', Buffer.alloc(0))]);
  };
  assert.equal(badgePaint(png()).clear, true);
  for (const ink of [[0, 0, 0], [255, 0, 0]]) for (let thickness = 1; thickness <= 4; thickness++) {
    const result = badgePaint(png(thickness, ink));
    assert.equal(result.clear, false, `${thickness}px ${ink}`);
    assert.equal(result.centralHorizontalRuns.length, thickness);
  }
  assert.equal(badgePaint(png(5)).clear, true, 'explicit 4px bound; do not slice thicker bands');
});
