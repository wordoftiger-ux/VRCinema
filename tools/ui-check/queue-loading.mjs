// Browser check of the queue's pictures and the "Now loading" row: a row shows its film's
// picture (the engine's `thumb`), a picture that does not load falls back to the coloured box,
// a row without one keeps the box; while a film starts the top of the list says what loads, who
// added it, the stage and the seconds (or the percentage when known), and the row goes when
// the film is on air. Against the fake engine, in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/queue-loading.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8213);
if (!shots) throw new Error('usage: queue-loading.mjs <shots dir> [port]');
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
  viewport: { width: 1280, height: 760 },
  args: ['--disable-extensions', '--no-first-run'],
});
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const scenario = (s) => call('/__test/scenario', s);
const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};

try {
  await call('/__test/reset', {});
  // Row 8: a picture that loads; 9 (a disk film): the engine's frame; 10: a picture that fails;
  // 11: none.
  await scenario({
    thumbs: { 8: `${base}/api/admin/items/8/thumb`, 9: '/api/admin/items/9/thumb', 10: '/api/admin/items/10/thumb' },
    thumbFail: [10],
  });
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/admin`);
  await page.waitForSelector('.qlist li[data-id="8"]');
  const rowPic = (id) =>
    page.$eval(`.qlist li[data-id="${id}"] .th`, (th) => {
      const img = th.querySelector('img');
      return { pic: th.classList.contains('th-pic'), loaded: img ? img.complete && img.naturalWidth > 0 : null, referrer: img?.referrerPolicy ?? null };
    });
  await waitFor(async () => (await rowPic(8)).loaded && (await rowPic(9)).loaded, 'pictures load');
  const eight = await rowPic(8);
  check(eight.pic && eight.referrer === 'no-referrer', 'a row shows its picture, loaded without a referrer', eight);
  check((await rowPic(9)).loaded, 'a disk film shows the engine frame');
  await waitFor(async () => !(await rowPic(10)).pic, 'a failed picture falls back');
  check(true, 'a picture that does not load falls back to the coloured box');
  check(!(await rowPic(11)).pic, 'a row without a picture keeps the box');

  // A film starts: the row on top says what loads.
  const film = { id: 12, title: 'Big Buck Bunny', durationS: 596, isLive: false, requesterName: 'Kasia', requesterBadge: { hue: 40, shape: 3 }, sourceUrl: 'https://example.com/bbb', status: 'playing', thumb: `${base}/api/admin/items/12/thumb` };
  await scenario({ loading: { item: film, stage: 'resolving', elapsedS: 3, percent: null } });
  await page.waitForSelector('.q-loading[data-loading="12"]');
  const card = () => page.$eval('.q-loading', (e) => ({ text: e.innerText, stage: e.dataset.stage, bar: e.querySelector('.q-loading-bar')?.className ?? '', spinner: Boolean(e.querySelector('.q-spinner')), pic: Boolean(e.querySelector('.th-pic img')) }));
  let c = await card();
  check(/Now loading/i.test(c.text) && c.text.includes('Big Buck Bunny') && c.text.includes('Kasia'), 'the row names the film and who added it', c.text);
  check(c.text.includes('Reading the link') && c.text.includes('3 s') && c.spinner && /indeterminate/.test(c.bar), 'stage, seconds and an indeterminate bar', c);
  check(c.pic, 'with the film picture');
  const top = await page.evaluate(() => {
    const loading = document.querySelector('.q-loading').getBoundingClientRect().top;
    const list = document.querySelector('.qlist:not(.q-loading *)')?.getBoundingClientRect().top ?? Infinity;
    return loading < list;
  });
  check(top, 'above the queue list');
  await page.screenshot({ path: join(shots, 'queue-loading-en.png') });
  await scenario({ loading: { item: film, stage: 'downloading', elapsedS: 9, percent: 42.4 } });
  await waitFor(async () => (await card()).stage === 'downloading', 'downloading');
  c = await card();
  check(c.text.includes('Downloading') && c.text.includes('42%') && !/indeterminate/.test(c.bar), 'downloading with a percentage', c);
  await scenario({ loading: { item: film, stage: 'preparing', elapsedS: 14, percent: null } });
  await waitFor(async () => (await card()).text.includes('Preparing the picture'), 'preparing');
  check(true, 'then preparing');
  // Polish.
  await page.evaluate(() => import('/static/app/i18n.js').then((m) => m.loadLang('pl')));
  await waitFor(async () => (await card()).text.includes('Przygotowuję obraz'), 'Polish');
  check(!/loading\./.test((await card()).text), 'no raw keys in Polish');
  await page.screenshot({ path: join(shots, 'queue-loading-pl.png') });
  // On air: the row goes.
  await scenario({ loading: null });
  await waitFor(async () => !(await page.$('.q-loading')), 'the row goes');
  check(true, 'the row goes once the film is on air');
  check(errors.length === 0, 'no page errors', errors);
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
}
process.exit(0);
