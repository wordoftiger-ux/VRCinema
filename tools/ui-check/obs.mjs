// Browser check of OBS as a source (spec 2026-11-06-obs-design): Settings → OBS (server and
// stream key with Copy, the key hidden until shown, a new key, the two switches saving
// obsLan/obsPriority, the LAN address once allowed, a remote panel never getting the key) and
// the window's OBS states (player bar "OBS live" with Skip disabled, the queue's note, the
// Preview's note, "Be right back"), against the fake engine in headless Brave.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/obs.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: obs.mjs <shots dir> [port]');
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
  viewport: { width: 1200, height: 760 },
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
const actionsSince = async (m) => (await requests()).slice(m).filter((r) => r.method !== 'GET');

async function openApp(hash) {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  // The fake engine has no stream for the On air screen (404): not this check's concern.
  page.on('console', (m) => m.type() === 'error' && !/status of 40[34]/.test(m.text()) && errors.push(m.text()));
  await page.goto(`${base}/admin${hash}`);
  await page.waitForSelector('.player .play');
  page.__errors = errors;
  return page;
}
const sw = (label) => `button[role=switch][aria-label="${label}"]`;
const shot = (page, name) => page.screenshot({ path: join(shots, name) });
const RAW = /\b(settings|player|queue|monitor|ui)\.[a-z_]+\.?[a-z_]*/;

try {
  await reset();
  let page = await openApp('#/settings/obs');
  await page.waitForSelector('#view[data-section="obs"] [data-obs-value="server"]');

  // ---------- Settings → OBS ----------
  check((await page.$eval('.sform h1', (e) => e.textContent)) === 'OBS', 'the section is called OBS');
  check((await page.$eval('[data-obs-value="server"]', (e) => e.textContent)) === 'rtmp://127.0.0.1:1935/live', 'the server for this computer');
  const hidden = await page.$eval('[data-obs-value="key"]', (e) => e.textContent);
  check(/^•+$/.test(hidden), 'the key is hidden at first', hidden);
  await page.click('#view button:has-text("Show")');
  check((await page.$eval('[data-obs-value="key"]', (e) => e.textContent)) === 'abcdefghijklmnopqrstuvwx', 'Show reveals the key');
  await page.click('#view .obs-copy:has([data-obs-value="key"]) [data-copy]');
  await waitFor(async () => (await page.evaluate(() => navigator.clipboard.readText())) === 'abcdefghijklmnopqrstuvwx', 'Copy puts the key on the clipboard');
  check(true, 'Copy puts the key on the clipboard');
  check((await page.$eval('#view .obs-copy:has([data-obs-value="key"]) [data-copy]', (b) => b.textContent.trim())) === 'Copied', 'and says so');
  check((await page.$$('[data-obs-value="server"]')).length === 1, 'no LAN address while only this computer is allowed');
  check(await page.$eval(sw('OBS has priority over the queue'), (b) => b.getAttribute('aria-checked') === 'true'), 'priority is on by default');
  check(await page.$eval(sw('Allow OBS from other devices on the network'), (b) => b.getAttribute('aria-checked') === 'false'), 'other devices are off by default');
  check((await page.$eval('#view', (e) => e.innerText)).includes('firewall'), 'the hint names the Windows firewall');

  let m = await mark();
  await page.click(sw('Allow OBS from other devices on the network'));
  await waitFor(async () => (await actionsSince(m)).some((r) => r.path === '/api/desktop/config'), 'the LAN switch saves');
  const lanPut = (await actionsSince(m)).find((r) => r.path === '/api/desktop/config');
  check(JSON.stringify(lanPut.body) === '{"obsLan":true}', 'it saves {obsLan: true}', lanPut.body);
  await page.waitForSelector('.obs-lan [data-obs-value="server"]');
  check((await page.$eval('.obs-lan [data-obs-value="server"]', (e) => e.textContent)) === 'rtmp://192.168.1.20:1935/live', 'the home-network address appears');

  m = await mark();
  await page.click(sw('OBS has priority over the queue'));
  await waitFor(async () => (await actionsSince(m)).some((r) => r.path === '/api/desktop/config'), 'the priority switch saves');
  const prioPut = (await actionsSince(m)).find((r) => r.path === '/api/desktop/config');
  check(JSON.stringify(prioPut.body) === '{"obsPriority":false}', 'it saves {obsPriority: false}', prioPut.body);

  m = await mark();
  await page.click('[data-obs-new-key]');
  await waitFor(async () => (await actionsSince(m)).some((r) => r.path === '/api/desktop/obs/key'), 'New key asks for one');
  await page.waitForFunction(() => document.querySelector('[data-obs-value="key"]')?.textContent.startsWith('nowyklucz'));
  check(true, 'the new key is shown');
  await shot(page, 'obs-settings-en.png');
  check(!RAW.test(await page.$eval('#view', (e) => e.innerText)), 'no raw keys');

  // Polish: every text translated.
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await page.waitForFunction(() => document.querySelector('[data-obs-value="server"]') && document.body.innerText.includes('Klucz strumienia'));
  check(!RAW.test(await page.$eval('#view', (e) => e.innerText)), 'Polish: no raw keys');
  await shot(page, 'obs-settings-pl.png');
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('en'));
  await page.close();

  // ---------- On air ----------
  await reset();
  await scenario({ playing: 'live', obs: 'live' });
  page = await openApp('#/channel');
  await page.waitForSelector('.nowp-obs[data-obs="live"]');
  check((await page.$eval('.nowp-obs .nowp-title', (e) => e.textContent)).includes('OBS live'), 'the player bar says OBS live');
  check(await page.$eval('.player button[aria-label="Next"], .player .ctl button:nth-child(4)', (b) => b.disabled), 'Skip is disabled while OBS is on air');
  check(await page.$eval('.q-obs', (e) => e.textContent.includes('the queue waits')), 'the queue says it waits');
  const obsNote = await page.$('[data-obs-note]');
  check(obsNote !== null, 'the Preview explains why its controls are off');
  await shot(page, 'obs-live.png');
  await scenario({ obs: 'brb' });
  await page.waitForSelector('.nowp-obs[data-obs="brb"]', { timeout: 8000 });
  check((await page.$eval('.nowp-obs .nowp-title', (e) => e.textContent)).includes('Be right back'), 'after a drop: Be right back');
  await scenario({ obs: 'off', playing: 'film' });
  await page.waitForFunction(() => !document.querySelector('.nowp-obs') && !document.querySelector('.q-obs'), null, { timeout: 8000 });
  check(true, 'back to the queue: no OBS marks');
  check(page.__errors.length === 0, 'no console errors', page.__errors);
  await page.close();

  // ---------- A remote panel never gets the key ----------
  await reset();
  await scenario({ remote: true });
  page = await openApp('#/settings/obs');
  await page.waitForSelector('#view[data-section="obs"] .set-window-only');
  check((await page.$('[data-obs-value="key"]')) === null, 'a remote panel shows no key');
  const asked = (await requests()).filter((r) => r.path.startsWith('/api/desktop/obs'));
  check(asked.length === 0, 'and never asks for it', asked);
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
}
