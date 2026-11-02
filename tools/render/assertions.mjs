import path from 'node:path';
import { readFile, writeFile } from 'node:fs/promises';
import { pngPixels } from './png.mjs';
import { mutationProver, styleChange, removeNode, textChange, appendNode } from './mutations.mjs';
import { updaterState } from './updater.mjs';

const attention = 'section[aria-label="Agent needs attention"]';
const composer = '.composer-wrap[aria-label="Message composer"]';
const chatSelector = '.chat-pane[aria-label="Agent chat"]';
// Existing shipped selectors, inventoried read-only; no component test hooks added.
export function paneFor(page, agent) {
  return page.locator('.terminal-panel:not(.hidden) .terminal-item').filter({ has: page.locator(`button[aria-label=${JSON.stringify(`Focus ${agent.label}`)}]`) });
}
export async function expandAgents(page, session) {
  const group = page.locator('.session-agents[aria-label="Session agents"]');
  const collapsed = group.locator(':scope > button[aria-expanded="false"]');
  if (await collapsed.count()) await collapsed.click();
  const queen = session.agents.find(agent => agent.role === 'Queen');
  const row = page.locator(`.tree-row[data-agent-id=${JSON.stringify(queen.id)}]`);
  if (await row.getAttribute('aria-expanded') === 'false') await row.getByRole('button', { name: 'Expand', exact: true }).click();
}
export async function selectSession(page, session) {
  if (new URL(page.url()).pathname !== '/') await page.getByRole('link', { name: 'Sessions', exact: true }).click();
  const row = page.locator('.session-button').filter({ hasText: session.name }).first();
  await row.focus();
  await row.press('Enter');
  await expandAgents(page, session);
}
export async function viewMode(page, mode) {
  await page.getByRole('group', { name: 'Terminal view', exact: true }).getByRole('button', { name: mode, exact: true }).click();
}
export async function showChat(pane) {
  const button = pane.getByRole('group', { name: 'Agent pane', exact: true }).getByRole('button', { name: 'Chat', exact: true });
  if (await button.count() && await button.isVisible()) await button.click();
}
export const principalSelect = page => page.locator('.review-view .pickers > label').filter({ hasText: /^Principal/ }).locator('select');
const cellSelect = page => page.locator('.review-view .pickers > label').filter({ hasText: /^Cell/ }).locator('select');
async function openReview(page) {
  const toggle = page.locator('#title-bar-panel-toggle');
  if (await toggle.getAttribute('aria-expanded') === 'false') await toggle.click();
  await page.locator('.panel-drawer:not(.collapsed)').waitFor({ state: 'visible' });
  const changes = page.getByRole('group', { name: 'Panel views', exact: true }).getByRole('button', { name: 'Changes', exact: true });
  // The active top-level tab collapses on click; preserve its current state.
  if (await changes.getAttribute('aria-pressed') !== 'true') await changes.click();
  await page.getByRole('group', { name: 'Changes sections', exact: true }).getByRole('button', { name: 'Review', exact: true }).click();
}
export async function composerGeometry(pane) {
  const element = pane.locator(`${composer}:visible`).first();
  if (!await element.count()) return { contained: false, error: 'No visible composer in the expected pane' };
  return element.evaluate(node => {
    const parent = node.closest('.terminal-item');
    const box = node.getBoundingClientRect(), outer = parent.getBoundingClientRect();
    const rect = value => ({ x: value.x, y: value.y, width: value.width, height: value.height, right: value.right, bottom: value.bottom });
    return { contained: box.width > 0 && box.height > 0 && box.left >= outer.left - 1 && box.top >= outer.top - 1 && box.right <= outer.right + 1 && box.bottom <= outer.bottom + 1, composer: rect(box), pane: rect(outer) };
  });
}
export async function badgeGeometry(page) {
  return page.locator('.app-version[title="Installed version"]').evaluate(node => {
    const rect = node.getBoundingClientRect(), x = rect.x + rect.width / 2, y = rect.y + rect.height / 2;
    const stack = document.elementsFromPoint(x, y);
    const style = getComputedStyle(node);
    const top = stack[0];
    return { unobscured: !!top && (top === node || node.contains(top)) && style.visibility === 'visible' && style.display !== 'none' && Number(style.opacity) > 0 && !style.textDecorationLine.includes('line-through'), rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height }, center: { x, y }, styles: { textDecoration: style.textDecorationLine, boxShadow: style.boxShadow, borderTop: style.borderTop, background: style.backgroundColor, zIndex: style.zIndex, opacity: style.opacity, visibility: style.visibility }, stack: stack.slice(0, 8).map(el => ({ tag: el.tagName, class: el.className })) };
  });
}
export async function attentionState(page, agent) {
  const pane = paneFor(page, agent), row = page.locator(`.tree-row[data-agent-id=${JSON.stringify(agent.id)}]`);
  const card = await pane.locator(attention).isVisible().catch(() => false);
  return { paneExists: await pane.count() === 1, rowExists: await row.count() === 1, card, badge: await row.locator('.attention-badge').isVisible().catch(() => false), reason: card && await pane.locator(`${attention} .reason`).count() ? await pane.locator(`${attention} .reason`).textContent() : null };
}
export async function chatState(pane, manifest) {
  const chat = pane.locator(`${chatSelector}:visible`).first();
  const text = await chat.textContent();
  return { table: await chat.locator('[aria-label="Markdown table"] table').count() > 0 && text.includes(manifest.chat.tableText), code: await chat.locator('pre[aria-label^="Code block"] code').allTextContents().then(items => items.some(item => item.includes(manifest.chat.codeText))), literal: text.includes(manifest.chat.literalHtml) && await chat.locator('[data-fixture="literal"]').count() === 0 };
}
export async function diagnoseReview({ page, manifest, report, out }) {
  const session = manifest.sessions.find(item => item.id === manifest.screens.review.sessionId);
  await selectSession(page, session);
  await page.locator(`.tree-row[data-agent-id=${JSON.stringify(manifest.screens.review.agentId)}]`).click();
  await openReview(page);
  await cellSelect(page).selectOption(manifest.screens.review.cellId);
  await principalSelect(page).selectOption(manifest.screens.review.agentId);
  await page.waitForTimeout(1500);
  report.reviewDiagnostic = await page.locator('.review-view').evaluate(node => ({ text: node.innerText, selects: [...node.querySelectorAll('select')].map(select => ({ id: select.id, name: select.name, aria: select.getAttribute('aria-label'), labels: [...select.labels].map(label => label.textContent), options: [...select.options].map(option => ({ value: option.value, text: option.text })) })) }));
  await page.screenshot({ path: path.join(out, 'review-diagnostic.png') });
}
export async function runRenderAssertions({ page, manifest, report, assert, out, repo, signal }) {
  const session = manifest.sessions.find(item => item.id === manifest.screens.grid.sessionId);
  const byState = state => session.agents.find(agent => agent.state === state);
  const normal = session.agents.find(agent => agent.id === manifest.chat.agentId), stalled = byState('stalled');
  let settled = false;
  const prove = mutationProver(report, assert);
  const probe = async (name, fn, context) => { try { const result = await fn(); assert(name, result.passed, result.details, context); } catch (error) { assert(name, false, error.message, context); } };
  const capture = async (screen, viewport, target, expected, variant) => {
    console.log(`render:check capture ${screen} ${viewport.width}x${viewport.height}`);
    const context = { screen, viewport: `${viewport.width}x${viewport.height}`, ...(variant ? { variant } : {}) };
    const visible = await target.isVisible().catch(() => false);
    const text = visible ? await target.innerText() : '';
    assert('screen-not-empty', visible && expected(text), { text: text.slice(0, 1500), visible }, context);
    await prove('screen-not-empty', async () => {
      const visible = await target.isVisible().catch(() => false);
      return { passed: visible && expected(await target.innerText()), details: { visible } };
    }, () => styleChange(target, { display: 'none' }), context);
    const file = path.join(out, `${screen}-${viewport.width}${variant ? `-${variant}` : ''}.png`);
    const bytes = await page.screenshot({ path: file });
    const pixels = pngPixels(bytes);
    assert('screenshot-has-pixels', pixels.hasPixels && pixels.width === viewport.width && pixels.height === viewport.height, pixels, context);
    report.screenshots.push({ screen, viewport, ...(variant ? { variant } : {}), path: path.relative(repo, file).replaceAll('\\', '/'), pixels });
    await prove('screenshot-has-pixels', async () => {
      const pixels = pngPixels(await page.screenshot());
      return { passed: pixels.hasPixels && pixels.width === viewport.width && pixels.height === viewport.height, details: pixels };
    }, () => appendNode(page.locator('body'), { attributes: { style: 'position:fixed;inset:0;background:white;z-index:2147483647' } }), context);
    if (screen === 'kanban') (report.badgeScope ??= []).push({ ...context, present: false, reason: 'UpdateChecker mounts only in routes/+page.svelte:250, not the Dashboard route.' });
    else if (screen !== 'dialog') {
      const measure = async () => { const details = await badgeGeometry(page); return { passed: details.unobscured, details }; };
      await probe('version-badge-unobscured', measure, context);
      await prove('version-badge-unobscured', measure, () => styleChange(page.locator('.app-version[title="Installed version"]'), { 'text-decoration': 'line-through' }), context);
    }
    else report.modalBadgeObservation = { ...(await badgeGeometry(page)), scope: 'Modal backdrop intentionally interdicts background controls; normal-screen badge checks remain gating.' };
  };
  const gridChecks = async (viewport, stage) => {
    const context = { screen: 'grid', viewport: `${viewport.width}x${viewport.height}`, stage };
    const panes = page.locator('.terminal-panel:not(.hidden) .terminal-grid .terminal-item');
    const visible = (await Promise.all((await panes.all()).map(pane => pane.isVisible()))).filter(Boolean).length;
    assert('grid-eight-panes', visible === manifest.screens.grid.paneCount, { visible, expected: manifest.screens.grid.paneCount }, context);
    await prove('grid-eight-panes', async () => ({ passed: (await Promise.all((await panes.all()).map(pane => pane.isVisible()))).filter(Boolean).length === manifest.screens.grid.paneCount }), () => styleChange(panes.first(), { display: 'none' }), context);
    for (const [agent, needs] of [[byState('finished'), false], [stalled, true], [byState('exited_during_startup'), true]]) {
      await probe(`${agent.state}-attention-card`, async () => { const details = await attentionState(page, agent); return { passed: details.paneExists && details.card === needs, details }; }, context);
      await probe(`${agent.state}-needs-you-count`, async () => { const details = await attentionState(page, agent); return { passed: details.rowExists && details.badge === needs, details }; }, context);
      for (const kind of ['attention-card', 'needs-you-count']) {
        const measure = async () => {
          const details = await attentionState(page, agent);
          return { passed: kind === 'attention-card' ? details.paneExists && details.card === needs : details.rowExists && details.badge === needs, details };
        };
        const parent = kind === 'attention-card' ? paneFor(page, agent) : page.locator(`.tree-row[data-agent-id=${JSON.stringify(agent.id)}]`);
        const selector = kind === 'attention-card' ? attention : '.attention-badge';
        await prove(`${agent.state}-${kind}`, measure, () => needs ? styleChange(parent.locator(selector), { display: 'none' }) : appendNode(parent, { tag: kind === 'attention-card' ? 'section' : 'span', attributes: kind === 'attention-card' ? { 'aria-label': 'Agent needs attention' } : { class: 'attention-badge' }, text: 'Mutation needs attention' }), context);
      }
    }
    for (const [agent, hasAttention] of [[normal, false], [stalled, true]]) {
      const pane = paneFor(page, agent);
      await showChat(pane);
      await probe(`grid-composer-${hasAttention ? 'with' : 'without'}-attention`, async () => { const details = await composerGeometry(pane); const card = await pane.locator(attention).isVisible().catch(() => false); return { passed: details.contained && card === hasAttention, details: { ...details, card } }; }, context);
      const name = `grid-composer-${hasAttention ? 'with' : 'without'}-attention`;
      // #414 already overflows. Establish a temporary contained control, prove
      // the exact geometry predicate rejects overflow, then restore the bug.
      if (!report.mutations.some(item => item.name === name)) {
        const box = await pane.boundingBox();
        const element = pane.locator(`${composer}:visible`).first();
        const undoControl = await styleChange(element, { position: 'fixed', left: `${box.x + 2}px`, top: `${box.y + 2}px`, width: '40px', height: '40px', 'min-height': '0', 'min-width': '0', margin: '0', transform: 'none' });
        try {
          await prove(name, async () => { const details = await composerGeometry(pane); return { passed: details.contained, details }; }, () => styleChange(element, { left: '-1000px' }), { ...context, control: 'Temporary contained DOM box; real #414 geometry restored afterwards' });
        } finally { await undoControl(); }
      }
    }
  };
  for (const viewport of [{ width: 1440, height: 900 }, { width: 800, height: 900 }]) {
    if (signal.aborted) throw new Error('Render assertions interrupted');
    await page.setViewportSize(viewport);
    const dimensions = await page.evaluate(() => ({ width: innerWidth, height: innerHeight, devicePixelRatio }));
    assert(`viewport-${viewport.width}`, dimensions.width === viewport.width && dimensions.height === viewport.height, dimensions);
    await prove(`viewport-${viewport.width}`, async () => { const details = await page.evaluate(() => ({ width: innerWidth, height: innerHeight })); return { passed: details.width === viewport.width && details.height === viewport.height, details }; }, async () => { await page.setViewportSize({ ...viewport, width: viewport.width - 10 }); return () => page.setViewportSize(viewport); });
    await page.getByRole('button', { name: 'New Session', exact: true }).click();
    await capture('launch', viewport, page.locator('.launch-inline'), text => text.includes('What would you like to work on?'));
    await selectSession(page, session);
    await viewMode(page, 'Focus');
    await page.locator(`.tree-row[data-agent-id=${JSON.stringify(manifest.screens.focus.agentId)}]`).click();
    const focused = paneFor(page, normal);
    await showChat(focused);
    await focused.locator('[aria-label="Markdown table"] table').first().waitFor();
    await focused.locator('[aria-label="Markdown table"] table').first().scrollIntoViewIfNeeded();
    const chat = await chatState(focused, manifest);
    for (const key of ['table', 'code', 'literal']) assert(`chat-${key}`, chat[key], chat, { screen: 'focus', viewport: `${viewport.width}x${viewport.height}` });
    for (const key of ['table', 'code', 'literal']) await prove(`chat-${key}`, async () => { const details = await chatState(focused, manifest); return { passed: details[key], details }; }, () => key === 'table' ? removeNode(focused.locator('[aria-label="Markdown table"] table').first()) : key === 'code' ? textChange(focused.locator('pre[aria-label^="Code block"] code').first(), 'mutated code') : appendNode(focused.locator(`${chatSelector}:visible`).first(), { attributes: { 'data-fixture': 'literal' } }), { screen: 'focus' });
    await capture('focus', viewport, focused, text => text.includes(manifest.chat.tableText) && text.includes(manifest.chat.codeText));
    const toggle = page.locator('#title-bar-panel-toggle');
    if (await toggle.getAttribute('aria-expanded') === 'true') await toggle.click();
    await viewMode(page, 'Grid');
    await gridChecks(viewport, 'load');
    if (!settled) {
      console.log('render:check observing the 60-second stall loop and 5-second sweep');
      const started = Date.now();
      while (Date.now() - started < 66000) { if (signal.aborted) throw new Error('Render check interrupted during stall/sweep proof'); await page.waitForTimeout(1000); }
      report.stallSweepWaitSeconds = (Date.now() - started) / 1000;
      settled = true;
    }
    await gridChecks(viewport, 'after-stall-and-sweep');
    await capture('grid', viewport, page.locator('.terminal-panel:not(.hidden) .terminal-grid'), text => session.agents.every(agent => text.includes(agent.label)));
    await page.getByRole('link', { name: 'Dashboard', exact: true }).click();
    await page.locator('.dashboard .board').waitFor();
    const kanbanSessions = manifest.screens.kanban.sessionIds.map(id => manifest.sessions.find(item => item.id === id));
    for (const item of kanbanSessions) {
      const card = page.locator(`article.card[aria-label=${JSON.stringify(item.name)}]`);
      await card.scrollIntoViewIfNeeded();
      assert('kanban-fixture-present', await card.isVisible(), { sessionId: item.id }, { screen: 'kanban', viewport: `${viewport.width}x${viewport.height}` });
      await prove('kanban-fixture-present', async () => ({ passed: await card.isVisible() }), () => styleChange(card, { display: 'none' }));
      // The board scrolls horizontally. Preserve a useful capture of each
      // synthetic card, rather than empty leftmost columns at narrow widths.
      await capture('kanban', viewport, page.locator('.dashboard .board'), text => text.includes(item.name), item.id);
    }
    await selectSession(page, session);
    const recent = page.getByRole('button', { name: 'Recent', exact: true });
    if (await recent.getAttribute('aria-expanded') === 'false') await recent.click();
    for (const id of manifest.screens.sidebar.sessionIds) {
      const item = manifest.sessions.find(session => session.id === id);
      const rows = page.locator('.session-button,.session-item.recent').filter({ hasText: item.name });
      assert('sidebar-fixture-present', await rows.count() > 0, { sessionId: id }, { screen: 'sidebar', viewport: `${viewport.width}x${viewport.height}` });
      await prove('sidebar-fixture-present', async () => ({ passed: await rows.count() > 0 }), () => removeNode(rows.first()));
    }
    await capture('sidebar', viewport, page.locator('.session-agents[aria-label="Session agents"]'), text => session.agents.every(agent => text.includes(agent.label)));
    await page.locator(`.tree-row[data-agent-id=${JSON.stringify(manifest.screens.review.agentId)}]`).click();
    await openReview(page);
    await cellSelect(page).selectOption(manifest.screens.review.cellId);
    await principalSelect(page).selectOption(manifest.screens.review.agentId);
    await page.locator(`.file-review[aria-label=${JSON.stringify(manifest.screens.review.file)}]`).waitFor();
    await capture('review', viewport, page.locator('.review-view'), text => text.includes(manifest.screens.review.file));
    if (!manifest.screens.dialog.action.includes('Keyboard Shortcuts')) throw new Error('Manifest dialog does not name the shipped Keyboard Shortcuts dialog');
    await page.keyboard.press('Control+/');
    const dialog = page.getByRole('dialog', { name: 'Keyboard shortcuts', exact: true });
    await dialog.waitFor();
    await capture('dialog', viewport, dialog, text => text.includes('Keyboard Shortcuts'));
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    if (await toggle.getAttribute('aria-expanded') === 'true') await toggle.click();
  }
  const updater = await updaterState(page);
  assert('updater-invoke-rejected', updater.rejected, updater);
  await prove('updater-invoke-rejected', async () => {
    const probe = await import(`./updater.mjs?proof=${Date.now()}-${Math.random()}`);
    const details = await probe.updaterState(page);
    return { passed: details.rejected, details };
  }, async () => {
    // Tauri freezes its invoke bridge. Mutate our owned adapter at the call
    // boundary instead, reload it, and restore its exact bytes under the gate.
    const file = new URL('./updater.mjs', import.meta.url);
    const original = await readFile(file);
    const source = original.toString('utf8');
    const call = "await window.__TAURI_INTERNALS__.invoke('plugin:updater|check');";
    if (source.split(call).length !== 2) throw new Error('Updater mutation target is ambiguous');
    try { await writeFile(file, source.replace(call, 'await Promise.resolve(null);')); }
    catch (error) { await writeFile(file, original); throw error; }
    return async () => {
      await writeFile(file, original);
      if (!(await readFile(file)).equals(original)) throw new Error('Updater adapter source was not restored exactly');
    };
  }, { file: 'tools/render/updater.mjs', mutation: 'Replace rejected IPC call with resolved response; restore exact source bytes in finally' });
  assert('all-screens-captured', Object.keys(manifest.screens).every(screen => [1440, 800].every(width => report.screenshots.some(item => item.screen === screen && item.viewport.width === width))), { captures: report.screenshots.map(item => `${item.screen}-${item.viewport.width}`) });
  const complete = () => Object.keys(manifest.screens).every(screen => [1440, 800].every(width => report.screenshots.some(item => item.screen === screen && item.viewport.width === width)));
  await prove('all-screens-captured', async () => ({ passed: complete(), details: report.screenshots.map(item => `${item.screen}-${item.viewport.width}`) }), async () => { const removed = report.screenshots.pop(); return async () => { report.screenshots.push(removed); }; });
}
