import path from 'node:path';
import { pngPixels, badgePaint } from './png.mjs';
import { mutationProver, styleChange, removeNode, textChange, appendNode, offscreenTarget, emptyContents, duplicateIndicators } from './mutations.mjs';
import { updaterState, classifyUpdater } from './updater.mjs';
import { classifyFinding } from './findings.mjs';
import { pressOutsideTerminal, proveTerminalFocus } from './keyboard.mjs';

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
export async function selectSession(page, session, report) {
  if (new URL(page.url()).pathname !== '/') await page.getByRole('link', { name: 'Sessions', exact: true }).click();
  const row = page.locator('.session-button').filter({ hasText: session.name }).first();
  // Enter can reach xterm if terminal startup moves focus between Playwright's
  // locator focus and key dispatch. Activate the shipped click handler directly.
  const selectionState = async () => ({
    active: await row.getAttribute('aria-current'),
    selectedAgents: await page.locator('.tree-row[aria-selected="true"]').evaluateAll(rows => rows.map(node => node.getAttribute('data-agent-id'))),
    renameOpen: await page.getByRole('textbox', { name: 'Session Name', exact: true }).count() > 0,
  });
  const before = await selectionState();
  // The padding belongs to the row, away from the name, Rename and attention
  // controls. Exercise the same shipped selection handler without a nested hit.
  await row.click({ position: { x: 4, y: 4 } });
  await expandAgents(page, session);
  if (report) (report.selectionEvidence ??= []).push({ sessionId: session.id, before, after: await selectionState() });
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
  const cardCount = await pane.locator(`${attention}:visible`).count(), badgeCount = await row.locator('.attention-badge:visible').count();
  return { paneExists: await pane.count() === 1, rowExists: await row.count() === 1, card: cardCount > 0, badge: badgeCount > 0, cardCount, badgeCount, reason: await pane.locator(`${attention}:visible .reason`).allTextContents() };
}
export async function chatState(pane, manifest) {
  const chat = pane.locator(`${chatSelector}:visible`).first();
  const text = await chat.textContent();
  const table = await chat.locator('[aria-label="Markdown table"] table').evaluateAll((tables, expected) => tables.some(node => [...node.querySelectorAll('th')].some(cell => cell.textContent.trim() === expected.header) && [...node.querySelectorAll('tbody td')].some(cell => cell.textContent.trim() === expected.cell)), { header: manifest.chat.tableHeader, cell: manifest.chat.tableCell });
  return { table, code: await chat.locator('pre[aria-label^="Code block"] code').allTextContents().then(items => items.some(item => item.includes(manifest.chat.codeText))), literal: text.includes(manifest.chat.literalHtml) && await chat.locator('[data-fixture="literal"]').count() === 0 };
}

