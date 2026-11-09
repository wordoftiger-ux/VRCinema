// Browser check of the new Preview (spec 2026-11-06-preview-player-design): the Preview plays the
// film itself (a real MP4 made by bin/ffmpeg.exe, served by the fake engine with Range), the
// owner's controls change its picture at once, On air sits in its corner (swap, side by side,
// own window), two volumes, the keys, the seek-bar thumbnails, the fallback to segments, the
// air clock's latency and the app icon. Headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/preview-player.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';
import { makeTestFilm, makeTestStream } from './hls-fixture.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8207);
if (!shots) throw new Error('usage: preview-player.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}

const film = await makeTestFilm();
const stream = await makeTestStream();
const engine = await startFakeEngine(port, { hlsDir: stream.dir, previewFile: film.file });
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  viewport: { width: 1280, height: 760 },
  args: ['--disable-extensions', '--no-first-run', '--autoplay-policy=no-user-gesture-required'],
});
// Every page (the air's own window too) runs as in the program's window.
await ctx.addInitScript(() => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
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

async function waitFor(fn, what, timeout = 8000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

const video = (page) =>
  page.$eval('.screen-monitor video.pv-file', (v) => ({ t: v.currentTime, paused: v.paused, ready: v.readyState, src: v.getAttribute('src') ?? '', muted: v.muted, volume: v.volume }));
const modeOf = (page) => page.$eval('.screen-monitor', (e) => e.dataset.mode);
const roleOf = (page, sel) => page.$eval(sel, (e) => [...e.classList].find((c) => c.startsWith('role-')));

/**
 * Presses `key` (a keydown on the document, like a real key) and measures in the page how long
 * until `cond(video)` holds (ms), at most 2 s; null when it never did. `key` and `cond` are
 * constant strings of this script (built into functions in the page; nothing from outside).
 */
const timed = (page, key, cond) =>
  page.evaluate(
    ({ key, cond }) =>
      new Promise((resolve) => {
        const v = document.querySelector('.screen-monitor video.pv-file');
        const test = new Function('v', 'before', `return (${cond})`);
        const before = { t: v.currentTime, paused: v.paused, src: v.getAttribute('src') };
        const start = performance.now();
        if (key.startsWith('js:')) new Function(key.slice(3))();
        else document.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
        const poll = () => {
          if (test(v, before)) resolve(performance.now() - start);
          else if (performance.now() - start > 2000) resolve(null);
          else requestAnimationFrame(poll);
        };
        poll();
      }),
    { key, cond },
  );

async function openApp(path = '/admin') {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  await page.goto(base + path);
  page.__errors = errors;
  return page;
}

const lastBody = async (action) => (await requests()).filter((r) => r.path === `/api/admin/channels/main/${action}`).at(-1)?.body;

try {
  await reset();
  let page = await openApp();
  await page.waitForSelector('.player .play');

  // ---------- The Preview plays the film itself, from the engine's position ----------
  await waitFor(async () => (await modeOf(page)) === 'file' && (await video(page)).ready >= 2, 'the Preview plays its file');
  let v = await video(page);
  check(v.src === '/api/desktop/preview/7/media', 'the film of the control item', v.src);
  check(Math.abs(v.t - 92) < 3 && !v.paused, 'starts at the engine position and plays', v);
  const statusAsk = (await requests()).find((r) => r.path === '/api/desktop/preview');
  check(statusAsk?.search === '?item=7', 'asks the engine for the film of the control item', statusAsk);

  // ---------- Layout: Preview main, On air in its corner, no backdrop ----------
  check((await roleOf(page, '.screen-monitor')) === 'role-main', 'the Preview is the main screen');
  check((await roleOf(page, '.screen-air')) === 'role-pip', 'On air is picture-in-picture');
  const geo = await page.evaluate(() => {
    const m = document.querySelector('.screen-monitor .monitor-box').getBoundingClientRect();
    const a = document.querySelector('.screen-air .video').getBoundingClientRect();
    const style = getComputedStyle(document.querySelector('.screen-air .video'));
    return { inside: a.left >= m.left && a.right <= m.right + 1 && a.top >= m.top - 1, small: a.width < m.width / 2, bg: style.backgroundImage, color: style.backgroundColor, overflow: document.documentElement.scrollWidth - innerWidth };
  });
  check(geo.inside && geo.small, 'the small screen sits in the corner of the big one', geo);
  check(geo.bg === 'none' && geo.color === 'rgba(0, 0, 0, 0)', 'no backdrop behind On air', geo);
  check(geo.overflow <= 0, 'nothing wider than the window', geo);
  await page.screenshot({ path: join(shots, 'preview-pip-en.png') });

  // ---------- On air: placed by the air clock, its requests marked ----------
  await waitFor(async () => (await call('/__test/live')).some((r) => r.path.endsWith('.ts')), 'On air loads segments');
  const live = await call('/__test/live');
  check(live.every((r) => r.air === '1'), 'every On air request carries X-VRCinema-Air', live.slice(0, 3));
  // The air clock: the Preview (~92 s) is about 12 s ahead of the typical viewer (80 s).
  const cap = await waitFor(async () => {
    const text = await page.$eval('.screen-monitor .monitor-caption', (e) => e.textContent);
    return /ahead of on-air/.test(text) && text;
  }, 'the Preview says how far ahead of the air it is');
  const ahead = Number(/(\d+) s/.exec(cap)?.[1]);
  check(ahead >= 11 && ahead <= 16, 'the Preview says how far ahead of the air it is', cap);

  // ---------- Instant controls (< 300 ms in the page) ----------
  await sleep(500);
  let ms = await timed(page, ' ', 'v.paused');
  check(ms !== null && ms < 300, `Space pauses the Preview at once (${ms?.toFixed(0)} ms)`);
  await waitFor(() => lastBody('pause'), 'the pause is sent');
  const pausedAt = (await video(page)).t;
  const atS = (await lastBody('pause'))?.atS;
  check(Math.abs(atS - pausedAt) < 0.3, 'the pause carries the Preview position (atS)', { atS, pausedAt });
  await waitFor(async () => (await page.$eval('.player .play', (b) => b.getAttribute('aria-label'))) === 'Resume', 'the bar shows Resume');
  ms = await timed(page, 'k', '!v.paused');
  check(ms !== null && ms < 300, `K resumes at once (${ms?.toFixed(0)} ms)`);
  await sleep(600);
  ms = await timed(page, 'ArrowRight', 'Math.abs(v.currentTime - before.t - 5) < 0.6');
  check(ms !== null && ms < 300, `→ jumps 5 s at once (${ms?.toFixed(0)} ms)`);
  await waitFor(() => lastBody('seek'), 'the seek is sent');
  ms = await timed(page, 'j', 'Math.abs(v.currentTime - before.t + 10) < 0.6');
  check(ms !== null && ms < 300, `J goes 10 s back at once (${ms?.toFixed(0)} ms)`);
  await sleep(300);
  const seekBody = await lastBody('seek');
  const nowAt = (await video(page)).t;
  check(Math.abs(seekBody.toS - nowAt) < 1.5, 'seeks are sent as Preview positions (toS)', { seekBody, nowAt });
  ms = await timed(page, 'Home', 'v.currentTime < 0.6');
  check(ms !== null && ms < 300, `Home restarts at once (${ms?.toFixed(0)} ms)`);
  ms = await timed(page, "js:import('/static/app/store.js').then((m) => m.playNow(8))", "v.getAttribute('src') === '/api/desktop/preview/8/media' && v.readyState >= 1 && v.currentTime < 0.6");
  check(ms !== null && ms < 300, `"Play now" shows the other film at once (${ms?.toFixed(0)} ms)`);
  check((await lastBody('play-now'))?.itemId === 8, 'play-now is sent');
  await sleep(1500);
  check((await video(page)).src === '/api/desktop/preview/8/media', 'the Preview stays on the new film after the engine switched');

  // ---------- The Preview follows the engine where the owner did not act here ----------
  await scenario({ positionS: 150, seekPending: true });
  await waitFor(async () => Math.abs((await video(page)).t - 150) < 2, 'a seek made elsewhere moves the Preview', 5000);
  check(true, 'a seek made elsewhere moves the Preview');
  await scenario({ seekPending: false, paused: true, pauseReasons: ['no_viewers'] });
  await waitFor(async () => (await video(page)).paused, 'a pause for another reason stops the Preview', 4000);
  check(true, 'a pause for another reason stops the Preview');
  await scenario({ paused: false, pauseReasons: [] });
  await waitFor(async () => !(await video(page)).paused, 'it plays on with the channel', 4000);
  check(true, 'it plays on with the channel');

  // ---------- Two volumes, the wheel over the Preview ----------
  check(Boolean(await page.$('.vol-preview input[type=range]')) && Boolean(await page.$('.vol-air input[type=range]')), 'two volume sliders');
  const volBefore = await page.evaluate(() => JSON.parse(localStorage.getItem('vrcinema.monitorVolume') ?? '0.6'));
  const box = await page.$('.screen-monitor .monitor-box');
  const bb = await box.boundingBox();
  await page.mouse.move(bb.x + 40, bb.y + bb.height - 40);
  await page.mouse.wheel(0, -100);
  await sleep(200);
  const volAfter = await page.evaluate(() => JSON.parse(localStorage.getItem('vrcinema.monitorVolume')));
  v = await video(page);
  check(Math.abs(volAfter - volBefore - 0.05) < 0.001 && !v.muted && Math.abs(v.volume - volAfter) < 0.001, 'the wheel over the Preview raises its volume (remembered)', { volBefore, volAfter, v });

  // ---------- Seek-bar thumbnails ----------
  const track = await (await page.$('.track')).boundingBox();
  await page.mouse.move(track.x + track.width * 0.3, track.y + track.height / 2);
  await waitFor(() => page.$('.track img.tl-thumb'), 'a thumbnail over the seek bar', 6000);
  check(true, 'a thumbnail over the seek bar');
  await page.screenshot({ path: join(shots, 'preview-thumb-en.png') });
  await page.mouse.move(10, 10);

  // ---------- Layout changes ----------
  await page.click('.screen-air .video');
  await waitFor(async () => (await roleOf(page, '.screen-air')) === 'role-main', 'a click on the small screen swaps');
  check((await roleOf(page, '.screen-monitor')) === 'role-pip', 'the Preview is then the small one');
  await page.screenshot({ path: join(shots, 'preview-swapped-en.png') });
  await page.hover('.screen-monitor.role-pip');
  await page.click('.screen-monitor .screen-tool[aria-label="Side by side"]');
  await waitFor(async () => (await roleOf(page, '.screen-air')) === 'role-side', 'side by side');
  check((await roleOf(page, '.screen-monitor')) === 'role-side', 'both screens beside each other');
  const airCap = await waitFor(async () => {
    const text = await page.$eval('.screen-air .screen-cap', (e) => e.textContent);
    return /behind the Preview/.test(text) && text;
  }, 'On air says how far behind the Preview it is');
  check(true, `On air says how far behind the Preview it is (${airCap})`);
  await page.reload();
  await page.waitForSelector('.screen-air.role-side');
  check(true, 'the layout is remembered');
  await page.hover('.screen-air');
  await page.click('.screen-air .screen-tool[aria-label="Picture in picture"]');
  await waitFor(async () => (await roleOf(page, '.screen-air')) === 'role-pip', 'back to picture-in-picture');
  check(true, 'back to picture-in-picture');

  // ---------- On air in its own window ----------
  await page.hover('.screen-air');
  const [popup] = await Promise.all([ctx.waitForEvent('page'), page.click('.screen-air .screen-tool[aria-label="Open in its own window"]')]);
  await popup.waitForLoadState();
  check(popup.url().endsWith('/admin?window=air'), 'On air opens in its own window', popup.url());
  await popup.waitForSelector('.air-window .video video');
  await waitFor(() => popup.$eval('.air-window .video video', (x) => x.readyState >= 2), 'the air window plays the channel');
  const bar = await popup.evaluate(() => ({
    slider: document.querySelector('.aw-bar .vol-air input')?.getBoundingClientRect().width ?? 0,
    onTop: Boolean(document.querySelector('.aw-bar .aw-top input[type=checkbox]')),
    picture: document.querySelector('.air-window .video')?.getBoundingClientRect().height ?? 0,
  }));
  check(bar.slider > 30 && bar.onTop && bar.picture > 200, 'the air window: picture, volume, "always on top"', bar);
  await page.waitForSelector('.air-away');
  check(!(await page.$('.screen-air')), 'the main window shows the Preview alone with a note');
  await popup.screenshot({ path: join(shots, 'air-window-en.png') });
  await popup.close();
  await waitFor(() => page.$('.screen-air.role-pip'), 'closing the air window brings On air back', 8000);
  check(true, 'closing the air window brings On air back');

  // ---------- Preparing, then the fallback to segments ----------
  await scenario({ previewSource: { 9: 'preparing' } });
  await page.evaluate(() => import('/static/app/store.js').then((m) => m.playNow(9)));
  await waitFor(async () => /Preparing the video/.test(await page.$eval('.screen-monitor .monitor-box', (e) => e.textContent)), '"Preparing…" while the film downloads');
  check(true, '"Preparing…" while the film downloads');
  // Meanwhile the Preview shows the engine's segments (never behind On air, which may already
  // show the film): the segment player, not the file player.
  await waitFor(async () => (await modeOf(page)) === 'interim', 'the segments meanwhile');
  const shown = await page.evaluate(() => ({
    hls: !document.querySelector('.screen-monitor video.pv-hls').classList.contains('is-hidden'),
    file: !document.querySelector('.screen-monitor video.pv-file').classList.contains('is-hidden'),
    note: Boolean(document.querySelector('.screen-monitor [data-interim]')),
  }));
  check(shown.hls && !shown.file && shown.note, 'the segments play meanwhile, with the note', shown);
  await scenario({ previewSource: { 9: 'ready' } });
  await waitFor(async () => (await video(page)).src === '/api/desktop/preview/9/media' && (await video(page)).ready >= 1, 'the film plays once ready', 6000);
  check(true, 'the film plays once ready');
  await waitFor(async () => (await modeOf(page)) === 'file', 'the file player takes over');
  check(true, 'then the file player takes over from the segments');
  await scenario({ previewSource: { 10: 'unavailable' } });
  await page.evaluate(() => import('/static/app/store.js').then((m) => m.playNow(10)));
  await waitFor(async () => (await modeOf(page)) === 'fallback', 'no file: the segment Preview instead');
  check(Boolean(await page.$('.monitor-fallback')), 'with a note that says so');

  // ---------- Keys, icon, language ----------
  await page.keyboard.press('?');
  await page.waitForSelector('.dialog .shortcuts');
  const keys = await page.$eval('.dialog .shortcuts', (e) => e.textContent);
  check(/5 seconds/.test(keys) && /J/.test(keys) && /Mouse wheel/.test(keys), 'the shortcuts list J/K/L, ±5 s and the wheel', keys);
  await page.keyboard.press('Escape');
  const icon = await page.$eval('.titlebar img.app-icon', (i) => i.complete && i.naturalWidth);
  check(icon > 0, 'the title bar shows the app icon', icon);
  await page.evaluate(() => import('/static/app/i18n.js').then((m) => m.loadLang('pl')));
  await page.hover('.screen-monitor');
  await waitFor(async () => /fragmentów kanału/.test(await page.$eval('.screen-monitor', (e) => e.textContent)), 'Polish texts');
  check(true, 'the new texts switch language (pl)');
  await page.screenshot({ path: join(shots, 'preview-fallback-pl.png') });

  check(page.__errors.filter((e) => !/favicon|ERR_ABORTED|net::|404/.test(e)).length === 0, 'no page errors', page.__errors);
  console.log(`\n${passed} checks passed`);
} catch (e) {
  console.error(e.message);
  process.exitCode = 1;
} finally {
  await ctx.close();
  await engine.close();
  rmSync(profile, { recursive: true, force: true });
  film.cleanup();
  stream.cleanup();
}
