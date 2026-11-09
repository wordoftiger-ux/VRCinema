// Browser check of the mini player, the film-perforation background and the Preview of music:
// away from Channel the same Preview and On air players keep playing in a small window in the
// bottom right corner (not remounted: the same <video> elements, times advancing), its button
// mutes both, a click goes back to Channel; the open view can scroll its last control clear of
// it; the app's background has the perforation (not the viewer page); an audio-only film in
// the Preview shows its title, artist, time, "Up next" and moving bars. Against the fake engine
// (a real MP4 film, a real audio-only MP4 and a real HLS stream made by bin/ffmpeg.exe), headless
// Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/mini-player.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';
import { makeTestFilm, makeTestSong, makeTestStream } from './hls-fixture.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8231);
if (!shots) throw new Error('usage: mini-player.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(fn, what, timeout = 8000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

const film = await makeTestFilm();
const song = await makeTestSong();
const stream = await makeTestStream();
const base = `http://127.0.0.1:${port}`;
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1280, height: 760 },
  args: ['--disable-extensions', '--no-first-run', '--autoplay-policy=no-user-gesture-required'],
});
await ctx.addInitScript(() => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
});

/** The players' state, each <video> tagged once so a new element is told from the old one. */
const players = (page) =>
  page.evaluate(() => {
    const one = (sel) => {
      const v = document.querySelector(sel);
      if (!v) return null;
      v.dataset.tag ??= String(Math.random());
      return { tag: v.dataset.tag, t: v.currentTime, paused: v.paused, muted: v.muted, ready: v.readyState };
    };
    return { preview: one('.screen-monitor video.pv-file'), air: one('.screen-air video') };
  });

