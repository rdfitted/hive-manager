import path from 'node:path';
import { pngPixels, badgePaint } from './png.mjs';
import { mutationProver, styleChange, removeNode, textChange, appendNode, offscreenTarget, emptyContents, duplicateIndicators } from './mutations.mjs';
import { updaterState, classifyUpdater } from './updater.mjs';
import { pressOutsideTerminal, proveTerminalFocus } from './keyboard.mjs';
import { runSecurityAssertions } from './security.mjs';

const attention = 'section[aria-label="Agent needs attention"]';
const composer = '.composer-wrap[aria-label="Message composer"]';
const chatSelector = '.chat-pane[aria-label="Agent chat"]';
// Existing shipped selectors, inventoried read-only; no component test hooks added.
export function paneFor(page, agent) {
  return page.locator('.terminal-panel:not(.hidden) .terminal-item').filter({ has: page.locator(`button[aria-label=${JSON.stringify(`Focus ${agent.label}`)}]`) });
}
export async function scrollColumn(pane) {
  await pane.scrollIntoViewIfNeeded();
  await pane.evaluate(node => {
    if (node.dataset.pinned === 'true') return;
    const grid = node.closest('.terminal-grid');
    const outer = grid.getBoundingClientRect();
    const space = parseFloat(getComputedStyle(grid).getPropertyValue('--pin-space')) || 0;
    const box = node.getBoundingClientRect();
    if (box.left < outer.left + space) grid.scrollLeft -= outer.left + space - box.left;
    const after = node.getBoundingClientRect();
    if (after.right > outer.right) grid.scrollLeft += after.right - outer.right;
  });
  // Programmatic writes precede the browser's queued scroll/clip update.
  // Matching clips and repeated animation-frame geometry signal completion.
  await waitForPinLayout(pane.page());
}
export async function waitForPinLayout(page) {
  await page.locator('.terminal-panel:not(.hidden) .terminal-grid').evaluate(grid => new Promise((resolve, reject) => {
    const started = performance.now();
    let previous, stable = 0;
    const frame = () => {
      const outer = grid.getBoundingClientRect();
      const space = parseFloat(getComputedStyle(grid).getPropertyValue('--pin-space')) || 0;
      const rows = [...grid.querySelectorAll(':scope > .terminal-item')].map(node => {
        const box = node.getBoundingClientRect();
        const pinned = node.dataset.pinned === 'true';
        const clip = parseFloat(getComputedStyle(node).getPropertyValue('--wall-clip')) || 0;
        const expected = pinned ? 0 : Math.min(box.width, Math.max(0, outer.left + space - box.left));
        return { id: node.dataset.agentId, left: box.left, right: box.right, width: box.width, pinned, clip, expected };
      });
      const snapshot = JSON.stringify({ left: outer.left, width: outer.width, scroll: grid.scrollLeft, space, rows });
      const ready = (!document.fonts || document.fonts.status === 'loaded') && rows.every(row => Math.abs(row.clip - row.expected) < 1 / 64);
      stable = ready && snapshot === previous ? stable + 1 : 0;
      previous = snapshot;
      if (stable >= 2) return resolve();
      if (performance.now() - started > 5000) return reject(new Error(`Wall scroll/clipping did not settle: ${snapshot}`));
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  }));
}
export async function pinGeometry(page, selectedId) {
  return page.locator('.terminal-panel:not(.hidden) .terminal-grid').evaluate((grid, selectedId) => {
    const outer = grid.getBoundingClientRect();
    const rows = [...grid.querySelectorAll(':scope > .terminal-item')].map(node => {
      const box = node.getBoundingClientRect(), css = getComputedStyle(node);
      const control = node.querySelector('.terminal-pane-focus'), header = control.getBoundingClientRect();
      const top = document.elementFromPoint(header.x + header.width / 2, header.y + header.height / 2);
      const clip = parseFloat(css.getPropertyValue('--wall-clip')) || 0;
      return { id: node.dataset.agentId, pinned: css.position === 'sticky', queen: node.classList.contains('queen-column'), attention: node.classList.contains('attention-column'),
        clip, box: { left: box.left, right: box.right, width: box.width }, paint: { left: Math.max(outer.left, box.left + clip), right: Math.min(outer.right, box.right) },
        hit: !!top && (top === control || control.contains(top)) };
    });
    const pins = rows.filter(row => row.pinned);
    const overlaps = (a, b) => Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1;
    const padding = parseFloat(getComputedStyle(grid).paddingLeft) || 0;
    let next = outer.left + grid.clientLeft + padding + 4;
    const offsets = pins.every(pin => { const fits = Math.abs(pin.box.left - next) <= 1; next = pin.box.right + 12; return fits; });
    const overlapPairs = [
      ...pins.flatMap((pin, index) => pins.slice(index + 1).filter(other => overlaps(pin.box, other.box)).map(other => ({ first: pin.id, second: other.id, kind: 'pinned', firstRect: pin.box, secondRect: other.box }))),
      ...rows.filter(row => !row.pinned && row.paint.right > row.paint.left).flatMap(row => pins.filter(pin => overlaps(row.paint, pin.box)).map(pin => ({ first: row.id, second: pin.id, kind: 'scrolling', firstRect: row.paint, secondRect: pin.box }))),
    ];
    const noOverlap = overlapPairs.length === 0;
    const reserve = pins.length === 0 || outer.right - next >= 280 - 1;
    const selected = rows.find(row => row.id === selectedId);
    const unselected = pins.every(pin => pin.id !== selectedId);
    return { offsets, noOverlap, reserve, unselected, hits: pins.every(pin => pin.hit) && selected?.hit === true,
      queen: pins.some(pin => pin.queen), attention: pins.some(pin => pin.attention), width: outer.width, left: outer.left, padding, pins, selected, rows, overlapPairs };
  }, selectedId);
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
export async function wallBarGeometry(page) {
  const bar = page.locator('.terminal-panel:not(.hidden) .command-bar');
  if (await bar.count() !== 1) return { contained: false, count: await bar.count() };
  return bar.evaluate(node => {
    const workspace = node.closest('.terminal-workspace').getBoundingClientRect();
    const outer = node.getBoundingClientRect();
    const elements = [node, node.querySelector('[data-composer]'), node.querySelector('.send-button'), node.querySelector('.target-picker')];
    const boxes = elements.map(element => element?.getBoundingClientRect());
    const visible = elements.every(element => {
      if (!element) return false;
      for (let current = element; current; current = current.parentElement) {
        const style = getComputedStyle(current);
        if (style.display === 'none' || style.visibility !== 'visible' || Number(style.opacity) === 0) return false;
      }
      return true;
    });
    const hit = elements.slice(1).every(element => {
      if (!element) return false;
      const box = element.getBoundingClientRect(), top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
      return !!top && (top === element || element.contains(top));
    });
    const inside = box => !!box && box.width > 0 && box.height > 0 && box.left >= Math.max(0, workspace.left, outer.left) - 1 && box.top >= Math.max(0, workspace.top, outer.top) - 1 && box.right <= Math.min(innerWidth, workspace.right, outer.right) + 1 && box.bottom <= Math.min(innerHeight, workspace.bottom, outer.bottom) + 1;
    return { contained: visible && hit && boxes.every(inside), visible, hit, boxes: boxes.map(box => box ? { x: box.x, y: box.y, width: box.width, height: box.height } : null) };
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
export async function runRenderAssertions({ page, manifest, report, assert, out, repo, signal, consoleErrors }) {
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
    if (mode === 'Grid') {
      await page.locator(`.tree-row[data-agent-id=${JSON.stringify(agent.id)}]`).click();
      await scrollColumn(pane);
    }
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
  const revealGridAgent = async agent => {
    await page.locator(`.tree-row[data-agent-id=${JSON.stringify(agent.id)}]`).click();
    const pane = paneFor(page, agent);
    await scrollColumn(pane);
    return pane;
  };
  const gridChecks = async (viewport, stage) => {
    const context = { screen: 'grid', viewport: `${viewport.width}x${viewport.height}`, stage };
    if (stage === 'load') {
      const region = page.locator('.terminal-panel:not(.hidden) .event-ticker');
      const first = region.locator('.ticker-item').first();
      const label = region.locator('.ticker-label');
      await first.waitFor({ state: 'attached' });
      const defaultTicker = async () => {
        const visible = await visibleTargetState(first), heading = await visibleTargetState(label);
        if (!visible.visible || !heading.visible) return { passed: false, details: { visible, heading, hit: false } };
        const hit = await first.evaluate(node => { const box = node.getBoundingClientRect(), top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2); return !!top && (top === node || node.contains(top)); });
        const scroll = await region.evaluate(node => node.scrollLeft);
        return { passed: !!visible.visibleText.trim() && heading.visibleText === 'Latest events' && hit && scroll === 0, details: { visible, heading, hit, scroll } };
      };
      await probe(`wall-tick-default-${viewport.width}`, defaultTicker, context);
      await prove(`wall-tick-default-${viewport.width}`, defaultTicker, () => styleChange(first, { transform: 'translateY(2000px)' }), context);
      await prove(`wall-tick-label-${viewport.width}`, defaultTicker, () => removeNode(label), context);
      const bar = page.locator('.terminal-panel:not(.hidden) .command-bar');
      const editor = bar.locator('[data-composer]'), add = bar.locator('.add-button'), picker = bar.locator('.target-picker');
      const freeTab = async () => {
        const target = await picker.textContent();
        await pressOutsideTerminal(page, editor, 'Tab');
        const forward = await add.evaluate(node => document.activeElement === node);
        if (!forward) return { passed: false, details: { forward } };
        await pressOutsideTerminal(page, add, 'Shift+Tab');
        const reverse = await editor.evaluate(node => document.activeElement === node);
        const last = bar.locator('button:enabled').last();
        await pressOutsideTerminal(page, last, 'Tab');
        const outsideForward = await bar.evaluate(node => !!document.activeElement && document.activeElement !== document.body && !node.contains(document.activeElement));
        await pressOutsideTerminal(page, editor, 'Shift+Tab');
        const pickerReached = await picker.evaluate(node => document.activeElement === node);
        await pressOutsideTerminal(page, picker, 'Shift+Tab');
        const outsideReverse = await bar.evaluate(node => !!document.activeElement && document.activeElement !== document.body && !node.contains(document.activeElement));
        const unchangedTarget = await picker.textContent() === target;
        return { passed: forward && reverse && pickerReached && outsideForward && outsideReverse && unchangedTarget, details: { forward, reverse, pickerReached, outsideForward, outsideReverse, unchangedTarget } };
      };
      await probe(`wall-tab-free-${viewport.width}`, freeTab, context);
      await prove(`wall-tab-free-${viewport.width}`, freeTab, async () => {
        const handle = await bar.evaluateHandle(node => {
          const listener = event => { if (event.key === 'Tab' && event.target instanceof Element && node.contains(event.target) && event.target.matches('[data-composer], .target-picker')) event.preventDefault(); };
          window.addEventListener('keydown', listener, true);
          return { dispose: () => window.removeEventListener('keydown', listener, true) };
        });
        return async () => { try { await handle.evaluate(guard => guard.dispose()); } finally { await handle.dispose(); } };
      }, context);
    }
    const panes = page.locator('.terminal-panel:not(.hidden) .terminal-grid .terminal-item');
    const roster = async () => {
      const ids = await panes.evaluateAll(nodes => nodes.map(node => node.dataset.agentId));
      const expected = session.agents.map(agent => agent.id);
      return { passed: ids.length === manifest.screens.grid.paneCount && new Set(ids).size === expected.length && expected.every(id => ids.includes(id)) && ids[0] === normal.id, details: { ids, expected } };
    };
    await probe('grid-eight-panes', roster, context);
    await prove('grid-eight-panes', roster, () => removeNode(panes.last()), context);
    await prove(`wall-roster-${viewport.width}`, roster, () => removeNode(panes.last()), context);
    const columnWidths = async () => {
      const widths = await panes.evaluateAll(nodes => nodes.map(node => node.getBoundingClientRect().width));
      return { passed: widths.length === 8 && widths.slice(1).every(width => width >= manifest.opswall.minimumColumnWidth - 1) && widths.slice(1).every(width => widths[0] >= width * manifest.opswall.queenRatio - 1), details: { widths } };
    };
    await probe(`wall-width-${viewport.width}`, columnWidths, context);
    await prove(`wall-width-${viewport.width}`, columnWidths, () => styleChange(panes.first(), { transform: 'scaleX(0.2)', 'transform-origin': 'left' }), context);
    const far = session.agents.at(-1);
    await revealGridAgent(far);
    const pinMeasure = async () => {
      const details = await pinGeometry(page, far.id);
      return { passed: details.offsets && details.noOverlap && details.reserve && details.unselected && details.hits && (viewport.width !== 1440 || (details.queen && details.attention)), details };
    };
    await probe(`wall-pin-${viewport.width}`, pinMeasure, context);
    await prove(`wall-pin-${viewport.width}`, pinMeasure, async () => {
      const undo = [];
      for (const pane of await panes.all()) undo.push(await styleChange(pane, { position: 'sticky', left: '4px' }));
      return async () => { for (const restore of undo.reverse()) await restore(); };
    }, context);
    if (viewport.width === 1440) await prove('wall-pin-fixed', pinMeasure, async () => {
      const undo = [];
      for (const pane of await panes.filter({ has: page.locator('.attention-chip') }).all()) undo.push(await styleChange(pane, { left: '4px' }));
      return async () => { for (const restore of undo.reverse()) await restore(); };
    }, context);
    for (const [index, agent] of session.agents.entries()) {
      const pane = paneFor(page, agent);
      await scrollColumn(pane);
      const button = pane.getByRole('button', { name: `Focus ${agent.label}`, exact: true });
      const reachable = async () => {
        const visible = await visibleTargetState(button);
        if (!visible.visible) return { passed: false, details: { agentId: agent.id, visible, hit: false } };
        const hit = await button.evaluate(node => {
          const r = node.getBoundingClientRect();
          const top = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
          return !!top && (top === node || node.contains(top));
        });
        return { passed: visible.visible && visible.visibleText.includes(agent.label) && hit, details: { agentId: agent.id, visible, hit } };
      };
      const name = `wall-col-${index}-${viewport.width}`;
      await probe(name, reachable, context);
      await prove(name, reachable, () => styleChange(button, { visibility: 'hidden' }), context);
    }
    const noColumnComposer = async () => ({ passed: await panes.locator('.chat-composer, [data-composer]').count() === 0 });
    await probe('grid-no-column-composer', noColumnComposer, context);
    await prove('grid-no-column-composer', noColumnComposer, () => appendNode(panes.first(), { attributes: { class: 'chat-composer' } }), context);
    await prove(`wall-tail-${viewport.width}`, noColumnComposer, () => appendNode(panes.first(), { attributes: { class: 'chat-composer' } }), context);
    const farPane = await revealGridAgent(far);
    await farPane.getByRole('button', { name: `Maximize ${far.label}`, exact: true }).click();
    const maximized = async () => {
      const box = await farPane.boundingBox(), outer = await page.locator('.terminal-panel:not(.hidden) .terminal-grid').boundingBox();
      const count = await panes.count(), hidden = await page.locator('.terminal-panel:not(.hidden) .terminal-grid > .terminal-item.hidden-by-maximize').count();
      const bar = await wallBarGeometry(page);
      const contained = !!box && !!outer && box.width > 0 && box.height > 0 && box.x >= Math.max(0, outer.x) - 1 && box.y >= Math.max(0, outer.y) - 1 && box.x + box.width <= Math.min(viewport.width, outer.x + outer.width) + 1 && box.y + box.height <= Math.min(viewport.height, outer.y + outer.height) + 1;
      return { passed: contained && count === 8 && hidden === 7 && bar.contained, details: { box, outer, count, hidden, bar, contained } };
    };
    await probe(`wall-max-${viewport.width}`, maximized, context);
    await prove(`wall-max-${viewport.width}`, maximized, () => styleChange(farPane, { transform: 'translateX(2000px)' }), context);
    await farPane.getByRole('button', { name: `Restore ${far.label}`, exact: true }).click();
    const waitForHeader = async id => page.waitForFunction(id => {
      const node = document.querySelector(`.terminal-panel:not(.hidden) .terminal-item[data-agent-id="${CSS.escape(id)}"] .terminal-pane-focus`);
      if (!node) return false;
      const box = node.getBoundingClientRect(), top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
      return !!top && (top === node || node.contains(top));
    }, id, { timeout: 5000 });
    const ticker = page.locator('.terminal-panel:not(.hidden) .event-ticker').getByRole('button', { name: `Task: ${manifest.opswall.tickerText}`, exact: true });
    await ticker.scrollIntoViewIfNeeded();
    const tickerVisible = async () => { const details = await visibleTargetState(ticker); return { passed: details.visible && details.visibleText.includes(manifest.opswall.tickerText), details }; };
    await probe(`wall-tick-${viewport.width}`, tickerVisible, context);
    await prove(`wall-tick-${viewport.width}`, tickerVisible, () => styleChange(ticker, { visibility: 'hidden' }), context);
    await ticker.click();
    await waitForHeader(manifest.opswall.tickerAgentId);
    const tickAgent = session.agents.find(agent => agent.id === manifest.opswall.tickerAgentId);
    const tickTarget = async () => {
      const target = page.locator('.terminal-panel:not(.hidden) .command-bar .target-picker');
      const header = paneFor(page, tickAgent).getByRole('button', { name: `Focus ${tickAgent.label}`, exact: true });
      const visible = await visibleTargetState(header);
      if (!visible.visible) return { passed: false, details: { visible, hit: false } };
      const hit = await header.evaluate(node => { const box = node.getBoundingClientRect(), top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2); return !!top && (top === node || node.contains(top)); });
      return { passed: (await target.textContent()).trim() === `@${tickAgent.label}` && visible.visible && hit, details: { visible, hit } };
    };
    await probe(`wall-tick-go-${viewport.width}`, tickTarget, context);
    await prove(`wall-tick-go-${viewport.width}`, tickTarget, () => styleChange(paneFor(page, tickAgent).locator('.terminal-pane-focus'), { visibility: 'hidden' }), context);
    const attentionTile = page.locator('.terminal-panel:not(.hidden) .resource-strip').getByRole('button', { name: /^Attention \d+$/ });
    await attentionTile.click();
    const firstAttention = session.agents.find(agent => ['stalled', 'exited_during_startup', 'delivery_failed'].includes(agent.state));
    await waitForHeader(firstAttention.id);
    const tileMatch = async () => {
      const expected = Number((await attentionTile.textContent()).replace(/\D/g, ''));
      const count = await page.locator('.terminal-panel:not(.hidden) .resource-match').count();
      return { passed: expected > 0 && count === expected && await attentionTile.getAttribute('aria-pressed') === 'true', details: { expected, count } };
    };
    await probe(`wall-tile-${viewport.width}`, tileMatch, context);
    await prove(`wall-tile-${viewport.width}`, tileMatch, () => appendNode(page.locator('.terminal-panel:not(.hidden) .terminal-grid'), { attributes: { class: 'resource-match' } }), context);
    await attentionTile.click();
    for (const [agent, needs] of [[byState('finished'), false], [stalled, true], [byState('exited_during_startup'), true]]) {
      await revealGridAgent(agent);
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
      const pane = await revealGridAgent(agent);
      await showChat(pane);
      const bar = page.locator('.terminal-panel:not(.hidden) .command-bar');
      const measure = async () => {
        const details = await wallBarGeometry(page);
        const card = (await pane.locator(`${attention}:visible`).count()) > 0;
        return { passed: details.contained && card === hasAttention, details: { ...details, card } };
      };
      const name = `grid-composer-${hasAttention ? 'with' : 'without'}-attention`;
      await probe(name, measure, context);
      await prove(name, measure, () => styleChange(bar, { transform: 'translateY(2000px)' }), context);
      await prove(`${hasAttention ? 'wall-warn' : 'wall-calm'}-${viewport.width}`, measure, () => styleChange(bar, { transform: 'translateY(2000px)' }), context);
      if (!hasAttention) {
        for (const [part, target] of [['hide', bar], ['edit', bar.locator('[data-composer]')], ['send', bar.locator('.send-button')], ['pick', bar.locator('.target-picker')]]) {
          await prove(`wall-bar-${part}-${viewport.width}`, measure, () => styleChange(target, { visibility: 'hidden' }), context);
        }
      }
      if (!hasAttention) await prove('composer-offscreen-not-known', measure, () => styleChange(bar, { position: 'fixed', left: '-10000px' }), context);
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
    await runSecurityAssertions({ page, pane: focused, report, assert, consoleErrors, sessionId: manifest.chat.sessionId,
      capture: () => capture('focus', viewport, focused, text => text.includes('Close HTML preview'), 'html') });
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
    // Every column was individually painted/reachable above; preserve captures
    // of both meaningful attention states without treating offscreen text as painted.
    for (const [agent, variant, phrase] of [[stalled, undefined, 'No heartbeat has arrived'], [byState('exited_during_startup'), 'dead', "This agent's process exited"]]) {
      await revealGridAgent(agent);
      await capture('grid', viewport, page.locator('.terminal-panel:not(.hidden) .terminal-grid'),
        text => session.agents.every(agent => text.includes(agent.label)), variant,
        text => text.includes(phrase));
    }
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
    // Exercise the persisted result envelope through the shipped conversation UI.
    const conversationFixture = manifest.conversation;
    if (await toggle.getAttribute('aria-expanded') === 'false') await toggle.click();
    const activity = page.getByRole('group', { name: 'Panel views', exact: true }).getByRole('button', { name: 'Activity', exact: true });
    if (await activity.getAttribute('aria-pressed') !== 'true') await activity.click();
    await page.getByRole('group', { name: 'Activity sections', exact: true }).getByRole('button', { name: 'Chat', exact: true }).click();
    const viewer = page.locator('.conversation-viewer');
    await viewer.getByRole('tab', { name: 'Queen', exact: true }).click();
    const diffWidget = viewer.locator('.diff-widget').first();
    await diffWidget.waitFor({ state: 'visible' });
    // Playwright visibility does not wait for the loading crossfade or queued
    // conversation autoscroll. Settle those states before taking one strict read.
    const settleConversation = async () => viewer.evaluate(node => new Promise((resolve, reject) => {
      const started = performance.now();
      let previous, stable = 0;
      const rect = element => { const box = element.getBoundingClientRect(); return { x: box.x, y: box.y, width: box.width, height: box.height }; };
      const frame = () => {
        const widget = node.querySelector('.diff-widget'), messages = node.querySelector('.messages');
        const skeleton = node.querySelector('.conversation-loading'), content = skeleton?.querySelector('.lattice-skeleton-content');
        if (widget && messages && skeleton && content) {
          const snapshot = JSON.stringify({ widget: rect(widget), messages: rect(messages), scrollTop: messages.scrollTop, scrollHeight: messages.scrollHeight, clientHeight: messages.clientHeight });
          const revealed = skeleton.getAttribute('aria-busy') === 'false' && Number(getComputedStyle(content).opacity) === 1;
          const animated = content.getAnimations().some(animation => animation.playState !== 'finished' && animation.playState !== 'idle');
          const ready = revealed && !animated && (!document.fonts || document.fonts.status === 'loaded');
          stable = ready && snapshot === previous ? stable + 1 : 0;
          previous = snapshot;
          if (stable >= 2) return resolve();
        }
        if (performance.now() - started > 5000) return reject(new Error(`Conversation reveal/scroll did not settle: ${previous}`));
        requestAnimationFrame(frame);
      };
      requestAnimationFrame(frame);
    }));
    await settleConversation();
    await diffWidget.scrollIntoViewIfNeeded();
    await settleConversation();
    const measureDiff = async () => {
      const widgets = await viewer.locator('.diff-widget').count();
      const painted = widgets === 1 ? await visibleTargetState(diffWidget) : { visible: false };
      const visible = painted.visible;
      const additions = await viewer.locator('.diff-widget .diff-line.add .line-text').allTextContents();
      const deletions = await viewer.locator('.diff-widget .diff-line.del .line-text').allTextContents();
      const ancestors = widgets === 1 ? await diffWidget.evaluate(node => {
        const rows = [];
        for (let parent = node; parent; parent = parent.parentElement) {
          const box = parent.getBoundingClientRect(), css = getComputedStyle(parent);
          rows.push({ tag: parent.tagName, className: parent.className, rect: { x: box.x, y: box.y, width: box.width, height: box.height }, display: css.display, visibility: css.visibility, opacity: css.opacity, overflowX: css.overflowX, overflowY: css.overflowY, scrollTop: parent.scrollTop, scrollHeight: parent.scrollHeight, clientHeight: parent.clientHeight });
        }
        return rows;
      }) : [];
      return { passed: visible && additions.includes(conversationFixture.added) && deletions.includes(conversationFixture.removed), details: { widgets, visible, additions, deletions, painted, ancestors } };
    };
    await probe('conversation-diff-envelope', measureDiff, { screen: 'conversation', viewport: `${viewport.width}x${viewport.height}` });
    await prove(`conv-diff-${viewport.width}`, measureDiff, () => emptyContents(diffWidget), { screen: 'conversation' });
    await capture('conversation', viewport, viewer, text => text.includes(conversationFixture.added) && text.includes(conversationFixture.removed));
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
  await runChatContrastAssertions({ page, session, manifest, report, assert, prove, out });
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
  await prove('all-screens-captured', async () => ({ passed: complete(), details: report.screenshots.map(item => `${item.screen}-${item.viewport.width}`) }), async () => {
    const original = report.screenshots;
    const target = original.at(-1);
    report.screenshots = original.filter(item => item.screen !== target.screen || item.viewport.width !== target.viewport.width);
    return async () => { report.screenshots = original; };
  });
}

// Lane B appends measurements after the complete Lane A screen/guard sequence.
export async function runChatContrastAssertions({ page, session, manifest, report, assert, prove, out }) {
  const agent = session.agents.find(item => item.id === manifest.chat.senderAgentId);
  await selectSession(page, session, report);
  for (const viewport of [{ width: 1440, height: 900 }, { width: 800, height: 900 }]) {
    await page.setViewportSize(viewport);
    for (const mode of ['Grid', 'Focus']) {
      await viewMode(page, mode);
      await page.locator(`.tree-row[data-agent-id=${JSON.stringify(agent.id)}]`).click();
      const pane = paneFor(page, agent);
      if (mode === 'Grid') await scrollColumn(pane);
      await showChat(pane);
      const chat = pane.locator(`${chatSelector}:visible`).first();
      const queen = chat.locator('[data-sender="queen"]');
      const operator = chat.locator('[data-sender="operator"]');
      await queen.waitFor(); await operator.waitFor();
      const body = chat.locator('.message p').first();
      const meta = queen.locator('.sender-label');
      await body.scrollIntoViewIfNeeded();
      const sample = locator => locator.evaluate(node => {
        const css = getComputedStyle(node);
        return { color: css.color, background: css.backgroundColor, font: css.fontSize, line: css.lineHeight,
          border: css.borderLeftColor, borderWidth: css.borderLeftWidth, radius: css.borderTopLeftRadius,
          outline: ['Top', 'Right', 'Bottom', 'Left'].every(side => css[`border${side}Width`] === '1px' && css[`border${side}Style`] === 'solid' && css[`border${side}Color`] === css.borderLeftColor),
          text: node.textContent, label: node.querySelector('.sender-label, strong')?.textContent };
      });
      const cases = [
        ['well', chat, css => css.background === 'rgb(7, 22, 40)', { 'background-color': 'rgb(244,247,251)' }],
        ['body', body, css => css.color === 'rgb(244, 247, 251)', { color: 'rgb(7,22,40)' }],
        ['meta', meta, css => css.color === 'rgb(201, 213, 228)', { color: 'rgb(7,22,40)' }],
        ['font', body, css => css.font === '13.5px' && Math.abs(parseFloat(css.line) - 20.925) < .01, { 'font-size': '12px' }],
        ['line', body, css => Math.abs(parseFloat(css.line) - 20.925) < .01, { 'line-height': '1' }],
        ['queen', queen, css => css.text.includes(manifest.chat.queenDirective) && css.background === 'rgba(193, 66, 196, 0.1)' && css.border === 'rgb(193, 66, 196)' && css.outline && css.label === 'Queen', { 'border-top-width': '0px' }],
        ['warn', chat.locator('.message[aria-label="Agent error"]'), css => css.background === 'rgba(236, 221, 165, 0.1)' && css.border === 'rgb(236, 221, 165)' && css.outline && css.label === 'Error', { 'border-top-width': '0px' }],
        ['operator', operator, css => css.text.includes(manifest.chat.operatorMessage) && css.background === 'rgb(7, 22, 40)' && css.borderWidth === '1px' && parseFloat(css.radius) > 0, { 'border-left-width': '0px' }],
      ];
      for (const [part, locator, accepts, mutation] of cases) {
        await locator.scrollIntoViewIfNeeded();
        const measure = async () => {
          const details = await sample(locator);
          return { passed: await locator.isVisible() && accepts(details), details };
        };
        const name = `chat-${part}-${mode.toLowerCase()}-${viewport.width}`;
        const result = await measure();
        assert(name, result.passed, result.details, { screen: mode.toLowerCase(), viewport: `${viewport.width}x${viewport.height}` });
        await prove(name, measure, () => styleChange(locator, mutation));
      }
      await queen.scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(out, `chat-${mode.toLowerCase()}-${viewport.width}.png`) });
    }
  }
}
