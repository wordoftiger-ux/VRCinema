// Browser check of viewer lists: the owner's switch and list size (Settings → Channel), the
// request page's multi-line field (one link per line, playlists), the "Checking N links…"
// state, the result with the left-out links and their reasons, and a list waiting for approval
// as one group in the Channel view (✓ all, ✕ per video) — in headless Brave against the fake
// engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/viewer-lists.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: viewer-lists.mjs <shots dir> [port]');
if (port === 8080) throw new Error('never port 8080');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}

const engine = await startFakeEngine(port);
const base = `http://127.0.0.1:${port}`;
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const scenario = (s) => call('/__test/scenario', s);
const requests = () => call('/__test/requests');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};

async function waitFor(fn, what, timeout = 6000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

const profile = mkdtempSync(join(tmpdir(), 'brave-lists-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1100, height: 760 },
  args: ['--disable-extensions', '--no-first-run'],
});

function watch(page) {
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 4\d\d/.test(m.text()) && errors.push(m.text()));
  return errors;
}

try {
  // ---------- Settings → Channel: the switch and the list size ----------
  const app = await ctx.newPage();
  await app.addInitScript(FAKE_TAURI);
  const appErrors = watch(app);
  await app.goto(`${base}/admin#/settings/channel`);
  await app.waitForSelector('#view[data-section="channel"]');
  const sw = 'button[role=switch][aria-label="Allow viewers to add lists"]';
  await app.waitForSelector(sw);
  check((await app.$eval(sw, (b) => b.getAttribute('aria-checked'))) === 'false', 'lists are off by default');
  check(await app.$eval('input[name=viewerListMax]', (i) => i.disabled), 'the list size waits for the switch');
  let mark = (await requests()).length;
  await app.click(sw);
  await waitFor(async () => (await requests()).slice(mark).some((r) => r.method === 'PUT' && JSON.stringify(r.body) === '{"viewerLists":true}'), 'the switch saves {viewerLists: true}');
  await app.waitForFunction(() => !document.querySelector('input[name=viewerListMax]').disabled);
  await app.fill('input[name=viewerListMax]', '60');
  await app.evaluate(() => {
    const form = document.querySelector('form[data-form="viewer-lists"]');
    form.noValidate = true; // the browser's own maximum would stop it first
    form.requestSubmit();
  });
  await app.waitForFunction(() => [...document.querySelectorAll('.set-note.err')].some((n) => n.textContent.includes('1 to 50')));
  check(true, 'a size above 50 is refused on the page');
  mark = (await requests()).length;
  await app.fill('input[name=viewerListMax]', '3');
  await app.click('form[data-form="viewer-lists"] button[type=submit]');
  await waitFor(async () => (await requests()).slice(mark).some((r) => r.method === 'PUT' && JSON.stringify(r.body) === '{"viewerListMax":3}'), 'the size saves {viewerListMax: 3}');
  await app.screenshot({ path: join(shots, 'settings-lists.png') });

  // ---------- The request page: several links, a playlist ----------
  const page = await ctx.newPage();
  const errors = watch(page);
  await page.goto(`${base}/`);
  await page.waitForSelector('textarea[name=urls]');
  check((await page.locator('input[name=url]').count()) === 0, 'the single link field gives way to the list field');
  check((await page.locator('#v-links-hint').innerText()).includes('Up to 3 videos'), 'the hint names the list size', await page.locator('#v-links-hint').innerText());
  await scenario({ listDelayMs: 900 });
  await page.fill('textarea[name=urls]', [
    'https://youtube.com/playlist?list=PL1',
    '',
    'https://youtu.be/a',
    'http://192.168.0.5/x',
    'https://youtu.be/a',
    'not a link',
  ].join('\n'));
  await page.fill('input[name=nick]', 'Ola');
  mark = (await requests()).length;
  await page.click('.v-submit');
  await waitFor(async () => (await page.locator('.v-submit').innerText()) === 'Checking 5 links…', 'the button counts the links being checked');
  await page.waitForSelector('.v-list-result');
  const sent = (await requests()).slice(mark).find((r) => r.path === '/api/channels/main/request-list');
  check(JSON.stringify(sent?.body?.urls) === JSON.stringify(['https://youtube.com/playlist?list=PL1', 'https://youtu.be/a', 'http://192.168.0.5/x', 'https://youtu.be/a', 'not a link']), 'blank lines are dropped, the rest sent as urls', sent?.body);
  check(typeof sent.body.deviceKey === 'string' && sent.body.name === 'Ola', 'the nick goes with a device key', sent.body);
  const result = page.locator('.v-list-result');
  check((await result.getAttribute('data-added')) === '3', 'three videos added (the playlist fills the list)');
  const summary = await result.locator('p').innerText();
  check(summary.includes('3 videos added') && summary.includes('4 skipped'), 'the counts', summary);
  const left = await result.locator('li').allInnerTexts();
  check(left.length === 4, 'every left-out row is listed', left);
  check(left.some((l) => l.includes('Line 3') && l.includes('local network')), 'a link without a title is named by its line, with the reason', left);
  check(left.some((l) => l.includes('already in your list')), 'the duplicate is explained', left);
  check(await result.locator('li[data-code="list_full"]').count() === 1, 'the video beyond the list size is left out as list_full');
  check((await page.inputValue('textarea[name=urls]')) === '', 'the field is cleared after something was added');
  await page.screenshot({ path: join(shots, 'viewer-result.png'), fullPage: true });

  // Phone width: no horizontal scroll.
  await page.setViewportSize({ width: 320, height: 800 });
  check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1), 'no horizontal scroll at 320 px');
  await page.screenshot({ path: join(shots, 'viewer-result-320.png'), fullPage: true });
  await page.setViewportSize({ width: 520, height: 900 });

  // ---------- With approval: one group in the Channel view ----------
  await scenario({ listDelayMs: 0, settingsPatch: { moderation: true } });
  await page.waitForFunction(() => document.querySelector('.v-rules')?.innerText.includes('approves'));
  await page.fill('textarea[name=urls]', 'https://youtube.com/playlist?list=PL2');
  await page.click('.v-submit');
  await page.waitForSelector('.v-list-result');
  check((await page.locator('.v-list-result p').innerText()).includes('3 videos are waiting for approval'), 'the page says the list waits for approval');
  check(errors.length === 0, 'no errors on the request page', errors);

  await app.goto(`${base}/admin`);
  await app.waitForSelector('.qi-list');
  const group = app.locator('.qi-list');
  check((await group.locator('.qi-list-head .qi-title').innerText()) === 'List: 3 videos', 'the list is one group with its count');
  check((await group.locator('.qi-list-head .qi-sub').innerText()).includes('Fake playlist'), 'the group names the playlist');
  check((await group.locator('.qi-pending').count()) === 3, 'its videos are listed under it');
  await app.screenshot({ path: join(shots, 'approvals-group.png') });
  // One video refused on its own, the rest approved together.
  const firstId = await group.locator('.qi-pending').first().getAttribute('data-id');
  mark = (await requests()).length;
  await group.locator('.qi-pending').first().locator('.act-button.no').click();
  await waitFor(async () => (await requests()).slice(mark).some((r) => r.method === 'PATCH' && r.path === `/api/admin/items/${firstId}`), 'a video of the list is rejected on its own');
  await waitFor(async () => (await app.locator('.qi-list .qi-pending').count()) === 2, 'two left in the group');
  const listId = await app.locator('.qi-list').getAttribute('data-list');
  mark = (await requests()).length;
  await app.locator('.qi-list-head .act-button.yes').click();
  await waitFor(async () => (await requests()).slice(mark).some((r) => r.method === 'POST' && r.path === `/api/admin/channels/main/lists/${listId}` && r.body?.action === 'approve'), '✓ all → POST lists/{id} {action: approve}');
  await waitFor(async () => (await app.locator('.approvals').count()) === 0, 'nothing waits any more');
  check(appErrors.length === 0, 'no errors in the panel', appErrors);

  console.log(`\n${passed} checks passed`);
} catch (e) {
  console.error(e.message);
  process.exitCode = 1;
} finally {
  await ctx.close();
  engine.close?.();
  rmSync(profile, { recursive: true, force: true });
  process.exit(process.exitCode ?? 0);
}