let engine = await startFakeEngine(port, { hlsDir: stream.dir, previewFile: film.file });
try {
  await call('/__test/reset', {});
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/admin#/channel`);
  await waitFor(async () => {
    const p = await players(page);
    return p.preview?.ready >= 2 && p.air?.ready >= 2 && !p.preview.paused && !p.air.paused;
  }, 'both screens play', 15000);
  // A click: the window's sound starts (the meters' tap) like after the owner's first click.
  await page.mouse.click(5, 300);
  const before = await players(page);

  // ---------- Perforation in the app's background ----------
  const bg = await page.$eval('.app > .body', (e) => getComputedStyle(e).backgroundImage);
  check(bg.includes('radial-gradient'), 'the app background has the film perforation', bg);
  await page.screenshot({ path: join(shots, 'mini-channel-en.png') });

  // ---------- Another tab: the same players, small, still playing ----------
  await page.click('a[href="#/settings"], a[href="#/settings/general"]');
  await page.waitForSelector('.mini-player');
  check(await page.$eval('#view', (e) => e.dataset.view) === 'settings', 'the Settings view is open');
  await sleep(1500);
  const away = await players(page);
  check(away.preview.tag === before.preview.tag && away.air.tag === before.air.tag, 'the players are the same elements (not remounted)', { before, away });
  check(!away.preview.paused && !away.air.paused, 'both keep playing', away);
  check(away.preview.t > before.preview.t + 1 && away.air.t > before.air.t + 0.5, 'their time moves on', { before, away });
  const box = await page.$eval('.mini-player', (e) => {
    const r = e.getBoundingClientRect();
    const bar = document.querySelector('.player').getBoundingClientRect();
    return { right: innerWidth - r.right, bottom: r.bottom, barTop: bar.top, width: r.width, height: r.height };
  });
  check(box.right >= 8 && box.right <= 24 && box.bottom <= box.barTop - 8, 'docked in the bottom right corner, above the player bar', box);
  check(box.width <= 330 && box.height <= 190, 'small', box);
  check((await page.$$('.mini-player .q, .mini-player .screen-actions:not([hidden])')).length === 0 || (await page.$eval('.mini-player', (e) => !e.querySelector('.q'))), 'no queue in the mini player');
  // The open view scrolls its last control clear of the mini player.
  const clear = await page.evaluate(() => {
    const view = document.querySelector('#view');
    view.scrollTop = view.scrollHeight;
    const mini = document.querySelector('.mini-player').getBoundingClientRect();
    const controls = [...view.querySelectorAll('button, input, select, a[href], textarea')].filter((c) => c.getClientRects().length);
    const last = controls.at(-1)?.getBoundingClientRect();
    return last ? { lastBottom: last.bottom, lastRight: last.right, miniTop: mini.top, miniLeft: mini.left } : null;
  });
  check(clear && (clear.lastBottom <= clear.miniTop || clear.lastRight <= clear.miniLeft), 'the last control of the view is not under the mini player', clear);
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'no horizontal overflow');
  await page.screenshot({ path: join(shots, 'mini-settings-en.png') });

  // Scrolling through a long section: the mini player covers no control while some corner is free
  // (bottom right at the top of the list, top right where the bottom one would cover something).
  await page.evaluate(() => { location.hash = '#/settings/channel'; });
  await page.waitForSelector('#view[data-view="settings"]');
  await sleep(400);
  const sweep = await page.evaluate(async () => {
    const view = document.querySelector('#view');
    const scroller = [...document.querySelectorAll('#view, #view *')].reduce((best, el) => (el.scrollHeight - el.clientHeight > (best?.scrollHeight ?? 0) - (best?.clientHeight ?? 0) ? el : best), view);
    const sel = 'button, input, select, textarea, a[href], summary, [role="switch"], [role="button"], [role="tab"], label';
    const hits = (rect, mini) => [...view.querySelectorAll(sel)].filter((el) => {
      if (mini.contains(el)) return false;
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.height > 0 && r.left < rect.right && r.right > rect.left && r.top < rect.bottom && r.bottom > rect.top;
    }).length;
    const out = { corners: new Set(), bad: [], steps: 0 };
    const max = scroller.scrollHeight - scroller.clientHeight;
    for (let y = 0; y <= max + 1; y += Math.max(60, Math.floor(max / 25))) {
      scroller.scrollTop = y;
      await new Promise((r) => setTimeout(r, 120)); 
      const mini = document.querySelector('.mini-player');
      const r = mini.getBoundingClientRect();
      out.corners.add(mini.dataset.corner ?? 'bottom');
      out.steps += 1;
      if (hits(r, mini) > 0) {
        const was = mini.dataset.corner;
        mini.dataset.corner = was === 'top' ? 'bottom' : 'top';
        const o = mini.getBoundingClientRect();
        if (was === undefined) delete mini.dataset.corner; else mini.dataset.corner = was;
        if (hits({ left: o.left, right: o.right, top: o.top, bottom: o.bottom }, mini) === 0) out.bad.push({ y, corner: was ?? 'bottom', rb: [r.left, r.top, r.right, r.bottom].map(Math.round), ro: [o.left, o.top, o.right, o.bottom].map(Math.round), vt: Math.round(view.getBoundingClientRect().top), cov: [...view.querySelectorAll(sel)].filter((el) => { const q = el.getBoundingClientRect(); return !mini.contains(el) && q.width > 0 && q.height > 0 && q.left < r.right && q.right > r.left && q.top < r.bottom && q.bottom > r.top; }).slice(0, 2).map((el) => el.tagName + '.' + el.className + ' ' + Math.round(el.getBoundingClientRect().left) + ',' + Math.round(el.getBoundingClientRect().top) + ',' + Math.round(el.getBoundingClientRect().right) + ',' + Math.round(el.getBoundingClientRect().bottom)) });
      }
    }
    out.corners = [...out.corners];
    return out;
  });
  check(sweep.steps > 3 && sweep.bad.length === 0, 'the mini player never covers a control while the other corner is free', sweep);
  console.log('  corners used while scrolling:', sweep.corners.join(', '));
  await page.evaluate(() => { location.hash = '#/settings/general'; });
  await sleep(300);

  // ---------- Mute, then back to Channel with a click ----------
  await page.click('.mini-mute');
  await waitFor(async () => {
    const p = await players(page);
    return p.preview.muted && p.air.muted;
  }, 'the mini player mutes both screens');
  check(true, 'the mini player mutes both screens');
  check(await page.$eval('.mini-mute', (b) => b.getAttribute('aria-pressed')) === 'true', 'its button says so');
  await page.click('.mini-back');
  await page.waitForSelector('#view[data-view="channel"]');
  check(!(await page.$('.mini-player')), 'a click returns to Channel');
  const back = await players(page);
  check(back.preview.tag === before.preview.tag && back.air.tag === before.air.tag, 'still the same players on Channel', back);
  check(!back.air.muted, 'On air has its own volume back on Channel', back);
  check(await page.$eval('.screen-air', (e) => e.classList.contains('role-pip')), 'the picture-in-picture layout stays');

  // ---------- Phone width: the mini player above the tab bar ----------
  await page.setViewportSize({ width: 390, height: 760 });
  await page.goto(`${base}/admin#/history`);
  await page.waitForSelector('.mini-player');
  await waitFor(() => page.$eval('.mini-player video', (v) => v.readyState >= 2 && !v.paused), 'On air plays in the phone\'s mini player', 15000);
  const phone = await page.$eval('.mini-player', (e) => {
    const r = e.getBoundingClientRect();
    const rail = document.querySelector('.rail').getBoundingClientRect();
    return { bottom: r.bottom, railTop: rail.top, width: r.width, right: innerWidth - r.right };
  });
  check(phone.bottom <= phone.railTop && phone.width < 200, 'on a phone: small, above the tab bar', phone);
  const muteSize = await page.$eval('.mini-mute', (b) => b.getBoundingClientRect().width);
  check(muteSize >= 44, 'its button is a 44 px target on a phone', muteSize);
  check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'no horizontal overflow on a phone');
  await page.screenshot({ path: join(shots, 'mini-history-phone.png') });
  check(errors.length === 0, 'no page errors', errors);
  await page.close();

  // ---------- The viewer page has no perforation ----------
  const viewer = await ctx.newPage();
  await viewer.goto(`${base}/`);
  await viewer.waitForLoadState('domcontentloaded');
  await sleep(500);
  check(await viewer.evaluate(() => !document.querySelector('.app > .body') && !getComputedStyle(document.body).backgroundImage.includes('radial-gradient')), 'the viewer page has no perforation');
  await viewer.close();
} finally {
  await engine.close?.();
}

