// Browser check of the live preview and the "Stats for nerds" overlay against the fake engine
// (which serves a short real HLS stream made by bin/ffmpeg.exe), in headless Brave with a
// temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/preview-stats.mjs <shots dir> [port]
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
const port = Number(process.argv[3] ?? 8195);
if (!shots) throw new Error('usage: preview-stats.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}

const stream = await makeTestStream();
const engine = await startFakeEngine(port, { hlsDir: stream.dir });
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  viewport: { width: 1100, height: 680 },
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
const statsReads = async () => (await requests()).filter((r) => r.path.endsWith('/channels/main/stats')).length;
const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const delayOf = (page) => page.evaluate(async () => (await import('/static/app/store.js')).previewDelayS.value);
const rowText = (page, label, nth = 0) =>
  page.evaluate(([l, n]) => {
    const rows = [...document.querySelectorAll('.stats-row')].filter((r) => r.firstElementChild.textContent.trim() === l);
    return rows[n] ? rows[n].lastElementChild.textContent.trim() : null;
  }, [label, nth]);

const setLayout = (page, layout) =>
  page.evaluate(async (l) => { (await import('/static/app/components/screens.js')).airLayout.value = l; }, layout);
/**
 * On air's caption with On air as the main screen (`swapped`): as the small screen it says just
 * "On air" (the figure goes to the Preview's caption, spec deviation 4). Back to `pip` after.
 */
async function mainAirCaption(page, re) {
  await setLayout(page, 'swapped');
  let caption = '';
  try {
    await waitFor(async () => re.test((caption = await text(page, '.screen-air .stage-caption'))), `On air caption ${re}`);
  } catch {
    // The check below reports what it said.
  }
  await setLayout(page, 'pip');
  return caption;
}

async function waitFor(fn, what, timeout = 8000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

async function openApp() {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  page.on('response', (r) => r.status() >= 400 && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${base}/admin`);
  await page.waitForSelector('.player .play');
  await page.waitForSelector('.qi');
  page.__errors = errors;
  return page;
}

try {
  await reset();
  let page = await openApp();

  // ---------- The preview ----------
  await waitFor(async () => Number.isFinite(await delayOf(page)), 'the preview measures its delay');
  const video = await page.$eval('.video video', (v) => ({ w: v.videoWidth, h: v.videoHeight, t: v.currentTime }));
  check(video.w === 320 && video.h === 180, 'the preview plays the channel stream (320x180 from /live/main.m3u8)', video);
  await waitFor(() => page.$eval('.video video', (v) => v.currentTime > 0.5), 'the preview moves on');
  check(true, 'the preview is playing (the picture moves on)');
  const delay = await delayOf(page);
  check(delay >= 0 && delay < 20, 'previewDelayS is a measured number of seconds', delay);
  check((await text(page, '.screen-air .stage-caption')) === 'On air', 'On air as the small screen: its caption is just "On air"');
  const caption = await mainAirCaption(page, /^On air · about \d+ s behind the server$/);
  check(/^On air · about \d+ s behind the server$/.test(caption), 'caption of the On air screen (main) carries the measured delay', caption);
  check(Math.abs(Number(caption.match(/\d+/)[0]) - (await delayOf(page))) <= 2, 'the caption number is the measured delay, rounded');
  await page.screenshot({ path: join(shots, 'after-preview-en-1100x680.png') });

  // The autoplay fallback keeps the stored mute choice in step with the video.
  const sync = await page.evaluate(async () => {
    const { previewMuted, previewVolume } = await import('/static/app/components/playerbar.js');
    return { video: document.querySelector('.video video').muted, signal: previewMuted.value || previewVolume.value === 0 };
  });
  check(sync.video === sync.signal, 'the video is muted exactly when previewMuted says so (autoplay fallback included)', sync);

  // Never "0 s": a measurement is a number above zero or nothing.
  const samples = [];
  for (let i = 0; i < 6; i += 1) {
    samples.push(await delayOf(page));
    await sleep(500);
  }
  check(samples.every((x) => x === null || x > 0), 'the measured delay is never 0 or negative', samples);

  // A hidden window stops the preview loading (so it is no viewer) and resumes when shown.
  const liveRequests = [];
  page.on('request', (r) => r.url().includes('/live/') && liveRequests.push(Date.now()));
  const setHidden = (hidden) =>
    page.evaluate((h) => {
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
      document.dispatchEvent(new Event('visibilitychange'));
    }, hidden);
  await setHidden(true);
  await sleep(500); // a request already on its way may still land
  const during = liveRequests.length;
  await sleep(4500);
  check(liveRequests.length === during, 'hidden window: no more requests for the stream (0 in 4.5 s)', liveRequests.length - during);
  check(await page.$eval('.video video', (v) => v.paused), 'hidden window: the video is paused');
  check((await delayOf(page)) === null, 'hidden window: the delay is unknown, not a stale number');
  await setHidden(false);
  await waitFor(() => page.$eval('.video video', (v) => !v.paused && v.currentTime > 0), 'the video plays again');
  await waitFor(async () => liveRequests.length > during, 'requests resume');
  await waitFor(async () => Number.isFinite(await delayOf(page)), 'the delay is measured again');
  check(true, 'visible again: loading and playing resume, the delay is measured again');

  // Volume and mute: the signals drive the video, the choice is remembered in localStorage.
  await page.click('.vol-air .icon-button');
  await waitFor(() => page.$eval('.video video', (e) => e.muted), 'the video is muted');
  let v = await page.$eval('.video video', (e) => ({ muted: e.muted }));
  check(v.muted === true && (await page.evaluate(() => localStorage.getItem('vrcinema.previewMuted'))) === 'true', 'mute: the video is muted and the choice is stored', v);
  await page.click('.vol-air .icon-button');
  await waitFor(() => page.$eval('.video video', (e) => !e.muted), 'the video is unmuted');
  v = await page.$eval('.video video', (e) => ({ muted: e.muted, volume: e.volume }));
  check(v.muted === false && (await page.evaluate(() => localStorage.getItem('vrcinema.previewMuted'))) === 'false', 'unmute: the video has its sound again and the choice is stored', v);
  await page.$eval('.vol-air input[type=range]', (e) => {
    e.value = '0.3';
    e.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await waitFor(() => page.$eval('.video video', (e) => Math.abs(e.volume - 0.3) < 0.01), 'the volume is set');
  v = await page.$eval('.video video', (e) => e.volume);
  check(Math.abs(v - 0.3) < 0.01 && (await page.evaluate(() => localStorage.getItem('vrcinema.previewVolume'))) === '0.3', 'volume slider sets the preview volume and is stored', v);

  // ---------- The context menu ----------
  check((await page.$('.stats')) === null, 'the overlay is closed at the start');
  let reads0 = await statsReads();
  check(reads0 === 0, 'closed overlay: no read of the stats route', reads0);
  await page.click('.video', { button: 'right' });
  await page.waitForSelector('.menu');
  const items = await page.$$eval('.menu [role=menuitem]', (b) => b.map((e) => [e.textContent.trim(), e.getAttribute('aria-disabled')]));
  check(JSON.stringify(items.map((i) => i[0])) === JSON.stringify(['Pause', 'Back 10 seconds', 'Forward 10 seconds', 'Stats for nerds', 'Copy channel link']), 'right click on the preview opens the menu: pause, -10 s, +10 s, stats, copy link', items);
  check(items.every((i) => i[1] === 'false'), 'all the menu items are enabled while a film plays');
  check((await page.$eval('.menu', (m) => m.getAttribute('aria-label'))) === 'On air menu', 'the menu has its own name');

  // Menu: pause and seek reach the engine; copy puts the channel link on the clipboard.
  let m = (await requests()).length;
  await page.click('.menu button:nth-of-type(1)');
  await waitFor(async () => (await requests()).slice(m).some((r) => r.path.endsWith('/pause')), 'menu: Pause sends pause');
  check(true, 'menu: Pause sends pause');
  await page.click('.video', { button: 'right' });
  check((await text(page, '.menu [role=menuitem]')) === 'Resume', 'while paused the first item is Resume');
  await page.click('.menu [role=menuitem]');
  await waitFor(async () => (await requests()).slice(m).some((r) => r.path.endsWith('/resume')), 'menu: Resume sends resume');
  check(true, 'menu: Resume sends resume');
  // The seek bubble shows on On air as the main screen only (not as the small one): swap.
  await setLayout(page, 'swapped');
  await page.waitForSelector('.screen-air.role-main');
  await page.click('.video', { button: 'right' });
  await page.click('.menu button:nth-of-type(3)'); // +10 s
  await waitFor(async () => (await requests()).slice(m).find((r) => r.path.endsWith('/seek') && r.body?.byS === 10), 'menu: +10 s sends seek byS 10');
  check(true, 'menu: +10 s sends seek byS 10');
  await page.click('.video', { button: 'right' });
  await page.click('.menu button:nth-of-type(5)'); // copy link
  check((await page.evaluate(() => navigator.clipboard.readText())) === `${base}/live/main.m3u8` || (await page.evaluate(() => navigator.clipboard.readText())).endsWith('/live/main.m3u8'), 'menu: Copy channel link copies the channel address');

  // ---------- The seek bubble ----------
  const bubble = await waitFor(() => page.$('.seek-bubble'), 'the bubble appears after a seek');
  const bubbleText = (await bubble.textContent()).replace('⏩', '').trim();
  const n = Number(bubbleText.match(/about (\d+) s/)?.[1]);
  const d2 = await delayOf(page);
  check(Number.isFinite(n) && n >= 1 && Math.abs(n - d2) <= 2, 'bubble after the seek: "Viewers will see this in about N s", N from the measurement', { bubbleText, d2 });

  // Rounding and the unknown delay, set by hand (retried: a measurement tick may land in between).
  const bubbleFor = (delayValue) =>
    page.evaluate(async (dv) => {
      const store = await import('/static/app/store.js');
      for (let i = 0; i < 8; i += 1) {
        store.previewDelayS.value = dv;
        store.seekBubble.value += 1;
        await new Promise((r) => setTimeout(r, 80));
        const el = document.querySelector('.seek-bubble');
        const txt = el?.textContent.replace('⏩', '').trim();
        const want = dv === null ? 'Viewers will see this in a few seconds' : `Viewers will see this in about ${Math.max(1, Math.round(dv))} s`;
        if (txt === want) return txt;
        if (i === 7) return txt ?? null;
      }
      return null;
    }, delayValue);
  check((await bubbleFor(11.8)) === 'Viewers will see this in about 12 s', 'the bubble rounds the delay (11.8 s -> 12 s)');
  await page.evaluate(async () => (await import('/static/app/store.js')).previewDelayS.value = 3);
  await sleep(150);
  check((await text(page, '.seek-bubble')).endsWith('about 12 s'), 'the bubble keeps the number from the moment of the seek (not the live delay)', await text(page, '.seek-bubble'));
  const fallback = await bubbleFor(null);
  check(/^Viewers will see this in about \d+ s$/.test(fallback), 'with no current measurement the bubble uses the last good one', fallback);
  await waitFor(async () => (await page.$('.seek-bubble')) === null, 'the bubble goes away by itself', 7000);
  check(true, 'the bubble goes away after a few seconds');

  // ---------- The overlay ----------
  await page.click('.video', { button: 'right' });
  await page.click('.menu button:nth-of-type(4)'); // Stats for nerds
  await page.waitForSelector('.stats');
  check(true, 'menu: "Stats for nerds" opens the overlay');
  await waitFor(async () => (await rowText(page, 'Watching')) === '≈ 4', 'the overlay shows the answer of the stats route');
  const titles = await page.$$eval('.stats h5', (h) => h.map((e) => e.textContent.trim()));
  check(JSON.stringify(titles) === JSON.stringify(['On air', 'HLS channel', 'Next video', 'ffmpeg', 'Viewers', 'Internet (tunnel)', 'Playback in this window']), 'the groups follow the mockup', titles);
  const expect = {
    Video: 'Never Gonna Give You Up · youtube.com',
    'Server position': '1:42.0 / 3:33.0 · attempt 1',
    Source: '1080p · 25 fps · h264 · yuv420p · audio',
    Mode: 'stream copy (no conversion)',
    'Server buffer': '5 ready (about 20 s)',
    Sequence: '#18,342 · window 10 · segment 4 s · discontinuities 37',
    Pause: 'no',
    Process: 'PID 18872 · 1.9× real time · — · RAM 84 MB',
    Encoder: '—',
    'Segment fetches': '58/min · 1.4 MB/s sent',
    Connections: '4 to Cloudflare (WAW01, FRA02) · reconnects 1',
    Address: 'https://proba-kanalu.trycloudflare.com',
  };
  for (const [label, want] of Object.entries(expect)) {
    const got = await rowText(page, label);
    check(got === want, `row "${label}": ${want}`, got);
  }
  check((await rowText(page, 'Timestamps')) === 'PTS 1,482,900 s · continuous ✓', 'row "Timestamps": PTS · continuous ✓', await rowText(page, 'Timestamps'));
  check((await page.$$('.stats-bars i')).length === 60, 'the fetches row has a bar chart of 60 samples');
  check(/^[\d.]+ s behind the server · buffer [\d.]+ s/.test(await rowText(page, 'Delay')), 'row "Delay": measured by the preview (delay, buffer, dropped frames)', await rowText(page, 'Delay'));
  check(/dropped frames \d+$/.test(await rowText(page, 'Delay')), 'dropped frames come from the video element');
  check((await rowText(page, 'State', 1)) === 'online · online for 1 h 12 min', 'row "State" of the tunnel', await rowText(page, 'State', 1));
  check((await rowText(page, 'State', 0)) === 'ready · 3 ready (about 12 s)', 'row "State" of the next video', await rowText(page, 'State', 0));
  await page.screenshot({ path: join(shots, 'after-stats-en-1100x680.png') });

  // Polling: once a second while open; none after closing.
  let a = await statsReads();
  await sleep(2600);
  let b = await statsReads();
  check(b - a >= 2 && b - a <= 4, 'open overlay reads the stats about once a second', b - a);
  await page.click('.stats .icon-button');
  check((await page.$('.stats')) === null, 'the x closes the overlay');
  await sleep(300); // a read already on its way may still land
  a = await statsReads();
  await sleep(2600);
  b = await statsReads();
  check(b === a, 'closed overlay: the reads stop (0 in 2.6 s)', b - a);

  // The key I; not while typing.
  await page.click('.stage-caption'); // focus leaves everything
  await page.keyboard.press('i');
  await page.waitForSelector('.stats');
  check(true, 'key I opens the overlay');
  await page.keyboard.press('I');
  check((await page.$('.stats')) === null, 'key I closes it');
  await page.focus('.addf-input');
  await page.keyboard.press('i');
  check((await page.$('.stats')) === null && (await page.$eval('.addf-input', (e) => e.value)) === 'i', 'key I typed in the link field does not open the overlay');
  await page.evaluate(() => document.activeElement.blur());

  // Nothing known: "—" for every null, the viewers group still has its numbers.
  await scenario({ stats: 'nulls' });
  await page.keyboard.press('i');
  await page.waitForSelector('.stats');
  await waitFor(async () => (await rowText(page, 'Video')) === '—', 'null onAir shows a dash');
  const dashes = {};
  for (const l of ['Video', 'Server position', 'Source', 'Mode', 'Server buffer', 'Sequence', 'Timestamps', 'Process', 'Encoder', 'Connections', 'Address']) dashes[l] = await rowText(page, l);
  check(Object.values(dashes).every((x) => x === '—'), 'every value that is null shows "—"', dashes);
  check((await rowText(page, 'Watching')) === '≈ 0', 'zero is shown as a number, not a dash');
  await page.screenshot({ path: join(shots, 'after-stats-nulls-en-1100x680.png') });
  await scenario({ stats: 'full' });
  await waitFor(async () => (await rowText(page, 'Watching')) === '≈ 4', 'the overlay follows the engine again');

  // Copy: all the groups, as text.
  await page.click('.stats-copy');
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  check(/\[\d+( \d+){59}\]/.test(copied), 'the copied text carries the 60 samples of the viewers history');
  const missing = ['On air', 'HLS channel', 'Next video', 'ffmpeg', 'Viewers', 'Internet (tunnel)', 'Playback in this window', 'Never Gonna Give You Up', 'PID 18872', '58/min', 'WAW01', 'Darude — Sandstorm', 'Delay:'].filter((x) => !copied.includes(x));
  check(missing.length === 0, 'Copy puts every group on the clipboard as text', { missing, copied });
  check(copied.split('\n').length > 20 && !/<|\[object/.test(copied), 'the copied text is plain lines (group heading, indented "label: value")');
  check((await text(page, '.stats-copy')) === 'Copied', 'the button confirms with "Copied"');

  // Language switch with the overlay open.
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await waitFor(async () => (await text(page, '.stats-head b')) === 'Statystyki dla nerdów', 'the overlay switches to Polish');
  const plTitles = await page.$$eval('.stats h5', (h) => h.map((e) => e.textContent.trim()));
  check(plTitles[0] === 'Teraz na antenie' && plTitles.at(-1) === 'Odtwarzanie w tym oknie', 'group headings follow the language', plTitles);
  check((await rowText(page, 'Bufor serwera')) === '5 gotowych (ok. 20 s)', 'plural (many) in the buffer row', await rowText(page, 'Bufor serwera'));
  check((await rowText(page, 'Stan', 0)).includes('3 gotowe'), 'plural (few) in the next video row', await rowText(page, 'Stan', 0));
  check((await rowText(page, 'Pozycja serwera')) === '1:42.0 / 3:33.0 · próba 1', 'row values follow the language', await rowText(page, 'Pozycja serwera'));
  const raw = await page.$eval('.stage', (e) => e.textContent.match(/\b(stats|preview)\.[a-z_0-9]+/g));
  check(raw === null, 'no untranslated keys in the stage', raw);
  const plCaption = await mainAirCaption(page, /^Na antenie · ok\. \d+ s za serwerem$/);
  check(/^Na antenie · ok\. \d+ s za serwerem$/.test(plCaption), 'the caption follows the language', plCaption);
  await page.screenshot({ path: join(shots, 'after-stats-pl-1100x680.png') });
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('en'));

  // Leaving the view: the overlay and its reads stop; On air plays on in the mini player.
  await page.click('.rail-history');
  await waitFor(async () => (await page.$('.stats')) === null, 'leaving the Channel view removes the overlay');
  await sleep(300);
  a = await statsReads();
  await sleep(2200);
  b = await statsReads();
  check(b === a, 'leaving the Channel view stops the stats reads', b - a);
  check(Number.isFinite(await delayOf(page)), 'On air plays on in the mini player (its delay is still measured)');
  await page.keyboard.press('i');
  check((await page.$('.stats')) === null, 'key I does nothing in the mini player');
  check(page.__errors.length === 0, 'no errors in the console', page.__errors);
  await page.close();

  // ---------- A source analysis with holes ----------
  await reset();
  await scenario({ stats: 'partial' });
  page = await openApp();
  await page.keyboard.press('i');
  await waitFor(async () => (await rowText(page, 'Source')) !== null && (await rowText(page, 'Watching')) === '≈ 4', 'the partial stats arrive');
  check((await rowText(page, 'Source')) === '— · — · h264 · yuv420p · audio', 'a source without height and frame rate: dashes for those, no "nullp"', await rowText(page, 'Source'));
  check((await rowText(page, 'Server position')) === '1:32.0 / — · attempt 1', 'an unknown length is a dash', await rowText(page, 'Server position'));
  check((await rowText(page, 'Server buffer')) === '5 ready (about 20 s)', 'the plural form for 5 in English');
  check(page.__errors.length === 0, 'no errors with partial stats', page.__errors);
  await page.close();

  // ---------- Without a stream ----------
  await reset();
  await scenario({ hls: false });
  page = await openApp();
  await sleep(1500);
  check((await text(page, '.stage-caption')) === 'On air', 'no stream: the caption is just "On air"');
  check((await delayOf(page)) === null, 'no stream: the delay stays unknown');
  await setLayout(page, 'swapped'); // the bubble shows on On air as the main screen
  await page.waitForSelector('.screen-air.role-main');
  await page.evaluate(async () => { (await import('/static/app/store.js')).seekBubble.value += 1; });
  await waitFor(() => page.$('.seek-bubble'), 'bubble without any measurement');
  check((await text(page, '.seek-bubble')).endsWith('Viewers will see this in a few seconds'), 'no measurement at all: the bubble says "a few seconds"');
  // It gives up after a few tries (no endless loop) and offers a retry.
  const playlistReads = [];
  page.on('request', (r) => r.url().includes('/live/main.m3u8') && playlistReads.push(Date.now()));
  await waitFor(() => page.$('.preview-unavailable'), 'the preview gives up', 40000);
  check((await text(page, '.preview-unavailable span')) === 'On air unavailable', 'after several failed tries: "On air unavailable"');
  const reads = playlistReads.length;
  await sleep(4500);
  check(playlistReads.length === reads, 'once given up it stops requesting the stream', playlistReads.length - reads);
  await page.screenshot({ path: join(shots, 'after-preview-unavailable-en-1100x680.png') });
  await scenario({ hls: true });
  await page.click('.preview-unavailable .btn');
  await waitFor(async () => Number.isFinite(await delayOf(page)), 'the retry plays the stream');
  check((await page.$('.preview-unavailable')) === null, 'Try again plays the stream and the message goes away');
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
  stream.cleanup();
}
