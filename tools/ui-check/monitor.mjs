// Browser check of the producer's monitor ("Preview" left of "On air" in the Channel view)
// against the fake engine, in headless Brave with a temporary profile. Two real HLS folders
// made by bin/ffmpeg.exe: the test pattern (clip A) and a plain red picture (clip B, "after the
// seek"), so a change of picture can be read from the video's pixels.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/monitor.mjs <shots dir> [port]
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
const port = Number(process.argv[3] ?? 8201);
if (!shots) throw new Error('usage: monitor.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}

const pattern = await makeTestStream();
const red = await makeTestStream({ color: 'red' });
const engine = await startFakeEngine(port, { hlsDir: pattern.dir, hlsDir2: red.dir });
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  viewport: { width: 1100, height: 680 },
  args: ['--disable-extensions', '--no-first-run', '--autoplay-policy=no-user-gesture-required'],
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
const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
// The window's own reads of the list (hls.js reloads it with `?from=`).
const monitorReads = async () => (await requests()).filter((r) => r.path === '/api/desktop/monitor.m3u8' && !r.search).length;
const monitorSegments = async () => (await requests()).filter((r) => r.path.startsWith('/api/desktop/monitor/')).map((r) => r.path.slice('/api/desktop/monitor/'.length));
const stateOf = (page) => page.evaluate(async () => (await import('/static/app/components/monitor.js')).monitorState.value);
const aheadOf = (page) => page.evaluate(async () => (await import('/static/app/components/monitor.js')).monitorAheadS.value);
/** The colour in the middle of the monitor's picture. */
const pixel = (page) =>
  page.$eval('.monitor-box video', (v) => {
    if (v.readyState < 2) return null;
    const c = document.createElement('canvas');
    c.width = 32;
    c.height = 18;
    const g = c.getContext('2d');
    g.drawImage(v, 0, 0, 32, 18);
    return [...g.getImageData(16, 9, 1, 1).data.slice(0, 3)];
  });
const isRed = (p) => Array.isArray(p) && p[0] > 180 && p[1] < 80 && p[2] < 80;

async function waitFor(fn, what, timeout = 8000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

async function openApp(viewport) {
  const page = await ctx.newPage();
  if (viewport) await page.setViewportSize(viewport);
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  page.on('response', (r) => r.status() >= 400 && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${base}/admin`);
  await page.waitForSelector('.player .play');
  page.__errors = errors;
  return page;
}

/** Both screens side by side, each at least 300 px, nothing wider than the window. */
async function layout(page) {
  return page.evaluate(() => {
    const r = (sel) => document.querySelector(sel)?.getBoundingClientRect();
    const m = r('.screen-monitor');
    const a = r('.screen-air');
    const view = document.querySelector('#view');
    return {
      monitor: m && { left: m.left, top: m.top, width: m.width },
      air: a && { left: a.left, top: a.top, width: a.width },
      monitorBox: r('.monitor-box')?.top,
      airBox: r('.video')?.top,
      pageOverflow: document.documentElement.scrollWidth - window.innerWidth,
      viewOverflow: view.scrollWidth - view.clientWidth,
    };
  });
}

const clipA = { key: 'clip-7-0', itemId: 7, baseS: 80, segs: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9], src: 'a' };
const clipB = { key: 'clip-7-0-r1', itemId: 7, baseS: 30, segs: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9], src: 'b' };

try {
  await reset();
  await scenario({ monitorFeed: { clips: [clipA], firstSeq: 100, antenna: 102, aired: 105 } });
  let page = await openApp();

  // ---------- Two screens, the monitor plays from the newest aired segment ----------
  await waitFor(async () => (await stateOf(page)) === 'playing', 'the monitor plays');
  await waitFor(() => page.$eval('.monitor-box video', (v) => v.currentTime > 0 && !v.paused), 'the monitor picture moves');
  const firstSegs = await monitorSegments();
  check(firstSegs[0] === 'clip-7-0/5.ts', 'a fresh monitor starts at the newest aired segment (row 105), not at the end of the list', firstSegs.slice(0, 3));
  check(await page.$eval('.monitor-box video', (v) => v.muted), 'the monitor starts muted');
  let lay = await layout(page);
  check(lay.monitor && lay.air && lay.monitor.left < lay.air.left && Math.abs(lay.monitor.top - lay.air.top) < 1, '1100x680: Preview on the left, On air on the right, side by side', lay);
  check(lay.monitor.width >= 300 && lay.air.width >= 300, '1100x680: each screen at least 300 px wide', lay);
  check(Math.abs(lay.monitorBox - lay.airBox) < 1, 'the two pictures start at the same height', lay);
  check(lay.pageOverflow <= 0 && lay.viewOverflow <= 0, '1100x680: no horizontal overflow', lay);
  await waitFor(async () => /^Preview · about \d+ s ahead of on-air$/.test(await text(page, '.monitor-caption')), 'the caption says how far ahead');
  const ahead = await aheadOf(page);
  check(ahead >= 5 && ahead <= 12, 'about 6 s ahead (three 2 s rows between the antenna row and the playing one)', ahead);
  check(/^On air( · about \d+ s behind the server)?$/.test(await text(page, '.screen-air .stage-caption')), 'the right screen is captioned "On air"', await text(page, '.screen-air .stage-caption'));
  await page.screenshot({ path: join(shots, 'monitor-en-1100x680.png') });

  // The monitor's own sound switch.
  check((await page.getAttribute('.monitor-mute', 'aria-label')) === 'Unmute “Preview”', 'the sound button says it unmutes');
  await page.click('.monitor-mute');
  await waitFor(() => page.$eval('.monitor-box video', (v) => !v.muted), 'the monitor has sound');
  check((await page.getAttribute('.monitor-mute', 'aria-label')) === 'Mute “Preview”', 'unmuted: the button mutes again');
  check(await page.$eval('.video video', (v) => v !== document.querySelector('.monitor-box video')), 'the On air screen has its own video');
  await page.click('.monitor-mute');
  await waitFor(() => page.$eval('.monitor-box video', (v) => v.muted), 'muted again');
  check(true, 'mute / unmute toggles the monitor only');

  // ---------- A seek: the new clip appears and the picture changes at once ----------
  const segsBefore = (await monitorSegments()).length;
  const t0 = Date.now();
  await scenario({ monitorFeed: { clips: [clipA, clipB], firstSeq: 100, antenna: 103, aired: 106 } });
  await waitFor(async () => (await monitorSegments()).slice(segsBefore).includes('clip-7-0-r1/0.ts'), 'the new clip\'s first segment is loaded', 5000);
  await waitFor(async () => isRed(await pixel(page)), 'the monitor shows the new clip (red)', 6000);
  const took = Date.now() - t0;
  check(took < 4000, `after the list changed the monitor shows the new clip in ${took} ms (< 4 s, from its first segment)`, took);
  const newSegs = (await monitorSegments()).slice(segsBefore);
  check(newSegs.find((s) => s.startsWith('clip-7-0-r1/')) === 'clip-7-0-r1/0.ts', 'it starts at the first segment of the new clip', newSegs);
  const t1 = await page.$eval('.monitor-box video', (v) => v.currentTime);
  await sleep(2000);
  const t2 = await page.$eval('.monitor-box video', (v) => v.currentTime);
  check(t2 - t1 > 1.4 && t2 - t1 < 2.6, 'it plays in real time (not a jump to the end of the list)', t2 - t1);
  await page.screenshot({ path: join(shots, 'monitor-after-seek-en-1100x680.png') });

  // ---------- One read at a time ----------
  await scenario({ monitorDelayMs: 2500 });
  let reads0 = await monitorReads();
  await sleep(6000);
  let reads1 = await monitorReads();
  const stats = await call('/__test/stats');
  check(stats.maxMonitorInFlight === 1, 'a slow list: never two reads at once', stats);
  check(reads1 - reads0 <= 3, 'a slow list: the next read waits for the answer (≤ 3 in 6 s)', reads1 - reads0);
  await scenario({ monitorDelayMs: 0 });
  reads0 = await monitorReads();
  await sleep(3200);
  reads1 = await monitorReads();
  check(reads1 - reads0 >= 2 && reads1 - reads0 <= 4, 'otherwise about one read a second', reads1 - reads0);

  // ---------- Sync with on-air ----------
  const segsSync = (await monitorSegments()).length;
  await page.click('.monitor-sync');
  await waitFor(async () => (await monitorSegments()).slice(segsSync).includes('clip-7-0/3.ts'), 'sync loads the segment on air (row 103)');
  await waitFor(async () => {
    const p = await pixel(page);
    return p && !isRed(p);
  }, 'after the sync the monitor shows the old clip (what the viewers see)', 6000);
  await waitFor(async () => (await text(page, '.monitor-caption')) === 'Preview · in sync with on-air', 'the caption says in sync');
  check(true, 'Sync with on-air: the monitor shows the antenna\'s segment and says so');
  // The key S; nothing there to sync to: a message.
  await scenario({ monitorFeed: { clips: [clipA, clipB], firstSeq: 100, antenna: null, aired: 106 } });
  await sleep(1300);
  await page.click('.stage-caption');
  await page.keyboard.press('s');
  await waitFor(() => page.$('.toast'), 'a message when on-air is no longer in the list');
  check((await text(page, '.toast span')) === 'What is on air now is no longer in the preview.', 'the message says why', await text(page, '.toast span'));
  await page.click('.toast .icon-button');
  await scenario({ monitorFeed: { clips: [clipA, clipB], firstSeq: 100, antenna: 104, aired: 106 } });
  await sleep(1300);
  const segsKey = (await monitorSegments()).length;
  await page.keyboard.press('S');
  await waitFor(async () => (await monitorSegments()).slice(segsKey).includes('clip-7-0/4.ts'), 'the key S syncs like the button');
  check(true, 'key S syncs with on-air');
  await page.focus('.addf-input');
  const segsTyping = (await monitorSegments()).length;
  await page.keyboard.press('s');
  await sleep(1500);
  const typed = (await monitorSegments()).slice(segsTyping);
  check((await page.$eval('.addf-input', (e) => e.value)) === 's' && !typed.includes('clip-7-0/4.ts'), 'S typed in the link field only types (no new sync)', typed);
  await page.$eval('.addf-input', (e) => {
    e.value = '';
    e.blur();
  });

  // A live stream: nothing to sync.
  await scenario({ playing: 'live' });
  await waitFor(() => page.$eval('.monitor-sync', (b) => b.disabled), 'the sync button is off for a live stream');
  check((await page.getAttribute('.monitor-sync', 'title')) === 'Not needed for a live stream', 'its tooltip says why');
  await scenario({ playing: 'film' });
  await waitFor(() => page.$eval('.monitor-sync', (b) => !b.disabled), 'on again for a film');

  // ---------- Paused: a still picture of where the film resumes ----------
  await scenario({ paused: true, pauseReasons: ['manual'], positionS: 36 });
  await waitFor(async () => (await stateOf(page)) === 'paused', 'paused: the monitor stands still');
  await waitFor(async () => isRed(await pixel(page)), 'the still picture is the paused clip (row of 36 s)', 6000);
  check(await page.$eval('.monitor-box video', (v) => v.paused), 'the video is paused');
  check((await text(page, '.monitor-badge')) === 'Paused — the video resumes from here', 'with a label');
  const stillAt = await page.$eval('.monitor-box video', (v) => v.currentTime);
  await sleep(1500);
  check((await page.$eval('.monitor-box video', (v) => v.currentTime)) === stillAt, 'it does not move while paused');
  await scenario({ paused: false, pauseReasons: [] });
  await sleep(1500);
  check((await stateOf(page)) === 'paused', 'resumed: the still waits for the film\'s next clip');
  await scenario({ monitorFeed: { clips: [clipA, clipB, { ...clipA, key: 'clip-7-0-r2', baseS: 36 }], firstSeq: 100, antenna: 104, aired: 106 } });
  await waitFor(async () => (await stateOf(page)) === 'playing', 'the relaunched clip plays');
  await waitFor(() => page.$eval('.monitor-box video', (v) => !v.paused && v.currentTime > 0), 'and moves');
  check(true, 'after the resume the monitor plays the new clip');
  await page.screenshot({ path: join(shots, 'monitor-resumed-en-1100x680.png') });

  // ---------- Hidden window: nothing is read, the player is gone ----------
  const setHidden = (hidden) =>
    page.evaluate((h) => {
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
      document.dispatchEvent(new Event('visibilitychange'));
    }, hidden);
  await setHidden(true);
  await sleep(600);
  const hiddenAt = (await requests()).filter((r) => r.path.startsWith('/api/desktop/monitor')).length;
  await sleep(3500);
  check((await requests()).filter((r) => r.path.startsWith('/api/desktop/monitor')).length === hiddenAt, 'hidden window: no monitor requests (0 in 3.5 s)');
  check(await page.$eval('.monitor-box video', (v) => v.paused), 'hidden window: the monitor is stopped');
  await setHidden(false);
  await waitFor(async () => (await stateOf(page)) === 'playing', 'visible again: the monitor plays');
  check(true, 'visible again: reading and playing resume');

  // ---------- Nothing on air ----------
  await scenario({ monitorFeed: null });
  await waitFor(async () => (await stateOf(page)) === 'none', 'an empty list: no preview');
  check((await text(page, '.monitor-note')) === 'No preview', '"No preview" in the left screen');
  check(await page.$eval('.monitor-sync', (b) => b.disabled), 'nothing to sync');
  const segsNone = (await monitorSegments()).length;
  await sleep(2000);
  check((await monitorSegments()).length === segsNone, 'no segments are fetched without a list');
  await page.screenshot({ path: join(shots, 'monitor-none-en-1100x680.png') });

  // ---------- Leaving the view stops it ----------
  await scenario({ monitorFeed: { clips: [clipA], firstSeq: 100, antenna: 102, aired: 105 } });
  await waitFor(async () => (await stateOf(page)) === 'playing', 'playing again');
  await page.click('.rail-history');
  await waitFor(async () => (await stateOf(page)) === 'off', 'leaving the Channel view stops the monitor');
  await sleep(500);
  const away = (await requests()).filter((r) => r.path.startsWith('/api/desktop/monitor')).length;
  await sleep(2500);
  check((await requests()).filter((r) => r.path.startsWith('/api/desktop/monitor')).length === away, 'no monitor requests in another view');
  await page.keyboard.press('s');
  check((await page.$('.toast')) === null, 'S does nothing where there is no monitor');
  await page.click('.rail-channel');
  await waitFor(async () => (await stateOf(page)) === 'playing', 'back in the Channel view the monitor plays again');
  check(true, 'coming back starts it again');

  // The shortcuts dialog names S.
  await page.keyboard.press('?');
  await page.waitForSelector('.shortcuts');
  const rows = await page.$$eval('.shortcuts dt', (d) => d.map((e) => e.textContent.trim()));
  const sAt = rows.indexOf('S');
  check(sAt >= 0 && (await page.$$eval('.shortcuts dd', (d) => d.map((e) => e.textContent.trim())))[sAt] === 'Sync Preview with on-air', 'the shortcuts list has S', rows);
  await page.keyboard.press('Escape');

  // ---------- German, live switch ----------
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('de'));
  await waitFor(async () => (await text(page, '.monitor-sync span')) === 'Mit der Sendung synchronisieren', 'the button follows the language');
  await waitFor(async () => /^Vorschau · etwa \d+ s vor der Sendung$/.test(await text(page, '.monitor-caption')), 'the caption is German');
  check(/^Auf Sendung( · etwa \d+ s hinter dem Server)?$/.test(await text(page, '.screen-air .stage-caption')), 'and the On air caption', await text(page, '.screen-air .stage-caption'));
  const fits = await page.$eval('.monitor-sync span', (s) => s.scrollWidth <= s.clientWidth + 1);
  check(fits, 'the German button label is not cut at 1100 px');
  const raw = await page.$eval('.stage', (e) => e.textContent.match(/\bmonitor\.[a-z_]+/g));
  check(raw === null, 'no untranslated keys', raw);
  await page.screenshot({ path: join(shots, 'monitor-de-1100x680.png') });

  // ---------- The smallest window ----------
  await page.setViewportSize({ width: 960, height: 600 });
  await sleep(400);
  lay = await layout(page);
  check(lay.monitor.left < lay.air.left && Math.abs(lay.monitor.top - lay.air.top) < 1, '960x600: still side by side', lay);
  check(lay.monitor.width >= 300 && lay.air.width >= 300, '960x600: each screen at least 300 px wide', lay);
  check(lay.pageOverflow <= 0 && lay.viewOverflow <= 0, '960x600: no horizontal overflow', lay);
  const bottoms = await page.evaluate(() => ({
    sync: document.querySelector('.monitor-sync').getBoundingClientRect().bottom,
    player: document.querySelector('.player').getBoundingClientRect().top,
  }));
  check(bottoms.sync <= bottoms.player, '960x600: the sync button is above the player bar', bottoms);
  await page.screenshot({ path: join(shots, 'monitor-de-960x600.png') });
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('en'));
  await waitFor(async () => (await text(page, '.monitor-sync span')) === 'Sync with on-air', 'back to English');
  await page.screenshot({ path: join(shots, 'monitor-en-960x600.png') });
  check(page.__errors.length === 0, 'no errors in the console', page.__errors);
  await page.close();

  // ---------- A phone: On air only ----------
  // A read the closed page had already sent may still land: let it, then start counting.
  await sleep(1500);
  await reset();
  await scenario({ monitorFeed: { clips: [clipA], firstSeq: 100, antenna: 102, aired: 105 } });
  page = await openApp({ width: 390, height: 844 });
  await sleep(2500);
  check((await page.$('.screen-monitor')) === null && (await page.$('.video')) !== null, 'phone: only the On air screen');
  check((await monitorReads()) === 0, 'phone: the monitor reads nothing');
  check((await text(page, '.stage-caption')).startsWith('On air'), 'phone: the single screen is captioned On air', await text(page, '.stage-caption'));
  check((await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)) <= 0, 'phone: no horizontal overflow');
  await page.screenshot({ path: join(shots, 'monitor-phone-390x844.png') });
  await page.close();

  // ---------- A remote session: grey, with a note ----------
  await reset();
  await scenario({ remote: true, monitorFeed: { clips: [clipA], firstSeq: 100, antenna: 102, aired: 105 } });
  page = await openApp();
  await waitFor(() => page.$('.screen-monitor.is-off'), 'remote: the monitor is off');
  await sleep(2000);
  check((await text(page, '.monitor-note')) === 'Available in the program on the computer', 'remote: a note says where it works');
  check(await page.$eval('.monitor-sync', (b) => b.disabled), 'remote: the sync button is off');
  check((await page.$('.monitor-mute')) === null, 'remote: no sound button');
  check((await requests()).every((r) => !r.path.startsWith('/api/desktop/monitor')), 'remote: no monitor request at all');
  lay = await layout(page);
  check(lay.monitor.width >= 300 && lay.pageOverflow <= 0, 'remote: the same two screens, no overflow', lay);
  await page.screenshot({ path: join(shots, 'monitor-remote-1100x680.png') });
  // The window's info answers 403 to a remote session: that is how the panel learns it is remote.
  const unexpected = page.__errors.filter((e) => !e.includes('/api/desktop/info') && !/status of 403/.test(e));
  check(unexpected.length === 0, 'remote: no other errors in the console', unexpected);
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
  pattern.cleanup();
  red.cleanup();
}
