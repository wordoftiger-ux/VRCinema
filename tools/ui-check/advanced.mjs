// Browser check of Settings → Advanced (spec 2026-11-06-advanced-design): the broadcast options
// (switches, peak limit, loudness target, alarm thresholds with a wrong value refused here),
// the broadcast journal (rows, CSV download, clearing) and the alarm notices at the top — in the
// window and in a remote panel, in headless Brave against the fake engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/advanced.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8198);
if (!shots) throw new Error('usage: advanced.mjs <shots dir> [port]');
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

const now = Date.now();
const JOURNAL = [
  { id: 3, itemId: 9, title: 'Live now', sourceUrl: 'https://youtu.be/c', requesterName: null, addedBy: 'admin', startedAt: now - 60000, endedAt: null, fromS: 0, toS: null },
  { id: 2, itemId: 8, title: 'Second film', sourceUrl: 'https://youtu.be/b', requesterName: 'Kasia', addedBy: 'public', startedAt: now - 400000, endedAt: now - 60000, fromS: 0, toS: 340 },
];

const profile = mkdtempSync(join(tmpdir(), 'brave-advanced-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1280, height: 900 },
  acceptDownloads: true,
  args: ['--disable-extensions', '--no-first-run'],
});

try {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 4\d\d|Failed to load resource/.test(m.text()) && errors.push(m.text()));

  await scenario({ journal: JOURNAL });
  await page.goto(`${base}/admin#/settings/advanced`);
  await page.waitForSelector('form[data-form=peak]');
  const text = () => page.textContent('.sform');
  check((await text()).includes('Smooth transitions') && (await text()).includes('Broadcast journal'), 'the Advanced tab shows the broadcast options and the journal');
  check(await page.$eval('select[name=transitionS]', (s) => s.disabled), 'the fade length waits for the transitions switch');

  // Switches save at once.
  await page.click('button[role=switch][aria-label="Smooth transitions"]');
  await waitFor(async () => (await call('/api/admin/channels/main/settings')).transitionsEnabled === true, 'transitions are switched on');
  check((await text()).includes('Smooth transitions: on.'), 'the switch confirms');
  await waitFor(() => page.$eval('select[name=transitionS]', (s) => !s.disabled), 'the fade length can be chosen');
  await page.selectOption('select[name=transitionS]', '2');
  await waitFor(async () => (await call('/api/admin/channels/main/settings')).transitionS === 2, 'the fade length is saved');
  check(true, 'the fade length is saved');

  // Peak limit.
  await page.fill('input[name=peakMbps]', '4.5');
  await page.click('form[data-form=peak] button[type=submit]');
  await waitFor(async () => (await call('/api/admin/channels/main/settings')).peakMbps === 4.5, 'the peak limit is saved');
  check((await text()).includes('Saved. It applies from the next video.'), 'the peak limit confirms');

  // Loudness: off by default, the target needs the switch.
  check(await page.$eval('[role=radiogroup][aria-label="Target level"] button', (b) => b.disabled), 'the loudness target waits for its switch');
  await page.click('button[role=switch][aria-label="Loudness normalisation"]');
  await waitFor(() => page.$eval('[role=radiogroup][aria-label="Target level"] button', (b) => !b.disabled), 'the target can be chosen');
  await page.click('[role=radiogroup][aria-label="Target level"] button[data-value="-23"]');
  await waitFor(async () => (await call('/api/admin/channels/main/settings')).loudnessTarget === -23, 'the target is saved');
  check(true, 'loudness on with −23 LUFS');

  // Alarms: a wrong threshold is refused here.
  await page.click('button[role=switch][aria-label="Black picture and silence alarms"]');
  await waitFor(() => page.$eval('input[name=alarmBlackS]', (i) => !i.disabled), 'the thresholds can be typed');
  await page.fill('input[name=alarmBlackS]', '1');
  await page.click('form[data-form=alarms] button[type=submit]');
  await waitFor(async () => (await text()).includes('Black picture after (s): enter a number from 2 to 300.'), 'a too short threshold is refused');
  check((await call('/api/admin/channels/main/settings')).alarmBlackS === 10, 'nothing was saved');
  await page.fill('input[name=alarmBlackS]', '20');
  await page.click('form[data-form=alarms] button[type=submit]');
  await waitFor(async () => (await call('/api/admin/channels/main/settings')).alarmBlackS === 20, 'the threshold is saved');
  check(true, 'the black picture threshold is saved');

  // The journal.
  const rows = await page.$$eval('table[data-journal] tbody tr', (trs) => trs.map((tr) => tr.textContent));
  check(rows.length === 2 && rows[0].includes('Live now') && rows[0].includes('on air') && rows[1].includes('5:40'), 'the journal lists the rows, newest first', rows);
  const [download] = await Promise.all([page.waitForEvent('download'), page.click('button[data-export=csv]')]);
  check(download.suggestedFilename() === 'vrcinema-journal-20261106.csv', 'the CSV export downloads', download.suggestedFilename());
  await page.screenshot({ path: join(shots, 'advanced-window.png'), fullPage: true });
  page.once('dialog', (d) => d.accept());
  await page.click('button[data-journal-clear]');
  await page.waitForSelector('[data-journal-empty]');
  check(true, 'clearing empties the journal');

  // The alarm notices at the top.
  await scenario({ alarms: { black: true, silence: false } });
  await page.goto(`${base}/admin#/channel`);
  await page.waitForSelector('[data-status=alarm_black]');
  check((await page.textContent('[data-status=alarm_black]')).includes('Black picture on air.'), 'a black picture alarm shows at the top');
  check((await page.getAttribute('[data-status=alarm_black] a', 'href')) === '#/settings/advanced', 'the notice links to Settings → Advanced');
  check(!(await page.$('[data-status=alarm_silence]')), 'no silence notice without its alarm');

  // A remote panel has the broadcast options too.
  await scenario({ remote: true, alarms: { black: false, silence: true } });
  const remote = await ctx.newPage();
  await remote.goto(`${base}/admin#/settings/advanced`);
  await remote.waitForSelector('form[data-form=peak]');
  check(Boolean(await remote.$('button[role=switch][aria-label="Burned-in subtitles"]')), 'a remote panel shows the broadcast options');
  await remote.goto(`${base}/admin#/channel`);
  await remote.waitForSelector('[data-status=alarm_silence]');
  check(true, 'a remote panel shows the silence alarm');
  await remote.screenshot({ path: join(shots, 'advanced-remote.png') });
  await remote.close();

  check(errors.length === 0, 'no errors in the page', errors);
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
}