// ---------- The Preview of an audio-only film ----------
engine = await startFakeEngine(port, { hlsDir: stream.dir, previewFile: song.file });
try {
  await call('/__test/reset', {});
  await call('/__test/scenario', { audioMeta: { title: 'Żółta piosenka', artist: 'Zespół' } });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.setViewportSize({ width: 1280, height: 760 });
  await page.goto(`${base}/admin#/channel`);
  await page.waitForSelector('[data-audio-screen]', { timeout: 15000 });
  await page.mouse.click(5, 300);
  const text = () => page.$eval('[data-audio-screen]', (e) => e.innerText);
  await waitFor(async () => (await text()).includes('Żółta piosenka') && (await text()).includes('Zespół'), 'title and artist from the engine');
  check(true, 'the Preview of music shows the title and the artist');
  check((await text()).includes('Sound only'), 'it says the film is sound only');
  const first = await page.evaluate(async () => {
    const store = await import('/static/app/store.js');
    return store.queue.value.find((i) => i.id !== store.control.value?.itemId)?.title ?? null;
  });
  check(first && (await text()).includes(`Up next: ${first}`), 'Up next names the next film of the queue', { first, text: await text() });
  const t1 = await page.$eval('[data-as-time]', (e) => e.textContent);
  check(/^\d+:\d\d \/ 2:00$/.test(t1), 'elapsed and total time', t1);
  await waitFor(async () => (await page.$eval('[data-audio-screen] canvas', (c) => c.dataset.live)) === 'yes', 'the bars read the sound');
  check(true, 'the bars read the film\'s own sound');
  const frame = () => page.$eval('[data-audio-screen] canvas', (c) => c.toDataURL());
  const a = await frame();
  await sleep(400);
  const b = await frame();
  check(a !== b, 'the bars move');
  await page.screenshot({ path: join(shots, 'preview-audio-en.png') });
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await waitFor(async () => (await text()).includes('Tylko dźwięk'), 'Polish texts');
  check(!/\b(visual|preview|mini)\.[a-z_]+/.test(await text()), 'no raw keys (pl)');
  await page.screenshot({ path: join(shots, 'preview-audio-pl.png') });
  // In the mini player: the compact music screen keeps moving.
  await page.goto(`${base}/admin#/share`);
  await page.waitForSelector('.mini-player [data-audio-screen]');
  await page.screenshot({ path: join(shots, 'mini-audio-share.png') });
  check(errors.length === 0, 'no page errors', errors);
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
  film.cleanup();
  song.cleanup();
  stream.cleanup();
}
process.exit(0);
