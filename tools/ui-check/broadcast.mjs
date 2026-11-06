// Browser check of broadcasting like TV (spec 2026-11-06-broadcast-design): the next video's
// readiness mark, "Trim…" (start and end points, a wrong time refused here), "Start at…" (a
// quick "in 5 min" and a clock time), the countdown line of the player bar, the link warning
// at the top, the upload field (Settings → Channel) and the level meters beside both screens —
// in headless Brave against the fake engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/broadcast.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8197);
if (!shots) throw new Error('usage: broadcast.mjs <shots dir> [port]');
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

const profile = mkdtempSync(join(tmpdir(), 'brave-broadcast-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1280, height: 800 },
  args: ['--disable-extensions', '--no-first-run', '--autoplay-policy=no-user-gesture-required'],
});

try {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 4\d\d|Failed to load resource/.test(m.text()) && errors.push(m.text()));

  await scenario({ playing: 'film' });
  const panel = await call('/api/admin/channels/main');
  const first = panel.queue[0];
  check(Boolean(first), 'the fake queue has a video', panel.queue);
  await scenario({ next: { itemId: first.id, state: 'ready' } });
  await page.goto(`${base}/admin`);
  const row = `li.qi[data-id="${first.id}"]`;
  await page.waitForSelector(row);

  // ---------- Readiness of the next video ----------
  const ready = await waitFor(() => page.$eval(row, (li) => li.querySelector('[data-mark=ready]')?.textContent?.trim() ?? null).catch(() => null), 'the ready mark');
  check(ready === 'Ready', 'the next video is marked "Ready"', ready);
  await scenario({ next: { itemId: first.id, state: 'resolving' } });
  await waitFor(async () => (await page.$eval(`${row} [data-mark=ready]`, (e) => e.textContent.trim())) === 'Preparing…', 'resolving shows "Preparing…"');
  check(true, 'resolving ahead shows "Preparing…"');

  // ---------- Trim ----------
  const openFromMenu = async (label) => {
    await page.click(`${row} .qi-text`, { button: "right" });
    await page.click(`[role=menuitem]:has-text("${label}")`);
    await page.waitForSelector('.dialog');
  };
  await openFromMenu('Trim…');
  check((await page.textContent('#dialog-title')) === 'Start and end of the video', 'the trim dialog opens');
  await page.fill('.dialog input[name=inS]', 'abc');
  await page.click('.dialog-foot button:has-text("Save")');
  await waitFor(async () => (await page.textContent('.toast').catch(() => '')).includes('1:30 or 1:02:30'), 'a wrong time is refused');
  check(true, 'a wrong time is refused here');
  await openFromMenu('Trim…');
  await page.fill('.dialog input[name=inS]', '0:30');
  await page.fill('.dialog input[name=outS]', '2:00');
  await page.click('.dialog-foot button:has-text("Save")');
  const trimmed = await waitFor(async () => (await call('/__test/requests')) && (await page.$eval(row, (li) => li.querySelector('[data-mark=trim]')?.textContent ?? null)), 'the trim mark');
  check(trimmed === '0:30–2:00', 'the row shows the trimmed part', trimmed);
  await page.screenshot({ path: join(shots, 'broadcast-trim.png') });

  // ---------- Start at ----------
  await openFromMenu('Start at…');
  check((await page.textContent('#dialog-title')) === 'Start at a set time', 'the start-time dialog opens');
  await page.click('.dialog [data-after-min="5"]');
  const timeMark = await waitFor(() => page.$eval(row, (li) => li.querySelector('[data-mark=start]')?.textContent?.trim() ?? null), 'the start mark');
  check(/^at \d{1,2}:\d{2}/.test(timeMark), '"in 5 min" sets a start time shown on the row', timeMark);
  check(!(await page.$('.dialog')), 'the quick button closes the dialog');

  // ---------- The countdown in the player bar ----------
  await scenario({ playing: 'none', countdown: true });
  const sub = await waitFor(async () => {
    const text = await page.textContent('.nowp-sub').catch(() => '');
    return /^Starts in \d+:\d{2}$/.test(text) ? text : null;
  }, 'the countdown line');
  check(true, `the player bar counts down ("${sub}")`);
  check((await page.textContent('.nowp-title')) === first.title, 'with the title of the video it counts down to');

  // ---------- The link warning ----------
  await scenario({ link: { viewers: 6, streamMbps: 6.2, neededMbps: 37.2, sendingMbps: 30, uploadMbps: 40, level: 'warn' } });
  const warn = await waitFor(() => page.textContent('[data-status=link]').catch(() => null), 'the link notice');
  check(warn.includes('37.2') && warn.includes('40'), 'a warning when viewers need close to the upload', warn);
  await scenario({ link: { viewers: 8, streamMbps: 6.2, neededMbps: 49.6, sendingMbps: 30, uploadMbps: 40, level: 'over' } });
  await waitFor(async () => (await page.getAttribute('[data-status=link]', 'class').catch(() => '')).includes('status-err'), 'over the upload is an error');
  check(true, 'above the upload the notice turns into an error');
  await page.screenshot({ path: join(shots, 'broadcast-link.png') });

  // ---------- Meters ----------
  await scenario({ playing: 'film', countdown: false, link: null });
  check(Boolean(await page.$('.screen-monitor [data-meter]')), 'a level meter beside the Preview');
  // On air as a small picture has none; beside the Preview it has its own.
  await page.evaluate(() => localStorage.setItem('vrcinema.screens', JSON.stringify({ layout: 'side', docked: 'side' })));
  await page.reload();
  await page.waitForSelector('.screen-air [data-meter]');
  check(true, 'and beside On air');
  await page.click('.queue-title, h2.q-head'); // the first click starts the audio
  await waitFor(() => page.$eval('.screen-air [data-meter]', (m) => m.dataset.active === 'yes'), 'the On air meter taps the player after a click');
  check(true, 'after the first click the On air meter reads the player');
  const listening = await page.$eval('.screen-air video', (v) => ({
    volume: v.volume,
    real: Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'volume').get.call(v),
  }));
  check(Math.abs(listening.volume - 0.6) < 0.001 && listening.real === 1, 'the host volume moves to the gain; the element plays at full level', listening);
  check((await page.textContent('.screen-air .stage-caption')).startsWith('On air'), 'the On air caption keeps its text');
  check(Boolean(await page.$('.meter .meter-clip[aria-label^="Clipping"]')), 'the clipping light has its label');

  // ---------- Settings: the upload ----------
  await page.goto(`${base}/admin#/settings/channel`);
  await page.waitForSelector('input[name=uploadMbps]');
  await page.fill('input[name=uploadMbps]', '40');
  await page.click('form[data-form=upload] button[type=submit]');
  await waitFor(async () => (await page.textContent('.channel-settings').catch(() => '')).includes('Upload speed saved.'), 'the upload is saved');
  check(true, 'the upload speed is saved');

  check(errors.length === 0, 'no errors in the page', errors);
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
}
