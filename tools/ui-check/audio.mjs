// Browser check of Settings → Channel → Audio files (spec 2026-11-08): the picture for audio
// shows the default (cover art, else waveform), a choice saves `audioVisual`, the texts exist in
// Polish too, no raw keys, no horizontal overflow on a narrow window — against the fake engine,
// in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/audio.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: audio.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` - ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(fn, what, timeout = 6000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

const engine = await startFakeEngine(port);
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1100, height: 760 },
  args: ['--disable-extensions', '--no-first-run'],
});
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const requests = () => call('/__test/requests');
const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};
const SELECT = 'select[aria-label="Picture for audio"]';

try {
  await call('/__test/reset', {});
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/admin#/settings/channel`);
  await page.waitForSelector(SELECT);
  const view = () => page.$eval('#view', (e) => e.innerText);
  check((await view()).includes('Audio files'), 'the audio block is there');
  check(await page.$eval(SELECT, (s) => s.value) === 'auto', 'the default is cover art, else waveform');
  const options = await page.$$eval(`${SELECT} option`, (o) => o.map((x) => x.value));
  check(JSON.stringify(options) === JSON.stringify(['auto', 'cover', 'waveform', 'spectrum', 'off']), 'five looks', options);

  await page.selectOption(SELECT, 'spectrum');
  const put = await waitFor(async () => (await requests()).find((r) => r.method === 'PUT' && r.body?.audioVisual), 'saved');
  check(put.body.audioVisual === 'spectrum', 'the choice is saved', put.body);
  await waitFor(async () => (await view()).includes('It applies from the next audio file.'), 'the owner is told');
  check(true, 'the owner is told');
  await page.$eval(SELECT, (e) => e.scrollIntoView({ block: 'center' }));
  await page.screenshot({ path: join(shots, 'audio-channel-en.png') });

  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await waitFor(async () => (await view()).includes('Pliki dźwiękowe'), 'Polish texts');
  check(!/settings\.channel\.audio/.test(await view()), 'no raw keys');
  check(await page.$eval('select[aria-label="Obraz do dźwięku"]', (s) => s.value) === 'spectrum', 'the saved choice stays');
  await page.setViewportSize({ width: 560, height: 760 });
  check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1), 'no overflow at 560 px');
  await page.$eval('select[aria-label="Obraz do dźwięku"]', (e) => e.scrollIntoView({ block: 'center' }));
  await page.screenshot({ path: join(shots, 'audio-channel-pl-narrow.png') });
  check(errors.length === 0, 'no page errors', errors);
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
}
process.exit(0);