export async function visibleTargetState(target) {
  const visible = await target.isVisible();
  if (!visible) return { visible: false, text: '', visibleText: '' };
  return target.evaluate(node => {
    const box = node.getBoundingClientRect();
    const intersects = (rect, clip) => rect.right > clip.left && rect.left < clip.right && rect.bottom > clip.top && rect.top < clip.bottom;
    const viewport = { left: 0, top: 0, right: innerWidth, bottom: innerHeight };
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT), texts = [];
    while (walker.nextNode()) {
      const leaf = walker.currentNode;
      if (!leaf.textContent.trim()) continue;
      const range = document.createRange();
      const clip = { left: Math.max(0, box.left), top: Math.max(0, box.top), right: Math.min(innerWidth, box.right), bottom: Math.min(innerHeight, box.bottom) };
      let shown = true;
      for (let parent = leaf.parentElement; parent; parent = parent.parentElement) {
        const style = getComputedStyle(parent);
        if (style.display === 'none' || style.visibility !== 'visible' || Number(style.opacity) === 0) { shown = false; break; }
        const bounds = parent.getBoundingClientRect();
        if (/(auto|scroll|hidden|clip)/.test(style.overflowX)) { clip.left = Math.max(clip.left, bounds.left); clip.right = Math.min(clip.right, bounds.right); }
        if (/(auto|scroll|hidden|clip)/.test(style.overflowY)) { clip.top = Math.max(clip.top, bounds.top); clip.bottom = Math.min(clip.bottom, bounds.bottom); }
      }
      if (!shown || clip.right <= clip.left || clip.bottom <= clip.top) continue;
      // Measure each word; one visible line must not make the rest of a long
      // text node count as visible expected content below a scroll boundary.
      for (const word of leaf.textContent.matchAll(/\S+/g)) {
        range.setStart(leaf, word.index); range.setEnd(leaf, word.index + word[0].length);
        if ([...range.getClientRects()].some(rect => intersects(rect, clip))) texts.push(word[0]);
      }
    }
    return { visible: intersects(box, viewport) && texts.length > 0, text: node.innerText, visibleText: texts.join(' '), rect: { x: box.x, y: box.y, width: box.width, height: box.height } };
  });
}
export async function diagnoseReview({ page, manifest, report, out }) {
  const session = manifest.sessions.find(item => item.id === manifest.screens.review.sessionId);
  await selectSession(page, session, report);
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
  const resumable = session.agents.find(agent => agent.state === 'finished' && agent.providerSessionId);
  const unavailableSession = manifest.sessions.find(item => item.state === 'Completed' && item.agents.some(agent => agent.state === 'finished' && !agent.providerSessionId));
  const unavailable = unavailableSession.agents.find(agent => agent.state === 'finished' && !agent.providerSessionId);
  let settled = false;
  const prove = mutationProver(report, assert);
  const probe = async (name, fn, context) => { try { const result = await fn(); assert(name, result.passed, result.details, context); } catch (error) { assert(name, false, error.message, context); } };
  const capture = async (screen, viewport, target, expected, variant, expectedVisible = expected) => {
    console.log(`render:check capture ${screen} ${viewport.width}x${viewport.height}`);
    const context = { screen, viewport: `${viewport.width}x${viewport.height}`, ...(variant ? { variant } : {}) };
    const screenMeasure = async () => {
      const details = await visibleTargetState(target);
      return { passed: details.visible && expected(details.text) && expectedVisible(details.visibleText), details };
    };
    await probe('screen-not-empty', screenMeasure, context);
    await prove('screen-not-empty', screenMeasure, () => styleChange(target, { display: 'none' }), context);
    await prove('screen-target-offscreen', screenMeasure, () => offscreenTarget(target), context);
    if (screen === 'grid') await prove('grid-target-offscreen', screenMeasure, () => offscreenTarget(target), context);
    if (screen === 'sidebar') await prove('sidebar-target-offscreen', screenMeasure, () => offscreenTarget(target), context);
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
      const badge = page.locator('.app-version[title="Installed version"]');
      const paintMeasure = async () => {
        const rect = await badge.boundingBox();
        if (!rect) return { passed: false, details: 'Badge has no clip' };
        // page clip preserves pointer-transparent overlays; locator screenshots
        // can scroll the target before painting.
        const details = badgePaint(await page.screenshot({ clip: rect }));
        return { passed: details.clear, details };
      };
      await probe('version-badge-no-painted-line', paintMeasure, context);
      for (const [name, thickness] of [['line', 1], ['band', 2]]) {
        await prove(`badge-painted-${name}-${viewport.width}`, paintMeasure, async () => {
          const rect = await badge.boundingBox();
          return appendNode(page.locator('body'), { attributes: { style: `position:fixed;left:${rect.x}px;top:${Math.floor(rect.y + rect.height / 2)}px;width:${rect.width}px;height:${thickness}px;background:black;pointer-events:none;z-index:2147483647` } });
        }, context);
      }
    }
    else report.modalBadgeObservation = { ...(await badgeGeometry(page)), scope: 'Modal backdrop intentionally interdicts background controls; normal-screen badge checks remain gating.' };
  };
  const resumeChecks = async (agent, expected, mode, viewport) => {
    const pane = paneFor(page, agent);
    const button = pane.getByRole('button', { name: `Resume ${agent.label}`, exact: true });
    const name = `resume-${expected ? 'available' : 'unavailable'}-${mode.toLowerCase()}-${viewport.width}`;
    const context = { screen: mode.toLowerCase(), viewport: `${viewport.width}x${viewport.height}`, agentId: agent.id };
    const measure = async () => {
      const paneExists = await pane.count() === 1;
      const count = await button.count();
      const status = paneExists ? await pane.locator('.pane-status-text').textContent() : null;
      const visible = count === 1 ? await visibleTargetState(button) : { visible: false, visibleText: '' };
      const enabled = count === 1 && !await button.isDisabled();
      const buttonBox = count === 1 ? await button.boundingBox() : null;
      const paneBox = paneExists ? await pane.boundingBox() : null;
      const contained = !!buttonBox && !!paneBox && buttonBox.width > 0 && buttonBox.height > 0
        && buttonBox.x >= Math.max(0, paneBox.x) - 1 && buttonBox.y >= Math.max(0, paneBox.y) - 1
        && buttonBox.x + buttonBox.width <= Math.min(viewport.width, paneBox.x + paneBox.width) + 1
        && buttonBox.y + buttonBox.height <= Math.min(viewport.height, paneBox.y + paneBox.height) + 1;
      return { passed: paneExists && status === 'Completed' && (expected
        ? count === 1 && visible.visible && visible.visibleText === 'Resume' && enabled && contained
        : count === 0), details: { paneExists, status, count, visible, enabled, contained, buttonBox, paneBox, expected } };
    };
    await probe(name, measure, context);
    if (expected) await prove(`resume-clip-${mode.toLowerCase()}-${viewport.width}`, measure, async () => {
      const buttonBox = await button.boundingBox(), paneBox = await pane.boundingBox();
      return styleChange(button, { transform: `translateX(${paneBox.x + paneBox.width - buttonBox.x - buttonBox.width + 6}px)` });
    }, context);
    await prove(name, measure, () => expected ? removeNode(button) : appendNode(pane.locator('.terminal-header'), {
      tag: 'button', attributes: { 'aria-label': `Resume ${agent.label}` }, text: 'Resume',
    }), context);
  };
  const gridChecks = async (viewport, stage) => {
    const context = { screen: 'grid', viewport: `${viewport.width}x${viewport.height}`, stage };
    const panes = page.locator('.terminal-panel:not(.hidden) .terminal-grid .terminal-item');
    const visiblePaneCount = async () => (await Promise.all((await panes.all()).map(pane => visibleTargetState(pane)))).filter(item => item.visible).length;
    const visible = await visiblePaneCount();
    assert('grid-eight-panes', visible === manifest.screens.grid.paneCount, { visible, expected: manifest.screens.grid.paneCount }, context);
    await prove('grid-eight-panes', async () => ({ passed: await visiblePaneCount() === manifest.screens.grid.paneCount }), () => styleChange(panes.first(), { display: 'none' }), context);
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
        if (!needs) await prove(`finished-duplicate-${kind}`, measure, () => duplicateIndicators(parent, { tag: kind === 'attention-card' ? 'section' : 'span', attributes: kind === 'attention-card' ? { 'aria-label': 'Agent needs attention' } : { class: 'attention-badge' }, text: 'Duplicate mutation needs attention' }), context);
      }
    }
    for (const [agent, hasAttention] of [[normal, false], [stalled, true]]) {
      const pane = paneFor(page, agent);
      await showChat(pane);
      await probe(`grid-composer-${hasAttention ? 'with' : 'without'}-attention`, async () => { const details = await composerGeometry(pane); const card = (await pane.locator(`${attention}:visible`).count()) > 0; return { passed: details.contained && card === hasAttention, details: { ...details, card } }; }, context);
      const name = `grid-composer-${hasAttention ? 'with' : 'without'}-attention`;
      if (!hasAttention) await prove('composer-offscreen-not-known', async () => {
        const details = { ...await composerGeometry(pane), card: false };
        const classified = classifyFinding(name, details.contained, details, { [name]: { issue: 414 } });
        return { passed: classified.accepted, details: { ...details, classified } };
      }, () => styleChange(pane.locator(`${composer}:visible`).first(), { position: 'fixed', left: '-10000px' }), context);
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
    const projectField = page.locator('#launch-project-value');
    const projectMeasure = async () => {
      const value = await projectField.textContent();
      return { passed: path.isAbsolute(value) && !value.startsWith('\\\\?\\') && value === report.root, details: { value, expected: report.root } };
    };
    await probe('launch-project-ordinary-path', projectMeasure, { screen: 'launch', viewport: `${viewport.width}x${viewport.height}` });
    await prove('launch-project-verbatim-prefix', projectMeasure, async () => textChange(projectField, `\\\\?\\${await projectField.textContent()}`), { screen: 'launch' });
    await capture('launch', viewport, page.locator('.launch-inline'), text => text.includes('What would you like to work on?'));
    // Hold capabilities only for the focus mutation. This exposes the real
    // pre-roster xterm mount without changing app source or fixture data.
    if (!report.mutations.some(item => item.name === 'keyboard-focus-terminal')) {
      let release;
      const ready = new Promise(resolve => { release = resolve; });
      const hold = async route => { await ready; await route.continue().catch(() => {}); };
      await page.route('**/agent-feeds', hold);
      try {
        await selectSession(page, session, report);
        const terminal = page.locator('.terminal-panel:not(.hidden) .xterm-helper-textarea').first();
        await terminal.waitFor({ state: 'attached' });
        await proveTerminalFocus({ page, target: page.locator('.session-button').filter({ hasText: session.name }).first(), terminal, prove });
      } finally { release(); await page.unroute('**/agent-feeds', hold); }
    } else await selectSession(page, session, report);
    await viewMode(page, 'Focus');
    await page.locator(`.tree-row[data-agent-id=${JSON.stringify(manifest.screens.focus.agentId)}]`).click();
    const focused = paneFor(page, normal);
    await showChat(focused);
    await focused.locator('[aria-label="Markdown table"] table').first().waitFor();
    await focused.locator('[aria-label="Markdown table"] table').first().scrollIntoViewIfNeeded();
    const chat = await chatState(focused, manifest);
    for (const key of ['table', 'code', 'literal']) assert(`chat-${key}`, chat[key], chat, { screen: 'focus', viewport: `${viewport.width}x${viewport.height}` });
    for (const key of ['table', 'code', 'literal']) await prove(`chat-${key}`, async () => { const details = await chatState(focused, manifest); return { passed: details[key], details }; }, () => key === 'table' ? removeNode(focused.locator('[aria-label="Markdown table"] table').first()) : key === 'code' ? textChange(focused.locator('pre[aria-label^="Code block"] code').first(), 'mutated code') : appendNode(focused.locator(`${chatSelector}:visible`).first(), { attributes: { 'data-fixture': 'literal' } }), { screen: 'focus' });
    for (const key of ['table', 'code']) await prove(`chat-empty-${key}`, async () => { const details = await chatState(focused, manifest); return { passed: details[key], details }; }, () => emptyContents(focused.locator(key === 'table' ? '[aria-label="Markdown table"] table' : 'pre[aria-label^="Code block"] code').first()), { screen: 'focus' });
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
    await resumeChecks(resumable, true, 'Grid', viewport);
    // Full labels remain required in the DOM. Shipped headers truncate them;
    // visible fixture state text and each pane's viewport check prove content.
    await capture('grid', viewport, page.locator('.terminal-panel:not(.hidden) .terminal-grid'),
      text => session.agents.every(agent => text.includes(agent.label)), undefined,
      text => text.includes('No heartbeat has arrived') && text.includes("This agent's process exited"));
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
    await selectSession(page, session, report);
    const recent = page.getByRole('button', { name: 'Recent', exact: true });
    if (await recent.getAttribute('aria-expanded') === 'false') await recent.click();
    for (const id of manifest.screens.sidebar.sessionIds) {
      const item = manifest.sessions.find(session => session.id === id);
      const rows = page.locator(item.state === 'Completed' ? '.session-item.recent' : '.session-button').filter({ hasText: item.name });
      assert('sidebar-fixture-present', await rows.count() > 0, { sessionId: id }, { screen: 'sidebar', viewport: `${viewport.width}x${viewport.height}` });
      await prove('sidebar-fixture-present', async () => ({ passed: await rows.count() > 0 }), () => removeNode(rows.first()));
    }
    await capture('sidebar', viewport, page.locator('.session-agents[aria-label="Session agents"]'),
      text => session.agents.every(agent => text.includes(agent.label)), undefined,
      text => text.includes(normal.label) && text.includes(`Agents (${session.agents.length})`));
    const staleSession = manifest.sessions.find(item => item.id === manifest.screens.sidebar.staleSessionId);
    const staleBadge = page.getByRole('button', { name: `1 need attention in ${staleSession.name}`, exact: true });
    await staleBadge.scrollIntoViewIfNeeded();
    const staleMeasure = async () => {
      const count = await staleBadge.count();
      const visible = count === 1 && (await visibleTargetState(staleBadge)).visible;
      return { passed: visible, details: { sessionId: staleSession.id, count, visible } };
    };
    await probe('sidebar-inactive-stale-only', staleMeasure, { screen: 'sidebar', viewport: `${viewport.width}x${viewport.height}` });
    await prove('sidebar-inactive-stale-only', staleMeasure, () => removeNode(staleBadge));
    const staleRow = page.locator('.session-row').filter({ has: staleBadge });
    const sessionName = staleRow.locator('.session-name');
    const compactMeasure = async () => {
      const details = await staleBadge.evaluate(node => {
        const box = node.getBoundingClientRect();
        return { width: box.width, height: box.height };
      });
      return { passed: details.width > 0 && details.height > 0 && details.height < 32, details };
    };
    const singleLineMeasure = async () => {
      const details = await sessionName.evaluate(node => {
        const range = document.createRange();
        range.selectNodeContents(node);
        const lines = new Set([...range.getClientRects()]
          .filter(rect => rect.width > 0 && rect.height > 0).map(rect => Math.round(rect.top))).size;
        const box = node.getBoundingClientRect();
        return { width: box.width, height: box.height, lines };
      });
      return { passed: details.width > 0 && details.height > 0 && details.lines === 1, details };
    };
    const sidebarContext = { screen: 'sidebar', viewport: String(viewport.width) + 'x' + String(viewport.height) };
    await probe('sidebar-session-attention-compact', compactMeasure, sidebarContext);
    await prove('sidebar-session-attention-compact', compactMeasure,
      () => styleChange(staleBadge, { height: '90px', 'min-height': '90px' }), sidebarContext);
    await probe('sidebar-session-name-single-line', singleLineMeasure, sidebarContext);
    await prove('sidebar-session-name-single-line', singleLineMeasure,
      () => styleChange(sessionName, { width: '32px', flex: '0 0 32px', 'white-space': 'normal' }), sidebarContext);
    const fullNamesMeasure = async () => {
      const details = [];
      for (const expected of [session.name, staleSession.name]) {
        const name = page.locator('.session-button .session-name').filter({ hasText: expected });
        await name.scrollIntoViewIfNeeded();
        const painted = await visibleTargetState(name);
        const fit = await name.evaluate(node => ({ width: node.clientWidth, textWidth: node.scrollWidth }));
        details.push({ expected, ...painted, ...fit });
      }
      return { passed: details.every(item => item.visible && item.visibleText.includes(item.expected)
        && item.width > 0 && item.textWidth <= item.width + 1), details };
    };
    await probe('sidebar-session-names-painted', fullNamesMeasure, sidebarContext);
    await prove('sidebar-session-names-painted', fullNamesMeasure,
      () => styleChange(sessionName, { width: '32px', flex: '0 0 32px' }), sidebarContext);
    await staleRow.scrollIntoViewIfNeeded();
    await capture('sidebar', viewport, page.locator('.session-row').filter({ has: staleBadge }),
      text => text.includes(staleSession.name) && text.includes('1'), 'inactive-stale');
    const agentsDisclosure = page.getByRole('button', { name: `Agents (${session.agents.length})`, exact: true });
    await agentsDisclosure.click();
    const recentRows = page.locator('.session-item.recent');
    const recentMeasure = async () => ({ passed: await recentRows.count() === manifest.sessions.length && await recentRows.count() >= manifest.screens.sidebar.recentMinimum,
      details: { count: await recentRows.count(), expected: manifest.sessions.length } });
    await probe('sidebar-all-recent-sessions', recentMeasure, { screen: 'sidebar', viewport: `${viewport.width}x${viewport.height}` });
    await prove('sidebar-all-recent-sessions', recentMeasure, () => removeNode(recentRows.nth(5)));
    await recentRows.last().scrollIntoViewIfNeeded();
    await capture('sidebar', viewport, page.getByRole('list', { name: 'Recent sessions' }),
      text => manifest.sessions.every(item => text.includes(item.name)), 'recent',
      text => manifest.sessions.filter(item => text.includes(item.name)).length > 5);
    await agentsDisclosure.click();
    const reviewAgentSelector = `.tree-row[data-agent-id=${JSON.stringify(manifest.screens.review.agentId)}]`;
    await page.locator(reviewAgentSelector).click();
    await page.waitForFunction(selector => document.querySelector(selector)?.getAttribute('aria-selected') === 'true', reviewAgentSelector);
    await openReview(page);
    const reviewCell = cellSelect(page), reviewPrincipal = principalSelect(page);
    if (await reviewCell.inputValue() !== manifest.screens.review.cellId) await reviewCell.selectOption(manifest.screens.review.cellId);
    if (await reviewPrincipal.inputValue() !== manifest.screens.review.agentId) await reviewPrincipal.selectOption(manifest.screens.review.agentId);
    await page.waitForFunction(expected => {
      const labels = [...document.querySelectorAll('.review-view .pickers > label')];
      const value = name => labels.find(label => label.textContent.startsWith(name))?.querySelector('select')?.value;
      return value('Cell') === expected.cellId && value('Principal') === expected.agentId;
    }, manifest.screens.review);
    (report.reviewSelectionEvidence ??= []).push({ viewport: viewport.width,
      cellId: await reviewCell.inputValue(), agentId: await reviewPrincipal.inputValue(),
      selectedAgent: await page.locator(reviewAgentSelector).getAttribute('aria-selected') });
    await page.locator(`.file-review[aria-label=${JSON.stringify(manifest.screens.review.file)}]`).waitFor();
    await capture('review', viewport, page.locator('.review-view'), text => text.includes(manifest.screens.review.file));
    if (!manifest.screens.dialog.action.includes('Keyboard Shortcuts')) throw new Error('Manifest dialog does not name the shipped Keyboard Shortcuts dialog');
    await pressOutsideTerminal(page, page.getByRole('link', { name: 'Sessions', exact: true }), 'Control+/', assert);
    const dialog = page.getByRole('dialog', { name: 'Keyboard shortcuts', exact: true });
    await dialog.waitFor();
    await capture('dialog', viewport, dialog, text => text.includes('Keyboard Shortcuts'));
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    if (await toggle.getAttribute('aria-expanded') === 'true') await toggle.click();
  }
  // Check completed history after baseline Review/dialog captures so this
  // additional session navigation cannot change their selection state.
  const resumePanelToggle = page.locator('#title-bar-panel-toggle');
  if (await resumePanelToggle.getAttribute('aria-expanded') === 'true') await resumePanelToggle.click();
  for (const viewport of [{ width: 1440, height: 900 }, { width: 800, height: 900 }]) {
    if (signal.aborted) throw new Error('Resume render checks interrupted');
    await page.setViewportSize(viewport);
    await selectSession(page, session, report);
    await viewMode(page, 'Focus');
    await page.locator(`.tree-row[data-agent-id=${JSON.stringify(resumable.id)}]`).click();
    await resumeChecks(resumable, true, 'Focus', viewport);
    await capture('focus', viewport, paneFor(page, resumable),
      text => text.includes(resumable.label) && text.includes('Resume') && text.includes('Completed'), 'resume-available',
      text => text.includes('Resume') && text.includes('Completed'));
    await page.locator(`.tree-row[data-agent-id=${JSON.stringify(normal.id)}]`).click();
    await page.getByRole('link', { name: 'Dashboard', exact: true }).click();
    // Open the already-loaded completed fixture through the shipped Dashboard
    // action. Recent's Load action intentionally refuses spawns in isolation.
    const unavailableCard = page.locator(`article.card[aria-label=${JSON.stringify(unavailableSession.name)}]`);
    await unavailableCard.scrollIntoViewIfNeeded();
    await unavailableCard.getByRole('button', { name: 'Open session', exact: true }).click();
    await page.locator('.terminal-panel:not(.hidden)').waitFor({ state: 'visible' });
    await expandAgents(page, unavailableSession);
    await viewMode(page, 'Focus');
    await page.locator(`.tree-row[data-agent-id=${JSON.stringify(unavailable.id)}]`).click();
    await resumeChecks(unavailable, false, 'Focus', viewport);
    await capture('focus', viewport, paneFor(page, unavailable),
      text => text.includes(unavailable.label) && text.includes('Completed'), 'resume-unavailable', text => text.includes('Completed'));
    await viewMode(page, 'Grid');
    await resumeChecks(unavailable, false, 'Grid', viewport);
    await capture('grid', viewport, page.locator('.terminal-panel:not(.hidden) .terminal-grid'),
      text => text.includes(unavailable.label) && text.includes('Completed'), 'resume-unavailable', text => text.includes('Completed'));
    await selectSession(page, session, report);
  }
  const updater = await updaterState(page);
  assert('updater-invoke-rejected', updater.rejected, updater);
  let injectedOutcome;
  const updaterMeasure = async () => {
    const details = injectedOutcome === undefined ? await updaterState(page) : classifyUpdater(injectedOutcome);
    return { passed: details.rejected, details };
  };
  const injectOutcome = async outcome => {
    injectedOutcome = outcome;
    return async () => { injectedOutcome = undefined; };
  };
  // Exercise the same classifier without writing the adapter or replacing the frozen bridge.
  await prove('updater-invoke-rejected', updaterMeasure, () => injectOutcome({}), { boundary: 'in-memory outcome' });
  await prove('updater-network-rejection', updaterMeasure, () => injectOutcome({ error: 'network failed after updater HTTP request' }), { boundary: 'in-memory outcome' });
  assert('all-screens-captured', Object.keys(manifest.screens).every(screen => [1440, 800].every(width => report.screenshots.some(item => item.screen === screen && item.viewport.width === width))), { captures: report.screenshots.map(item => `${item.screen}-${item.viewport.width}`) });
  const complete = () => Object.keys(manifest.screens).every(screen => [1440, 800].every(width => report.screenshots.some(item => item.screen === screen && item.viewport.width === width)));
  await prove('all-screens-captured', async () => ({ passed: complete(), details: report.screenshots.map(item => `${item.screen}-${item.viewport.width}`) }), async () => { const removed = report.screenshots.pop(); return async () => { report.screenshots.push(removed); }; });
}
