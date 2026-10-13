// Browser check of the Channel view (queue panel, drag to reorder, context menu, approvals,
// link field, Ctrl+V, file drop events, polling rules) against the fake engine, in headless
// Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/channel-view.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';
import { makeTestStream } from './hls-fixture.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8194);
if (!shots) throw new Error('usage: channel-view.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}

const stream = await makeTestStream(); // the preview's stream (404s would count as errors)
const engine = await startFakeEngine(port, { hlsDir: stream.dir });
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  viewport: { width: 1100, height: 680 },
  args: ['--disable-extensions', '--no-first-run'],
});
await ctx.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: base });

const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const requests = () => call('/__test/requests');
const scenario = (s) => call('/__test/scenario', s);
const reset = () => call('/__test/reset', {});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};

async function waitFor(fn, what, timeout = 5000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}
const actionsSince = async (mark) => (await requests()).slice(mark).filter((r) => r.method !== 'GET');
const mark = async () => (await requests()).length;
const lastAction = async (m, what) =>
  waitFor(async () => (await actionsSince(m)).at(-1), what);

async function openApp({ width = 1100, height = 680 } = {}) {
  const page = await ctx.newPage();
  await page.setViewportSize({ width, height });
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  // The refused links of the checks below are 400s on purpose.
  page.on('console', (m) => m.type() === 'error' && !m.text().includes('status of 400') && errors.push(m.text()));
  page.on('response', (r) => r.status() >= 400 && !r.url().includes('/api/') && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${base}/admin`);
  await page.waitForSelector('.player .play');
  await page.waitForSelector('.qi');
  page.__errors = errors;
  return page;
}

const ids = (page, sel = '.qi-queued') => page.$$eval(sel, (rows) => rows.map((r) => Number(r.dataset.id)));
const box = (page, id) =>
  page.$eval(`.qi[data-id="${id}"]`, (e) => {
    const r = e.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height };
  });
const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const setLanguage = (page, code) => page.evaluate(async (c) => (await import('/static/app/store.js')).setLanguage(c), code);

/** Presses on row `fromId`, moves to `y` (in steps) and, when `release`, lets go. */
async function dragRow(page, fromId, y, release = true) {
  const b = await box(page, fromId);
  const x = b.x + b.w * 0.5;
  await page.mouse.move(x, b.y + b.h / 2);
  await page.mouse.down();
  await page.mouse.move(x, b.y + b.h / 2 + 8, { steps: 2 });
  await page.mouse.move(x, y, { steps: 6 });
  if (release) await page.mouse.up();
}

try {
  await reset();
  let page = await openApp();
  await page.waitForTimeout(300);
  await page.screenshot({ path: join(shots, 'after-channel-en-1100x680.png') });

  // ---------- The panel ----------
  check(JSON.stringify(await ids(page)) === '[8,9,10,11]', 'the queue lists the four queued videos in order');
  check((await text(page, '.q-head')) === 'Up next4 videos · 1:49:54+', 'header: count and total length ("+" because a live stream has none)', await text(page, '.q-head'));
  check((await page.$eval('.q-count', (e) => e.title)) === 'Total length of the known ones', 'the "+" is explained in a tooltip');
  check((await text(page, '.qi[data-id="11"] .qi-name')) === 'Untitled', 'the stored default title "Bez tytułu" shows the localized "Untitled"');
  check((await text(page, '.qi[data-id="10"] .live')) === 'LIVE', 'a live stream carries the LIVE mark');
  check((await text(page, '.qi[data-id="8"] .qi-sub')) === '3:52 · Tomek', 'details: length · requester', await text(page, '.qi[data-id="8"] .qi-sub'));
  check((await text(page, '.qi[data-id="9"] .qi-sub')) === '1:42:10 · from disk', 'a file from disk says so', await text(page, '.qi[data-id="9"] .qi-sub'));
  check((await text(page, '.qi[data-id="11"] .qi-sub')) === '3:52 · anonymous', 'a requester without a nick is "anonymous"');
  check((await page.$('.approvals')) === null, 'no "Waiting for approval" section without pending videos');
  check((await page.$eval('.qi[data-id="9"] .qi-text svg', (e) => e.dataset.badge)) === 'host', 'the owner has the host mark');
  check((await page.$eval('.qi[data-id="10"] .qi-text svg', (e) => [e.dataset.badge, e.querySelector('path').getAttribute('fill')].join(' '))) === 'shape-3 hsl(40 70% 62%)', 'a viewer badge: shape 3 tinted by hue 40');
  check((await page.$('.qi[data-id="11"] .qi-text svg')) === null, 'no badge for an anonymous requester');
  const shapes = await page.evaluate(async () => {
    const { SHAPES, RequesterBadge } = await import('/static/app/components/badge.js');
    const { html } = await import('/static/app/html.js');
    const { render } = await import('/static/vendor/preact.mjs');
    const host = document.createElement('div');
    document.body.append(host);
    const out = [];
    for (let shape = 0; shape < 12; shape += 1) {
      render(html`<${RequesterBadge} badge=${{ hue: 120, shape }} />`, host);
      out.push(host.querySelector('path').getAttribute('d'));
    }
    render(html`<${RequesterBadge} badge=${{ hue: 120, shape: 12 }} />`, host);
    const outside = host.innerHTML;
    host.remove();
    return { count: SHAPES.length, distinct: new Set(out).size, outside };
  });
  check(shapes.count === 12 && shapes.distinct === 12, '12 distinct SVG shapes for shape 0..11', shapes);
  check(shapes.outside === '', 'a shape outside 0..11 draws nothing');

  // ---------- Drag to reorder ----------
  let m = await mark();
  const b10 = await box(page, 10);
  await dragRow(page, 8, b10.y + b10.h * 0.75, false);
  await page.screenshot({ path: join(shots, 'after-channel-drag.png') });
  check(await page.$eval('.qi[data-id="8"]', (e) => e.classList.contains('dragging')), 'the dragged row is marked');
  check(await page.$eval('.qi[data-id="11"]', (e) => e.classList.contains('drop-before')), 'a line shows where it will land (before the 4th row)');
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await page.waitForTimeout(200);
  check((await actionsSince(m)).length === 0 && !(await page.$('.dragging')), 'Esc cancels the drag: no request');
  await dragRow(page, 8, b10.y + b10.h * 0.75);
  let a = await lastAction(m, 'PATCH after the drop');
  check(a.method === 'PATCH' && a.path === '/api/admin/items/8' && JSON.stringify(a.body) === '{"position":2}', 'dropping row 1 below row 3 sends PATCH /items/8 {position: 2}', a);
  check(JSON.stringify(await ids(page)) === '[9,10,8,11]', 'the list shows the new order at once');
  await page.waitForTimeout(1300);
  check(JSON.stringify(await ids(page)) === '[9,10,8,11]', 'the order holds after the next poll');
  m = await mark();
  await dragRow(page, 11, (await box(page, 9)).y + 4);
  a = await lastAction(m, 'PATCH to the top');
  check(JSON.stringify(a.body) === '{"position":0}' && a.path === '/api/admin/items/11', 'dragging the last row above the first sends {position: 0}', a);
  m = await mark();
  await dragRow(page, 9, (await box(page, 9)).y + 20);
  await page.waitForTimeout(300);
  check((await actionsSince(m)).length === 0, 'dropping a row where it was sends nothing');
  m = await mark();
  await dragRow(page, 11, (await box(page, 8)).y + (await box(page, 8)).h + 60);
  a = await lastAction(m, 'PATCH to the end');
  check(JSON.stringify(a.body) === '{"position":3}', 'dropping below the last row sends {position: 3} (the end)', a);
  await waitFor(async () => JSON.stringify(await ids(page)) === '[9,10,8,11]', 'order after the moves', 4000);
  // The engine answers slowly: polls that started before it answers must not undo the move.
  await scenario({ patchDelayMs: 1800 });
  await dragRow(page, 9, (await box(page, 8)).y + (await box(page, 8)).h * 0.75);
  await page.waitForTimeout(1500);
  check(JSON.stringify(await ids(page)) === '[10,8,9,11]', 'while the engine is slow, polls do not put the old order back', await ids(page));
  await scenario({ patchDelayMs: 0 });
  await waitFor(async () => JSON.stringify(await ids(page)) === '[10,8,9,11]', 'engine order');
  await page.waitForTimeout(1500);
  check(JSON.stringify(await ids(page)) === '[10,8,9,11]', 'the engine order is the one shown afterwards');
  await reset();
  await page.reload();
  await page.waitForSelector('.qi');

  // ---------- Keyboard reorder ----------
  m = await mark();
  await page.focus('.qi[data-id="8"]');
  await page.keyboard.press('Alt+ArrowDown');
  a = await lastAction(m, 'Alt+Down');
  check(a.path === '/api/admin/items/8' && JSON.stringify(a.body) === '{"position":1}', 'Alt+↓ on the first row sends {position: 1}', a);
  await waitFor(async () => JSON.stringify(await ids(page)) === '[9,8,10,11]', 'moved');
  check(await page.evaluate(() => document.activeElement?.dataset.id === '8'), 'focus stays on the moved row');
  check((await text(page, '.q [role=status]')) === '“Darude — Sandstorm” moved to position 2 of 4', 'the move is announced to screen readers', await text(page, '.q [role=status]'));
  m = await mark();
  await page.keyboard.press('Alt+ArrowUp');
  await waitFor(async () => JSON.stringify(await ids(page)) === '[8,9,10,11]', 'moved back');
  m = await mark();
  await page.keyboard.press('Alt+ArrowUp');
  await page.waitForTimeout(300);
  check((await actionsSince(m)).length === 0, 'Alt+↑ on the first row does nothing');

  // ---------- Context menu ----------
  await reset();
  await page.reload();
  await page.waitForSelector('.qi');
  await page.click('.qi[data-id="8"]', { button: 'right' });
  await page.waitForSelector('.menu [role=menuitem]');
  const items = await page.$$eval('.menu [role=menuitem]', (els) => els.map((e) => [e.textContent.trim(), e.getAttribute('aria-disabled')]));
  check(JSON.stringify(items) === JSON.stringify([['Play now', 'false'], ['Move to the top', 'true'], ['Move up', 'true'], ['Move down', 'false'], ['Copy link', 'false'], ['Remove from the queue', 'false']]), 'right click opens the menu (the first row cannot go to the top or up)', items);
  check((await page.$eval('.menu', (e) => e.getAttribute('aria-label'))) === 'Video menu', 'the menu has an accessible name');
  await page.screenshot({ path: join(shots, 'after-channel-menu-en.png') });
  m = await mark();
  await page.keyboard.press('Enter');
  a = await lastAction(m, 'play-now');
  check(a.path.endsWith('/play-now') && JSON.stringify(a.body) === '{"itemId":8}', '"Play now" → POST …/play-now {itemId: 8}', a);
  // Menu key and Shift+F10
  await page.focus('.qi[data-id="10"]');
  await page.keyboard.press('ContextMenu');
  await page.waitForSelector('.menu');
  check((await page.$$('.menu [role=menuitem]')).length === 6, 'the Menu key opens the menu of the focused row');
  await page.keyboard.press('Escape');
  check(await page.evaluate(() => document.activeElement?.dataset.id === '10'), 'focus returns to the row when the menu closes');
  await page.keyboard.press('Shift+F10');
  await page.waitForSelector('.menu');
  await page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'menuitem');
  check(true, 'Shift+F10 opens the menu too');
  m = await mark();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter'); // Move to the top
  a = await lastAction(m, 'to top');
  check(a.path === '/api/admin/items/10' && JSON.stringify(a.body) === '{"position":0}', '"Move to the top" → PATCH {position: 0}', a);
  await waitFor(async () => JSON.stringify(await ids(page)) === '[10,8,9,11]', 'moved to the top');
  // Copy link: a web video has one; a file from disk does not.
  await page.click('.qi[data-id="9"]', { button: 'right' });
  await page.waitForSelector('.menu');
  check((await page.$$eval('.menu [role=menuitem]', (els) => els[4].getAttribute('aria-disabled'))) === 'true', 'a file from disk has no link to copy');
  await page.keyboard.press('Escape');
  await page.click('.qi[data-id="8"]', { button: 'right' });
  await page.click('.menu [role=menuitem]:nth-child(5)');
  const clip = await page.evaluate(() => navigator.clipboard.readText());
  check(clip === 'https://www.youtube.com/watch?v=8', '"Copy link" puts the video address on the clipboard', clip);
  // Task 15: one place down / up from the menu (touch screens cannot drag).
  m = await mark();
  await page.click('.qi[data-id="8"]', { button: 'right' });
  await page.click('.menu [role=menuitem]:nth-child(4)');
  a = await lastAction(m, 'move down');
  check(a.path === '/api/admin/items/8' && JSON.stringify(a.body) === '{"position":2}', '"Move down" → PATCH {position: index + 1}', a);
  await waitFor(async () => JSON.stringify(await ids(page)) === '[10,9,8,11]', 'moved down');
  m = await mark();
  await page.click('.qi[data-id="9"]', { button: 'right' });
  await page.click('.menu [role=menuitem]:nth-child(3)');
  a = await lastAction(m, 'move up');
  check(a.path === '/api/admin/items/9' && JSON.stringify(a.body) === '{"position":0}', '"Move up" → PATCH {position: index − 1}', a);
  await waitFor(async () => JSON.stringify(await ids(page)) === '[9,10,8,11]', 'moved up');
  await page.click('.qi[data-id="11"]', { button: 'right' });
  check((await page.$$eval('.menu [role=menuitem]', (els) => els[3].getAttribute('aria-disabled'))) === 'true', 'the last row cannot go down');
  await page.keyboard.press('Escape');
  m = await mark();
  await page.click('.qi[data-id="11"]', { button: 'right' });
  await page.click('.menu .danger');
  a = await lastAction(m, 'delete');
  check(a.method === 'DELETE' && a.path === '/api/admin/items/11', '"Remove from the queue" → DELETE /items/11', a);
  await waitFor(async () => JSON.stringify(await ids(page)) === '[9,10,8]', 'removed');
  check(true, 'the row disappears');

  // ---------- Approvals ----------
  await reset();
  await scenario({ pending: 2 });
  await page.reload();
  await page.waitForSelector('.approvals .qi');
  check((await text(page, '.approvals .sec')) === 'Waiting for approval', 'the "Waiting for approval" section appears with moderation');
  check((await page.$$('.approvals .qi')).length === 2, 'two pending videos');
  check((await text(page, '.approvals .qi[data-id="20"] .qi-sub')) === '3:52 · requested by Bartek', 'who sent it', await text(page, '.approvals .qi[data-id="20"] .qi-sub'));
  check((await text(page, '.approvals .qi[data-id="21"] .qi-sub')) === '3:52 · requested anonymously', 'a requester without a nick');
  check((await page.$eval('.approvals .qi[data-id="20"] .act-button.yes', (e) => e.getAttribute('aria-label'))) === 'Approve “Nyan Cat 10 godzin”', 'the button names the video for screen readers');
  await page.screenshot({ path: join(shots, 'after-channel-pending-en.png') });
  m = await mark();
  await page.click('.approvals .qi[data-id="20"] .yes');
  a = await lastAction(m, 'approve');
  check(a.method === 'PATCH' && a.path === '/api/admin/items/20' && JSON.stringify(a.body) === '{"action":"approve"}', '✓ → PATCH /items/20 {action: "approve"}', a);
  await waitFor(async () => JSON.stringify(await ids(page)) === '[8,9,10,11,20]', 'approved video joins the queue');
  check((await page.$$('.approvals .qi')).length === 1, 'the approved video leaves the pending section');
  m = await mark();
  await page.click('.approvals .qi[data-id="21"] .no');
  a = await lastAction(m, 'reject');
  check(JSON.stringify(a.body) === '{"action":"reject"}' && a.path === '/api/admin/items/21', '✕ → PATCH /items/21 {action: "reject"}', a);
  await waitFor(async () => (await page.$('.approvals')) === null, 'section gone');
  check((await page.$('.rail-channel .badge')) === null, 'the badge on the rail disappears with the last pending video');
  check(JSON.stringify(await ids(page)) === '[8,9,10,11,20]', 'a rejected video does not enter the queue');

  // ---------- Adding a link ----------
  await reset();
  await page.reload();
  await page.waitForSelector('.qi');
  m = await mark();
  await page.fill('.addf-input', 'https://example.com/watch?v=1');
  await page.keyboard.press('Enter');
  a = await lastAction(m, 'add link');
  check(a.method === 'POST' && a.path === '/api/admin/channels/main/items' && JSON.stringify(a.body) === '{"url":"https://example.com/watch?v=1"}', 'Enter → POST …/items {url}', a);
  await waitFor(async () => (await ids(page)).length === 5, 'the new video appears in the queue');
  check((await page.$eval('.addf-input', (e) => e.value)) === '', 'the field is emptied');
  check((await text(page, '.toast span')) === 'Added: Added from example.com', 'a message says what was added');
  await page.fill('.addf-input', 'not a link');
  await page.keyboard.press('Enter');
  await page.waitForSelector('.addf-error');
  check((await text(page, '.addf-error')) === 'That does not look like a valid link (http/https).', 'an invalid link: the dictionary text of errors.invalid_link', await text(page, '.addf-error'));
  check((await page.$eval('.addf-input', (e) => e.getAttribute('aria-invalid'))) === 'true', 'the field is marked invalid');
  await page.fill('.addf-input', 'https://vimeo.com/1');
  check((await page.$('.addf-error')) === null, 'typing clears the error');
  await setLanguage(page, 'pl');
  await scenario({ addError: 'domain_not_allowed' });
  await page.keyboard.press('Enter');
  await page.waitForSelector('.addf-error');
  check((await text(page, '.addf-error')) === 'Ten serwis nie jest dozwolony. Dozwolone: youtube.com.', 'domain_not_allowed is shown in Polish (the engine fills in the domains)', await text(page, '.addf-error'));
  await scenario({ addError: 'local_address' });
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => document.querySelector('.addf-error')?.textContent.startsWith('Linki do sieci lokalnej'));
  check(true, 'local_address is shown from the Polish dictionary');
  await setLanguage(page, 'en');
  await page.waitForFunction(() => document.querySelector('.addf-error')?.textContent.startsWith('Links to a local network'));
  check(true, 'an error on screen follows a language change');
  await scenario({ addError: null });
  await page.fill('.addf-input', '');
  check(await page.$eval('.addf-button', (b) => b.disabled), 'the Add button is off for an empty field');

  // ---------- Ctrl+V ----------
  await page.click('#view .stage');
  m = await mark();
  await page.evaluate(() => navigator.clipboard.writeText('https://youtu.be/abc123'));
  await page.keyboard.press('Control+V');
  a = await lastAction(m, 'paste');
  check(a.method === 'POST' && JSON.stringify(a.body) === '{"url":"https://youtu.be/abc123"}', 'Ctrl+V on the window adds the link on the clipboard', a);
  await page.waitForFunction(() => document.querySelector('.toast span')?.textContent.startsWith('Added: '));
  check(true, 'Ctrl+V: a message says it was added');
  m = await mark();
  await page.evaluate(() => navigator.clipboard.writeText('just some words'));
  await page.keyboard.press('Control+V');
  await page.waitForFunction(() => document.querySelector('.toast span')?.textContent.startsWith('The clipboard does not hold a link'));
  check((await actionsSince(m)).length === 0, 'Ctrl+V with text that is not a link: a message, no request');
  await page.click('.addf-input');
  await page.evaluate(() => navigator.clipboard.writeText('https://youtu.be/typed'));
  await page.keyboard.press('Control+V');
  await page.waitForTimeout(300);
  check((await actionsSince(m)).length === 0 && (await page.$eval('.addf-input', (e) => e.value)) === 'https://youtu.be/typed', 'Ctrl+V in the link field only pastes (Enter adds)');
  await page.fill('.addf-input', '');
  await page.click('.rail-history');
  m = await mark();
  await page.evaluate(() => navigator.clipboard.writeText('https://youtu.be/other-view'));
  await page.click('#view');
  await page.keyboard.press('Control+V');
  a = await lastAction(m, 'paste elsewhere');
  check(a.method === 'POST', 'Ctrl+V works in the other views too');
  await page.click('.rail-channel');

  // ---------- Files dropped on the window ----------
  const fire = (name, detail) => page.evaluate(([n, d]) => window.dispatchEvent(new CustomEvent(n, { detail: d })), [name, detail]);
  check((await page.$('.drop-overlay')) === null, 'no drop overlay at rest');
  await fire('vrcinema:drag-enter');
  await page.waitForSelector('.drop-overlay');
  check((await text(page, '.drop-overlay')) === 'Drop to add to the queue', 'files over the window show the drop target');
  await page.screenshot({ path: join(shots, 'after-channel-drop.png') });
  await fire('vrcinema:drag-leave');
  await page.waitForFunction(() => !document.querySelector('.drop-overlay'));
  check(true, 'it goes away when the files leave');
  await fire('vrcinema:drag-enter');
  m = await mark();
  await fire('vrcinema:files-added', { added: 2, skipped: 1, error: null });
  await page.waitForFunction(() => document.querySelector('.toast span')?.textContent.startsWith('Added 2 videos.'));
  check((await text(page, '.toast span')) === 'Added 2 videos. Skipped 1 file (it is not a video or cannot be read).', 'a drop reports what was added and skipped', await text(page, '.toast span'));
  check((await page.$('.drop-overlay')) === null, 'the overlay goes away after the drop');
  await fire('vrcinema:files-added', { added: 0, skipped: 0, error: 'Serwer jest zajęty' });
  await page.waitForFunction(() => document.querySelector('.toast span')?.textContent === 'Serwer jest zajęty');
  check(true, 'a failed drop shows the window\'s message');
  await fire('vrcinema:files-added', { added: 0, skipped: 2, error: null });
  await page.waitForFunction(() => document.querySelector('.toast span')?.textContent.startsWith('Skipped 2 files'));
  check(true, 'only skipped files: no "added" part');
  await setLanguage(page, 'pl');
  await fire('vrcinema:files-added', { added: 5, skipped: 0, error: null });
  await page.waitForFunction(() => document.querySelector('.toast span')?.textContent === 'Dodano 5 filmów.');
  check(true, 'Polish plural: "Dodano 5 filmów."');
  await fire('vrcinema:files-added', { added: 3, skipped: 2, error: null });
  await page.waitForFunction(() => document.querySelector('.toast span')?.textContent.startsWith('Dodano 3 filmy. Pominięto 2 pliki'));
  check(true, 'Polish plural: "Dodano 3 filmy. Pominięto 2 pliki …"');
  await setLanguage(page, 'en');

  // ---------- Languages: de (the longest) and pl, nothing cut ----------
  await reset();
  await scenario({ pending: 2 });
  await page.reload();
  await page.waitForSelector('.approvals .qi');
  await setLanguage(page, 'de');
  await page.waitForFunction(() => document.querySelector('.rail-channel .rail-label')?.textContent === 'Kanal');
  for (const [w, h] of [[1100, 680], [960, 600]]) {
    await page.setViewportSize({ width: w, height: h });
    await page.waitForTimeout(200);
    const cut = await page.evaluate(() =>
      [...document.querySelectorAll('.q button, .q-head span, .sec, .addf-input, .qi-sub')]
        .filter((e) => e.scrollWidth > e.clientWidth + 1 && !e.matches('.qi-sub'))
        .map((e) => `${e.className}: ${e.textContent.trim()} ${e.scrollWidth}>${e.clientWidth}`),
    );
    check(cut.length === 0, `de ${w}×${h}: no button, heading or field of the queue panel is cut`, cut);
    const keysLeft = await page.evaluate(() => document.body.innerText.match(/\b(queue|drop|nav|player|ui)\.[a-z_]+/g));
    check(keysLeft === null, `de ${w}×${h}: no untranslated key on screen`, keysLeft);
    check(!(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)), `de ${w}×${h}: no horizontal scrolling`);
  }
  await page.screenshot({ path: join(shots, 'after-channel-de-960x600.png') });
  await page.setViewportSize({ width: 1100, height: 680 });
  check((await text(page, '.approvals .qi[data-id="20"] .qi-sub')).includes('Bartek'), 'de: pending row names the requester');
  await page.screenshot({ path: join(shots, 'after-channel-de-1100x680.png') });
  await setLanguage(page, 'pl');
  await page.waitForFunction(() => document.querySelector('#queue-title')?.textContent === 'Następne');
  check((await text(page, '.q-head')).startsWith('Następne4 filmy · '), 'pl: header with the plural "4 filmy"', await text(page, '.q-head'));
  check((await text(page, '.qi[data-id="11"] .qi-name')) === 'Bez tytułu', 'pl: the untitled placeholder');
  await page.screenshot({ path: join(shots, 'after-channel-pl-1100x680.png') });
  await page.setViewportSize({ width: 640, height: 800 });
  await page.waitForTimeout(200);
  check(await page.evaluate(() => document.querySelector('.q').getBoundingClientRect().width > 500), 'below 700 px the queue panel takes the full width');
  await page.screenshot({ path: join(shots, 'after-channel-narrow-640.png') });
  await page.setViewportSize({ width: 1100, height: 680 });
  await setLanguage(page, 'en');
  check(page.__errors.length === 0, 'no errors in the console', page.__errors);
  await page.close();

  // ---------- Keys follow the buttons (canSeek / canPause / current) ----------
  await reset();
  await scenario({ playing: 'live' });
  page = await openApp();
  await page.click('#view .stage');
  m = await mark();
  for (const key of ['Space', 'ArrowRight', 'Shift+ArrowLeft', 'Home']) {
    await page.keyboard.press(key);
    await page.waitForTimeout(60);
  }
  await page.waitForTimeout(250);
  check((await actionsSince(m)).length === 0, 'live stream: Space, →, Shift+←, Home do nothing (the buttons are greyed out)');
  await page.keyboard.press('n');
  a = await lastAction(m, 'N on a live stream');
  check(a.path.endsWith('/skip'), 'live stream: N still skips');
  await scenario({ playing: 'film', canSeek: false });
  await page.waitForFunction(() => !document.querySelector('.ctl .play').disabled);
  m = await mark();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Home');
  await page.waitForTimeout(250);
  check((await actionsSince(m)).length === 0, 'canSeek false: → and Home do nothing');
  await page.keyboard.press('Space');
  a = await lastAction(m, 'Space with canPause');
  check(a.path.endsWith('/pause'), 'canSeek false but canPause: Space pauses');
  await scenario({ playing: 'none' });
  await page.waitForFunction(() => document.querySelector('.nowp-title')?.textContent === 'Nothing is playing');
  m = await mark();
  for (const key of ['Space', 'n', 'ArrowRight']) await page.keyboard.press(key);
  await page.waitForTimeout(250);
  check((await actionsSince(m)).length === 0, 'nothing playing: Space, N and → do nothing');
  await page.close();

  // ---------- Polling: one read at a time, backoff, stop on 401, timeouts ----------
  await reset();
  await scenario({ panelDelayMs: 350 });
  page = await openApp();
  await page.evaluate(async () => {
    const store = await import('/static/app/store.js');
    for (let i = 0; i < 6; i += 1) {
      store.refreshPanel();
      await new Promise((r) => setTimeout(r, 120));
      store.seekBy(10);
    }
  });
  await page.waitForTimeout(1800);
  const stats = await call('/__test/stats');
  check(stats.maxPanelInFlight === 1, 'panel reads never overlap (polling and the refresh after actions share one)', stats);
  await scenario({ panelDelayMs: 0 });
  await page.close();

  await reset();
  page = await openApp();
  await scenario({ panelFail: true });
  await sleep(1300); // the poll in flight
  m = await mark();
  await sleep(7000);
  const failed = (await requests()).slice(m).filter((r) => r.method === 'GET' && r.path === '/api/admin/channels/main').length;
  check(failed >= 1 && failed <= 4, 'after failed reads the polling backs off (≤ 4 reads in 7 s instead of 7)', failed);
  await scenario({ panelFail: false });
  m = await mark();
  await waitFor(async () => (await requests()).slice(m).some((r) => r.path === '/api/admin/channels/main'), 'a read after recovery', 12000);
  check(true, 'polling goes on after the engine recovers');
  await page.close();

  await reset();
  page = await openApp();
  await scenario({ signedOut: true });
  await page.waitForSelector('.signed-out', { timeout: 6000 });
  check(true, 'a session that ends shows the sign-in screen');
  await sleep(3600); // the info poll that was already waiting gets its 401 too
  m = await mark();
  await sleep(4500);
  const after401 = (await requests()).slice(m).filter((r) => r.method === 'GET').length;
  check(after401 === 0, 'after a 401 the polling stops (no read in 4.5 s)', after401);
  await scenario({ signedOut: false });
  await page.evaluate(async () => (await import('/static/app/store.js')).resumePolling());
  await page.waitForSelector('.qi', { timeout: 6000 });
  check(true, 'resumePolling() takes polling up again after signing in');
  await page.close();

  page = await ctx.newPage();
  await page.goto(`${base}/admin`);
  await page.waitForSelector('.player .play');
  const timed = await page.evaluate(async () => {
    const { api } = await import('/static/app/api.js');
    const started = performance.now();
    try {
      await api('GET', '/__test/hang', undefined, { timeoutMs: 300 });
      return { code: 'answered' };
    } catch (e) {
      return { code: e.code, ms: Math.round(performance.now() - started) };
    }
  });
  check(timed.code === 'network' && timed.ms >= 250 && timed.ms < 2000, 'a request that gets no answer fails after its timeout (code "network")', timed);
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
  stream.cleanup();
}
