// Browser check of the audio and subtitle choice (spec 2026-11-22-viewer-and-tracks §4): the
// queue row's "Audio and subtitles…" dialog, the player bar's button for the film on air, the
// route each change posts and the preferred languages in Settings → Advanced, in headless Brave
// against the fake engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/tracks.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: tracks.mjs <shots dir> [port]');
if (port === 8080) throw new Error('never port 8080');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  console.log(`ok - ${what}`);
}

const engine = await startFakeEngine(port);
const base = `http://127.0.0.1:${port}`;
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
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
const posts = async () => (await call('/__test/requests')).filter((r) => /\/tracks$/.test(r.path));

const profile = mkdtempSync(join(tmpdir(), 'brave-tracks-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1280, height: 900 },
  args: ['--disable-extensions', '--no-first-run'],
});

try {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 4\d\d|Failed to load resource/.test(m.text()) && errors.push(m.text()));

  // Not read yet: no button on air, the menu entry is off.
  await call('/__test/scenario', { tracks: false });
  await page.goto(`${base}/admin#/channel`);
  await page.waitForSelector('li[data-id]');
  check(!(await page.$('[data-tracks-button]')), 'no tracks button until the film has been read');
  await page.click('li[data-id]', { button: 'right' });
  const entry = page.locator('[role=menuitem]', { hasText: 'Audio and subtitles…' });
  check(await entry.isDisabled(), 'the menu entry waits for the tracks');
  await page.keyboard.press('Escape');

  // Read: the queue row's dialog.
  await call('/__test/scenario', { tracks: true });
  await waitFor(() => page.$('[data-tracks-button]'), 'the on-air button appears');
  await page.click('li[data-id]', { button: 'right' });
  await page.locator('[role=menuitem]', { hasText: 'Audio and subtitles…' }).click();
  await page.waitForSelector('[data-dialog=tracks]', { timeout: 4000 }).catch(async (e) => { await page.screenshot({ path: join(shots, 'debug.png') }); throw e; });
  const audio = await page.$$eval('select[name=audio] option', (o) => o.map((x) => x.textContent));
  const subs = await page.$$eval('select[name=subtitle] option', (o) => o.map((x) => x.textContent));
  check(audio.join('|') === 'Auto (Polish · AAC 2.0)|English · AC3 5.1|Polish · AAC 2.0', 'audio options: auto says what it picks, then each track', audio);
  check(subs.join('|') === 'Auto (Polish · PGS)|Off|Polish · PGS|English (forced) · SRT|Track 3 · ASS', 'subtitle options: auto, off, each track', subs);
  await page.selectOption('select[name=subtitle]', '-1');
  await waitFor(async () => (await posts()).length === 1, 'the change posts');
  const first = (await posts())[0];
  check(/items\/\d+\/tracks$/.test(first.path) && first.body.subtitle === -1 && !('audio' in first.body), 'subtitles off posts only the subtitle', first.body);
  await page.waitForFunction(() => document.querySelector('select[name=subtitle]').value === '-1');
  check(true, 'the select shows the saved choice');
  await page.selectOption('select[name=audio]', '0');
  await waitFor(async () => (await posts()).length === 2, 'the audio change posts');
  check((await posts())[1].body.audio === 0, 'audio 0 posted');
  await page.selectOption('select[name=audio]', '');
  await waitFor(async () => (await posts()).length === 3, 'auto posts');
  check((await posts())[2].body.audio === null, 'Auto posts null');
  await page.screenshot({ path: join(shots, 'tracks-dialog.png') });
  await page.keyboard.press('Escape');

  // The film on air: the button, and the toast of the restart.
  await page.click('[data-tracks-button]');
  await page.waitForSelector('[data-dialog=tracks]');
  await page.selectOption('select[name=audio]', '1');
  await waitFor(async () => (await posts()).length === 4, 'the on-air change posts');
  await waitFor(async () => (await page.textContent('body')).includes('restarts from its position'), 'the toast says the film restarts');
  check(true, 'the film on air restarts from its position');
  await page.screenshot({ path: join(shots, 'tracks-on-air.png') });
  await page.keyboard.press('Escape');

  // Settings → Advanced.
  await page.goto(`${base}/admin#/settings/advanced`);
  await page.waitForSelector('form[data-form=preferredAudioLangs]');
  check(!(await page.$('input[name=subtitlesLanguage]')), 'the old subtitle language field is gone');
  await page.fill('input[name=preferredAudioLangs]', ' pl ,en');
  await page.click('form[data-form=preferredAudioLangs] button[type=submit]');
  await waitFor(async () => (await call('/api/admin/channels/main/settings')).preferredAudioLangs === 'pl, en', 'audio languages saved');
  check(true, 'preferred audio languages save as "pl, en"');
  await page.fill('input[name=preferredSubtitleLangs]', 'pl, 1234');
  await page.click('form[data-form=preferredSubtitleLangs] button[type=submit]');
  await waitFor(async () => (await page.textContent('.sform')).includes('expected language codes'), 'an invalid list shows the engine error');
  check((await call('/api/admin/channels/main/settings')).preferredSubtitleLangs === '', 'nothing was saved');
  await page.fill('input[name=preferredSubtitleLangs]', 'en');
  await page.click('form[data-form=preferredSubtitleLangs] button[type=submit]');
  await waitFor(async () => (await call('/api/admin/channels/main/settings')).preferredSubtitleLangs === 'en', 'subtitle languages saved');
  check(true, 'preferred subtitle languages save');
  await page.screenshot({ path: join(shots, 'tracks-settings.png'), fullPage: true });

  check(errors.length === 0, 'no console errors', errors);
  console.log('tracks: all checks passed');
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
}
process.exit(0);
