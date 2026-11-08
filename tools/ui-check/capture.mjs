// Browser check of the live capture (spec 2026-11-08-capture-design): the Capture button under
// the link field (window only), the dialog (kinds, sources with Refresh, region fields, fast
// capture for one screen, sound, frame rate, resolution, title, preview frame), the request it
// sends, the red Capturing pill with Stop in the title bar, Polish texts, and a remote panel
// that never sees the button nor asks the capture routes — against the fake engine in headless
// Brave.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/capture.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8197);
if (!shots) throw new Error('usage: capture.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` - ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}

const engine = await startFakeEngine(port);
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1200, height: 800 },
  args: ['--disable-extensions', '--no-first-run'],
});

const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const requests = () => call('/__test/requests');
const scenario = (s) => call('/__test/scenario', s);
const reset = () => call('/__test/reset', {});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};
const RAW = /\b(capture|queue|ui|errors)\.[a-z_]+/;

async function waitFor(fn, what, timeout = 6000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}
const mark = async () => (await requests()).length;
const since = async (m) => (await requests()).slice(m);

async function openApp(hash) {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 40[34]/.test(m.text()) && errors.push(m.text()));
  await page.goto(`${base}/admin${hash}`);
  await page.waitForSelector('.player .play');
  page.__errors = errors;
  return page;
}
const shot = (page, name) => page.screenshot({ path: join(shots, name) });
const select = (page, name, value) => page.selectOption(`.dialog select[data-cap="${name}"]`, String(value));

try {
  await reset();
  let page = await openApp('#/channel');
  await page.waitForSelector('[data-capture-open]');
  check((await page.$eval('[data-capture-open]', (b) => b.textContent.trim())) === 'Capture', 'the window has a Capture button under the link field');
  check((await page.$('[data-capture-on]')) === null, 'no Capturing pill while nothing is captured');

  // ---------- The dialog ----------
  let m = await mark();
  await page.click('[data-capture-open]');
  await page.waitForSelector('.dialog [data-capture] select[data-cap="source"]');
  check((await since(m)).some((r) => r.path === '/api/desktop/capture/sources'), 'opening it lists what can be captured');
  check((await page.$eval('#dialog-title', (e) => e.textContent)) === 'Live capture of this computer', 'the dialog has its title');
  const sources = await page.$$eval('.dialog select[data-cap="source"] option', (o) => o.map((x) => x.textContent));
  check(JSON.stringify(sources) === JSON.stringify(['All screens', 'Screen 1 (2560×1440, main)', 'Screen 2 (1920×1080)', 'Part of the desktop']), 'screens: all, each one, a part', sources);
  check((await page.$('.dialog [data-cap="fast"]')) === null, 'no fast capture for all screens');
  await select(page, 'source', 'screen:1');
  await page.waitForSelector('.dialog [data-cap="fast"]');
  check(true, 'one screen offers fast capture (ddagrab)');
  await select(page, 'source', 'region');
  await page.waitForSelector('.dialog [data-cap-region="width"]');
  check((await page.$$('.dialog [data-cap-region]')).length === 4, 'a part of the desktop has X, Y, width and height');
  check((await page.$eval('.dialog', (e) => e.innerText)).includes('Stereo Mix'), 'the Windows note on desktop sound is shown');

  await select(page, 'kind', 'window');
  await page.waitForFunction(() => document.querySelector('.dialog select[data-cap="source"] option')?.textContent === 'Notatnik — szkic.txt');
  check((await page.$eval('.dialog', (e) => e.innerText)).includes('found by its title'), 'a window says it is found by its title');
  await select(page, 'audio', '@device_cm_mic');
  await select(page, 'fps', 25);
  await select(page, 'height', 720);
  await page.fill('.dialog [data-cap="title"]', 'Mój ekran');

  m = await mark();
  await page.click('.dialog [data-cap-preview]');
  await page.waitForSelector('.dialog img[data-cap-frame]');
  const asked = (await since(m)).find((r) => r.path === '/api/desktop/capture/preview');
  check(asked?.body?.kind === 'window' && asked.body.source === 'Notatnik — szkic.txt', 'Preview asks for one frame of the chosen window', asked?.body);
  await shot(page, 'capture-dialog-en.png');
  check(!RAW.test(await page.$eval('.dialog', (e) => e.innerText)), 'no raw keys in the dialog');

  m = await mark();
  await page.click('.dialog-foot .btn.pri');
  await waitFor(async () => (await since(m)).some((r) => r.path === '/api/desktop/capture' && r.method === 'POST'), 'Add to queue sends the capture');
  const add = (await since(m)).find((r) => r.path === '/api/desktop/capture');
  check(
    JSON.stringify(add.body) === JSON.stringify({ kind: 'window', source: 'Notatnik — szkic.txt', audio: '@device_cm_mic', fps: 25, height: 720, title: 'Mój ekran' }),
    'it sends kind, source, sound, frame rate, resolution and the title',
    add.body,
  );
  await page.waitForFunction(() => [...document.querySelectorAll('.qlist .qi-name')].some((e) => e.textContent === 'Mój ekran'));
  check(true, 'the capture appears in the queue under the owner\'s title');

  // ---------- Capturing pill and Stop ----------
  const id = (await call('/api/admin/channels/main')).queue.find((i) => i.title === 'Mój ekran').id;
  await scenario({ captureRunning: [id] });
  await page.waitForSelector('[data-capture-on]', { timeout: 8000 });
  check((await page.$eval('[data-capture-on]', (e) => e.innerText)).includes('Capturing'), 'the title bar shows Capturing while a capture runs');
  await shot(page, 'capture-on-en.png');
  m = await mark();
  await page.click('[data-capture-stop]');
  await waitFor(async () => (await since(m)).some((r) => r.path === '/api/desktop/capture/stop'), 'Stop asks the engine');
  await page.waitForFunction(() => !document.querySelector('[data-capture-on]'));
  check(true, 'the pill goes away at once');
  await page.waitForFunction(() => ![...document.querySelectorAll('.qlist .qi-name')].some((e) => e.textContent === 'Mój ekran'));
  check(true, 'the waiting capture left the queue');

  // ---------- Polish ----------
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await page.waitForFunction(() => document.querySelector('[data-capture-open]')?.textContent.trim() === 'Przechwyć');
  await page.click('[data-capture-open]');
  await page.waitForSelector('.dialog select[data-cap="source"]');
  const pl = await page.$eval('.dialog', (e) => e.innerText);
  check(pl.includes('Przechwytywanie z tego komputera') && !RAW.test(pl), 'Polish: the dialog is translated', pl.slice(0, 200));
  await shot(page, 'capture-dialog-pl.png');
  await page.keyboard.press('Escape');
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('en'));
  check(page.__errors.length === 0, 'no console errors', page.__errors);
  await page.close();

  // ---------- A remote panel ----------
  await reset();
  await scenario({ remote: true });
  page = await openApp('#/channel');
  await sleep(1500);
  check((await page.$('[data-capture-open]')) === null, 'a remote panel has no Capture button');
  const remoteAsks = (await requests()).filter((r) => r.path.includes('/capture'));
  check(remoteAsks.length === 0, 'and never asks the capture routes', remoteAsks);
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
}
