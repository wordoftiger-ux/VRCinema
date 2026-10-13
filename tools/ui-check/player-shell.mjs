// Browser check of the interface's shell (title bar, rail, player bar, shortcuts, menu,
// dialog, languages) against the fake engine, in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/player-shell.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8193);
if (!shots) throw new Error('usage: player-shell.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) {
    throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  }
  passed += 1;
  console.log(`ok - ${what}`);
}

const engine = await startFakeEngine(port);
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  viewport: { width: 1100, height: 680 },
  args: ['--disable-extensions', '--no-first-run'],
});

const api = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const requests = () => api('/__test/requests');
const scenario = (s) => api('/__test/scenario', s);
const reset = () => api('/__test/reset', {});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitFor(fn, what, timeout = 5000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

/** API requests since `mark` other than the polling reads. */
async function actionsSince(mark) {
  return (await requests()).slice(mark).filter((r) => r.method !== 'GET');
}

// The program's window: Tauri's invoke, recorded.
const FAKE_TAURI = () => {
  window.__invokes = [];
  window.__TAURI_INTERNALS__ = {
    invoke: (cmd, args) => {
      window.__invokes.push(cmd);
      return Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null);
    },
  };
};

async function openApp({ inWindow = true, width = 1100, height = 680, reducedMotion = 'no-preference' } = {}) {
  const page = await ctx.newPage();
  await page.setViewportSize({ width, height });
  await page.emulateMedia({ reducedMotion });
  if (inWindow) await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  page.on('response', (r) => r.status() >= 400 && !r.url().includes('/api/') && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${base}/app`);
  await page.waitForSelector('.player .play');
  await page.waitForFunction(() => document.querySelector('.nowp-title')?.textContent !== '');
  page.__errors = errors;
  return page;
}

const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
// What the settings view does: saves the program's language, then switches the interface.
const setLanguage = (page, code) => page.evaluate(async (c) => (await import('/static/app/store.js')).setLanguage(c), code);

try {
  await reset();

  // ---------- Look: sizes of the mockup ----------
  let page = await openApp();
  await page.waitForTimeout(400);
  await page.screenshot({ path: join(shots, 'after-en-1100x680.png') });
  const sizes = await page.evaluate(() => {
    const r = (s) => document.querySelector(s).getBoundingClientRect();
    const cs = getComputedStyle(document.body);
    return { tb: r('.titlebar').height, rail: r('.rail').width, player: r('.player').height, play: r('.ctl .play').width, font: cs.fontFamily, bg: cs.backgroundColor, active: getComputedStyle(document.querySelector('.rail-item[aria-current=page]')).color };
  });
  check(sizes.tb === 32 && sizes.rail === 72 && sizes.player === 84 && sizes.play === 40, 'title bar 32 px, rail 72 px, player bar 84 px, play 40 px', sizes);
  check(sizes.font.includes('Segoe UI') && sizes.bg === 'rgb(20, 21, 31)' && sizes.active === 'rgb(242, 184, 75)', 'fonts and navy/amber tokens of the mockup', sizes);
  check(await page.$eval('.titlebar', (e) => e.getAttribute('data-tauri-drag-region')) === 'deep', 'the title bar is a drag region');

  // ---------- Requests carry the language ----------
  let all = await requests();
  check(all.length > 0 && all.every((r) => r.lang === 'en'), 'every request carries X-VRCinema-Lang: en', all.map((r) => r.lang));

  // ---------- Player bar buttons call the routes ----------
  check((await text(page, '.tl-pos')).startsWith('1:3'), 'the timeline shows the server position (control.positionS = 92 s, not current 80 s)', await text(page, '.tl-pos'));
  let mark = (await requests()).length;
  await page.click('.ctl .play');
  await waitFor(async () => (await actionsSince(mark)).some((r) => r.path.endsWith('/pause')), 'pause route');
  await page.waitForFunction(() => document.querySelector('.ctl .play').getAttribute('aria-label') === 'Resume');
  check(true, 'pause → POST …/pause, the button becomes "Resume"');
  check((await text(page, '.nowp-sub')) === 'Paused', 'the manual pause is shown under the title');
  await page.click('.ctl .play');
  await waitFor(async () => (await actionsSince(mark)).some((r) => r.path.endsWith('/resume')), 'resume route');
  check(true, 'resume → POST …/resume');
  mark = (await requests()).length;
  const sm = await page.$$('.ctl .sm');
  await sm[0].click();
  await sm[1].click();
  await page.click('.ctl [aria-label="From the beginning"]');
  await page.click('.ctl [aria-label="Next video"]');
  let acts = await waitFor(async () => {
    const a = await actionsSince(mark);
    return a.length >= 4 ? a : null;
  }, 'four control requests');
  check(
    JSON.stringify(acts.map((r) => [r.path.split('/').pop(), r.body])) ===
      JSON.stringify([['seek', { byS: -10 }], ['seek', { byS: 10 }], ['restart', null], ['skip', null]]),
    '−10 s, +10 s, ⏮, ⏭ call seek {byS:-10}, seek {byS:10}, restart, skip',
    acts.map((r) => [r.path, r.body]),
  );

  // ---------- Timeline: tooltip, click, drag ----------
  await scenario({ positionS: 92 });
  await page.waitForTimeout(1200);
  const box = await page.$eval('.track', (e) => {
    const r = e.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height };
  });
  await page.mouse.move(box.x + box.w * 0.5, box.y + box.h / 2);
  const tip = await page.waitForSelector('.track .tip');
  check((await tip.textContent()) === '1:46', 'hovering the middle shows 1:46 (half of 3:33)', await tip.textContent());
  mark = (await requests()).length;
  await page.mouse.down();
  await page.mouse.up();
  acts = await waitFor(async () => {
    const a = await actionsSince(mark);
    return a.length ? a : null;
  }, 'click seek');
  check(acts.length === 1 && Math.abs(acts[0].body.toS - 106.5) < 1.5, 'clicking the middle seeks to ≈106.5 s', acts.map((r) => r.body));
  mark = (await requests()).length;
  await page.mouse.move(box.x + box.w * 0.25, box.y + box.h / 2);
  await page.mouse.down();
  for (const f of [0.3, 0.4, 0.5, 0.6, 0.7, 0.75]) await page.mouse.move(box.x + box.w * f, box.y + box.h / 2);
  await page.waitForTimeout(150);
  check((await actionsSince(mark)).length === 0, 'no request while dragging');
  const dragShown = await text(page, '.tl-pos');
  await page.mouse.up();
  acts = await waitFor(async () => {
    const a = await actionsSince(mark);
    return a.length ? a : null;
  }, 'drag seek');
  check(acts.length === 1 && Math.abs(acts[0].body.toS - 0.75 * 213) < 2.5, 'dragging to 3/4 seeks once to ≈160 s', { body: acts[0].body, dragShown });
  await page.mouse.move(10, 10);

  // ---------- Keyboard shortcuts ----------
  await page.click('.view');
  mark = (await requests()).length;
  for (const key of ['Space', 'ArrowRight', 'Shift+ArrowLeft', 'ArrowLeft', 'Shift+ArrowRight', 'Home', 'n']) {
    await page.keyboard.press(key);
    await page.waitForTimeout(80);
  }
  acts = await waitFor(async () => {
    const a = await actionsSince(mark);
    return a.length >= 7 ? a : null;
  }, 'shortcut requests');
  check(
    JSON.stringify(acts.map((r) => [r.path.split('/').pop(), r.body])) ===
      JSON.stringify([['pause', null], ['seek', { byS: 10 }], ['seek', { byS: -60 }], ['seek', { byS: -10 }], ['seek', { byS: 60 }], ['restart', null], ['skip', null]]),
    'Space, →, Shift+←, ←, Shift+→, Home, N call pause, seek ±10/±60, restart, skip',
    acts.map((r) => [r.path, r.body]),
  );
  // Typing in a text field: no shortcut.
  await page.evaluate(() => {
    const i = document.createElement('input');
    i.id = 'typing';
    document.querySelector('#view').append(i);
  });
  await page.focus('#typing');
  mark = (await requests()).length;
  await page.keyboard.type(' n');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Home');
  await page.waitForTimeout(300);
  check((await actionsSince(mark)).length === 0, 'no shortcut acts while typing in a text field');
  await page.evaluate(() => document.getElementById('typing').remove());

  // ---------- Window controls ----------
  for (const label of ['Minimize', 'Maximize', 'Close']) await page.click(`.titlebar [aria-label="${label}"]`);
  const invokes = await page.evaluate(() => window.__invokes.filter((c) => c !== 'plugin:window|is_maximized'));
  check(
    JSON.stringify(invokes) === JSON.stringify(['plugin:window|minimize', 'plugin:window|toggle_maximize', 'plugin:window|close']),
    'title bar buttons run the window commands',
    invokes,
  );

  // ---------- Router ----------
  await page.click('.rail-history');
  check((await page.evaluate(() => location.hash)) === '#/history' && (await text(page, '.page h1')) === 'History', 'rail → #/history');
  await page.goto(`${base}/app#/settings/tools`);
  await page.waitForSelector('.view');
  check(
    (await page.$eval('.view', (e) => [e.dataset.view, e.dataset.section].join('/'))) === 'settings/tools',
    '#/settings/tools opens the settings view, section tools',
  );
  await page.click('.rail-channel');
  check((await page.$eval('.rail-channel', (e) => e.getAttribute('aria-current'))) === 'page', 'the current view is marked aria-current');

  // ---------- Languages: en → de → pl without a reload ----------
  await page.evaluate(() => (window.__noReload = true));
  await setLanguage(page, 'de');
  await page.waitForFunction(() => document.querySelector('.rail-channel .rail-label').textContent === 'Kanal');
  check((await page.evaluate(() => document.documentElement.lang)) === 'de', 'de: <html lang="de">');
  mark = (await requests()).length;
  await page.waitForTimeout(1300);
  all = (await requests()).slice(mark);
  check(all.length > 0 && all.every((r) => r.lang === 'de'), 'de: requests carry X-VRCinema-Lang: de', all.map((r) => r.lang));
  await page.screenshot({ path: join(shots, 'after-de-1100x680.png') });
  await setLanguage(page, 'pl');
  await page.waitForFunction(() => document.querySelector('.rail-channel .rail-label').textContent === 'Kanał');
  check(await page.evaluate(() => window.__noReload === true), 'en → de → pl switched without reloading the page');
  // The channel is paused by hand (Space above): the big button resumes.
  check((await page.$eval('.ctl .play', (e) => e.getAttribute('aria-label'))) === 'Wznów', 'pl: the play button is labelled "Wznów"');
  const saved = (await requests()).filter((r) => r.method === 'PUT').map((r) => [r.path, r.body.language]);
  check(JSON.stringify(saved) === JSON.stringify([['/api/desktop/config', 'de'], ['/api/desktop/config', 'pl']]), 'each switch saved the language with PUT /api/desktop/config', saved);
  // The program's language from the window's info follows a change made elsewhere (tray, settings).
  await scenario({ language: 'de' });
  await page.waitForFunction(() => document.querySelector('.rail-settings .rail-label').textContent.startsWith('Einstel'), null, { timeout: 6000 });
  check(true, 'a language change in the program reaches the open interface within 3 s');

  // ---------- Review Focus 5: language change with a menu and a dialog open ----------
  await setLanguage(page, 'en');
  await page.click('.nowp', { button: 'right' });
  await page.waitForSelector('.menu [role=menuitem]');
  check((await page.$eval('.menu', (e) => e.getAttribute('aria-label'))) === 'Playback menu', 'right click on the player bar opens the player menu');
  check((await page.evaluate(() => document.activeElement?.getAttribute('role'))) === 'menuitem', 'focus moves into the menu');
  await page.keyboard.press('ArrowDown');
  check((await page.evaluate(() => document.activeElement?.textContent.trim())) === 'Next video', '↓ moves to the next item');
  await page.screenshot({ path: join(shots, 'after-menu-en.png') });
  await setLanguage(page, 'de');
  const menuDe = await page.$$eval('.menu [role=menuitem]', (els) => els.map((e) => e.textContent.trim()));
  check(JSON.stringify(menuDe) === JSON.stringify(['Von vorn', 'Nächstes Video', 'Video-Link kopieren', 'Tastenkürzel']), 'the open menu switches to de', menuDe);
  await page.screenshot({ path: join(shots, 'after-menu-de.png') });
  await page.keyboard.press('Escape');
  check((await page.$('.menu')) === null, 'Esc closes the menu');
  await setLanguage(page, 'en');
  await page.click('.view');
  await page.keyboard.press('Shift+Slash');
  await page.waitForSelector('.dialog');
  check((await text(page, '#dialog-title')) === 'Keyboard shortcuts', '? opens the shortcuts dialog');
  check(await page.evaluate(() => document.querySelector('.dialog').contains(document.activeElement)), 'focus is inside the dialog');
  mark = (await requests()).length;
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('n');
  await page.keyboard.press('Home');
  await page.waitForTimeout(300);
  check((await actionsSince(mark)).length === 0 && (await page.$('.dialog')) !== null, 'shortcuts do not act behind the dialog');
  for (let i = 0; i < 4; i += 1) await page.keyboard.press('Tab');
  check(await page.evaluate(() => document.querySelector('.dialog').contains(document.activeElement)), 'Tab keeps the focus inside the dialog');
  await setLanguage(page, 'de');
  check((await text(page, '#dialog-title')) === 'Tastenkürzel', 'the open dialog switches to de');
  check((await page.$eval('.shortcuts', (e) => e.textContent)).includes('Leertaste'), 'the dialog body switches to de');
  await page.screenshot({ path: join(shots, 'after-dialog-de.png') });
  await page.keyboard.press('Escape');
  check((await page.$('.dialog')) === null, 'Esc closes the dialog');

  // ---------- Review Focus 5: nothing cut in de (player bar, rail), also at the minimum size ----------
  for (const [w, h] of [[1100, 680], [960, 600]]) {
    await page.setViewportSize({ width: w, height: h });
    await page.waitForTimeout(200);
    const cut = await page.evaluate(() =>
      [...document.querySelectorAll('.player button, .player .pill, .rail-item, .rail-label')]
        .filter((e) => e.scrollWidth > e.clientWidth || e.scrollHeight > e.clientHeight + 1)
        .map((e) => `${e.className}: ${e.textContent.trim()} ${e.scrollWidth}>${e.clientWidth}`),
    );
    check(cut.length === 0, `de ${w}×${h}: no button of the player bar and no rail item is cut`, cut);
    const keysLeft = await page.evaluate(() => document.body.innerText.match(/\b(nav|player|net|titlebar|shortcuts|ui)\.[a-z_]+/g));
    check(keysLeft === null, `de ${w}×${h}: no untranslated key on screen`, keysLeft);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    check(!overflow, `de ${w}×${h}: no horizontal scrolling`);
  }
  await page.screenshot({ path: join(shots, 'after-de-960x600.png') });
  check(page.__errors.length === 0, 'no errors in the console', page.__errors);
  await page.close();

  // ---------- States: approximate position, live, nothing playing, errors ----------
  await reset();
  await scenario({ positionApprox: true, pending: 2 });
  page = await openApp();
  await page.waitForFunction(() => document.querySelector('.tl-pos').textContent.startsWith('≈'));
  check(true, '"≈" before an approximate position');
  check((await text(page, '.rail-channel .badge [aria-hidden]')) === '2', 'the rail shows the number of videos waiting for approval');
  check((await text(page, '.rail-channel .badge .sr-only')) === '2 videos waiting for approval', 'plural text of the badge (tn)');
  await scenario({ playing: 'live', positionApprox: false });
  await page.waitForFunction(() => document.querySelector('.ctl .play').disabled);
  const live = await page.evaluate(() => ({
    sm: [...document.querySelectorAll('.ctl .sm')].map((b) => b.disabled),
    restart: document.querySelector('.ctl [aria-label="From the beginning"]').disabled,
    next: document.querySelector('.ctl [aria-label="Next video"]').disabled,
    track: document.querySelector('.track').getAttribute('aria-disabled'),
    tag: document.querySelector('.live-tag')?.textContent,
  }));
  check(
    JSON.stringify(live) === JSON.stringify({ sm: [true, true], restart: true, next: false, track: 'true', tag: 'LIVE' }),
    'live stream: pause, ±10 s, ⏮ and the timeline greyed out (canPause/canSeek false), ⏭ allowed, LIVE shown',
    live,
  );
  await page.screenshot({ path: join(shots, 'after-live.png') });
  await setLanguage(page, 'pl');
  await page.click('.view');
  // A refused control (the timeline is off, but the request may come from elsewhere): the
  // engine's message in the page's language is shown.
  await page.evaluate(async () => (await import('/static/app/store.js')).seekBy(10));
  await page.waitForSelector('.toast');
  check((await text(page, '.toast span')) === 'Transmisji na żywo nie można wstrzymać ani przewinąć.', 'a refused control shows the dictionary text of its error code (errors.live_item) in the page language', await text(page, '.toast span'));
  await scenario({ playing: 'none' });
  await page.waitForFunction(() => document.querySelector('.nowp-title').textContent === 'Nic nie leci');
  check(await page.$eval('.ctl .play', (b) => b.disabled), 'nothing playing: the controls are off');
  await page.close();

  // ---------- Remote panel (a browser): no title bar, no window status; reduced motion ----------
  await reset();
  await scenario({ remote: true });
  page = await openApp({ inWindow: false, reducedMotion: 'reduce' });
  check((await page.$('.titlebar')) === null, 'in a browser there is no title bar');
  check((await page.$('.player .pill')) === null, 'without the window info there is no internet pill');
  const transition = await page.$eval('.track .knob', (e) => getComputedStyle(e).transitionDuration);
  check(transition === '1e-05s' || transition === '0.00001s' || parseFloat(transition) < 0.001, 'prefers-reduced-motion turns transitions off', transition);
  await page.close();

  await reset();
  await scenario({ signedOut: true });
  page = await openApp({ inWindow: false });
  await page.waitForSelector('.signed-out');
  check((await page.$eval('.signed-out a', (a) => a.getAttribute('href'))) === '/admin', 'without a session the page points to the panel login');
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
}
