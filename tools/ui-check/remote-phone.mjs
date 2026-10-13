// Browser check of task 15 against the fake engine, in headless Brave with a temporary profile:
// the sign-in screen (password, errors and language), the phone layout at 390×844 (tabs at
// the bottom, the player bar under the preview, 44 px targets, no sideways scrolling, the
// queue's menu by long press and ⋮), the remote panel (language from the environment, the
// tunnel switch and its "you will lose access" question only for a session that came through
// the tunnel), a server without a window signing in with a password, the status strip, and the
// wizard's own title bar.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/remote-phone.mjs <shots dir> [port]
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
const port = Number(process.argv[3] ?? 8193);
if (!shots) throw new Error('usage: remote-phone.mjs <shots dir> [port]');
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
  hasTouch: true,
  viewport: { width: 1100, height: 680 },
  args: ['--disable-extensions', '--no-first-run'],
});

const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const requests = () => call('/__test/requests');
const scenario = (s) => call('/__test/scenario', s);
const reset = () => call('/__test/reset', {});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mark = async () => (await requests()).length;
const actionsSince = async (m) => (await requests()).slice(m).filter((r) => r.method !== 'GET');

async function waitFor(fn, what, timeout = 5000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};

/**
 * A page of the interface. `languages`: what the browser says it prefers; `stored`: a panel
 * language picked earlier in this browser (null: none).
 */
async function open(path, { inWindow = false, width = 1100, height = 680, languages = ['en-US', 'en'], stored = null, wait = '.player .play' } = {}) {
  const page = await ctx.newPage();
  await page.setViewportSize({ width, height });
  if (inWindow) await page.addInitScript(FAKE_TAURI);
  await page.addInitScript(({ languages, stored }) => {
    Object.defineProperty(navigator, 'languages', { get: () => languages });
    Object.defineProperty(navigator, 'language', { get: () => languages[0] });
    if (!sessionStorage.getItem('__primed')) {
      sessionStorage.setItem('__primed', '1');
      if (stored) localStorage.setItem('vrcinema.lang', stored);
      else localStorage.removeItem('vrcinema.lang');
    }
  }, { languages, stored });
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of (401|403|404)/.test(m.text()) && errors.push(m.text()));
  page.on('response', (r) => r.status() >= 400 && !r.url().includes('/api/') && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${base}${path}`);
  if (wait) await page.waitForSelector(wait);
  page.__errors = errors;
  return page;
}

const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const shot = (page, name) => page.screenshot({ path: join(shots, name) });
const noSideScroll = (page) =>
  page.evaluate(() => {
    const view = document.querySelector('#view');
    return document.documentElement.scrollWidth <= window.innerWidth + 1 && (!view || view.scrollWidth <= view.clientWidth + 1);
  });
/** Visible controls under `root` smaller than 44×44 px (the checkbox-like switch counts its box). */
const smallTargets = (page, root) =>
  page.$$eval(
    `${root} button, ${root} a[href], ${root} select, ${root} input:not([type=hidden]), ${root} [role=slider]`,
    (els) =>
      els
        .filter((e) => {
          const r = e.getBoundingClientRect();
          const s = getComputedStyle(e);
          if (r.width === 0 || r.height === 0 || s.visibility === 'hidden') return false;
          // A slider is pressed on its hit strip around it, a radio button or a checkbox on its label.
          const box = e.matches('[role=slider]')
            ? e.closest('.track-hit').getBoundingClientRect()
            : e.matches('input[type=radio], input[type=checkbox]') && e.closest('label')
              ? e.closest('label').getBoundingClientRect()
              : r;
          const text = e.matches('input[type=text], input[type=url], input[type=password], input[type=radio], input[type=checkbox], select, .txt');
          return box.height < 44 - 0.5 || (!text && !e.matches('[role=slider]') && box.width < 44 - 0.5);
        })
        .map((e) => `${e.tagName.toLowerCase()}.${e.className} ${Math.round(e.getBoundingClientRect().width)}×${Math.round(e.getBoundingClientRect().height)}`),
  );

try {
  // ---------- Sign-in: language from the browser, errors in it, the choice kept ----------
  await reset();
  await scenario({ remote: true, signedOut: true, language: 'pl' });
  let page = await open('/admin', { languages: ['pl-PL', 'pl'], wait: '.login-card input[type=password]' });
  check((await text(page, '.login-card h1')) === 'Zaloguj się do panelu kanału', 'signed out: the sign-in form in the browser language (pl)', await text(page, '.login-card h1'));
  check((await page.$('.rail')) === null && (await page.$('.player')) === null, 'nothing of the panel is shown before signing in');
  check(await page.evaluate(() => document.activeElement?.type === 'password'), 'the password field has the focus');
  check(await page.$eval('.login-submit', (b) => b.disabled), 'the button waits for a password');
  await shot(page, 'after-login-pl-1100x680.png');
  await page.fill('.login-card input[type=password]', 'zle-haslo');
  let m = await mark();
  await page.click('.login-submit');
  await page.waitForSelector('.login-error');
  check((await text(page, '.login-error')) === 'Złe hasło.', 'a wrong password: errors.wrong_password in Polish', await text(page, '.login-error'));
  let sent = (await actionsSince(m)).find((r) => r.path === '/api/admin/login');
  check(sent?.lang === 'pl' && sent.body?.password === 'zle-haslo', 'the login request carries the page language', sent);
  // Picking English here: kept in the browser, the error follows the switch.
  await page.selectOption('.login-lang select', 'en');
  await page.waitForFunction(() => document.querySelector('.login-card h1').textContent === 'Sign in to the channel panel');
  check((await text(page, '.login-error')) === 'Wrong password.', 'the error follows the language switch');
  check((await page.evaluate(() => localStorage.getItem('vrcinema.lang'))) === 'en', 'the choice is kept in this browser');
  await page.reload();
  await page.waitForSelector('.login-card');
  check((await text(page, '.login-card h1')) === 'Sign in to the channel panel', 'after a reload the stored choice wins over the browser language');
  await page.fill('.login-card input[type=password]', 'tajne-haslo');
  await page.click('.login-submit');
  await page.waitForSelector('.player .play');
  await page.waitForSelector('.rail');
  check(true, 'the right password opens the panel');
  await sleep(3500); // an environment read with language "pl" came and went
  check((await page.evaluate(() => document.documentElement.lang)) === 'en', 'signed in, the stored choice still wins over the program language', await page.evaluate(() => document.documentElement.lang));
  await page.close();

  // Without a stored choice the session takes the program's language once signed in.
  await reset();
  await scenario({ remote: true, signedOut: true, language: 'pl' });
  page = await open('/admin', { languages: ['en-US'], wait: '.login-card' });
  check((await text(page, '.login-card h1')) === 'Sign in to the channel panel', 'no choice: the browser language on the sign-in screen');
  await page.fill('.login-card input[type=password]', 'tajne-haslo');
  await page.click('.login-submit');
  await page.waitForFunction(() => document.documentElement.lang === 'pl', null, { timeout: 8000 });
  await page.waitForFunction(() => document.querySelector('.rail-channel .rail-label')?.textContent === 'Kanał');
  check(true, 'signed in without a choice: the program language from the environment (pl)');
  check(!(await requests()).some((r) => r.path.startsWith('/api/desktop/') && r.path !== '/api/desktop/info'), 'a remote session asks no window-only route but the info');
  await page.close();

  // The remote panel switched off.
  await reset();
  await scenario({ remote: true, signedOut: true, remotePanelOff: true });
  page = await open('/admin', { wait: '.login-card' });
  await page.fill('.login-card input[type=password]', 'tajne-haslo');
  await page.click('.login-submit');
  await page.waitForSelector('.login-error');
  check((await text(page, '.login-error')) === 'The remote panel is switched off.', 'remote panel off: its own message', await text(page, '.login-error'));
  await page.close();

  // The program's window never asks for a password.
  await reset();
  await scenario({ signedOut: true });
  page = await open('/admin', { inWindow: true, wait: '.login-card' });
  check((await page.$('.login-card input[type=password]')) === null, 'the window signed out: no password field');
  check((await text(page, '.login-card h1')) === "The window's session has ended", 'the window says how to get the panel back', await text(page, '.login-card h1'));
  check((await page.$('.titlebar')) !== null, 'and keeps its title bar (no frame)');
  await page.close();

  // ---------- A server without a window: sign in with the password ----------
  await reset();
  await scenario({ headless: true, signedOut: true, language: 'en' });
  page = await open('/admin', { wait: '.login-card' });
  await page.fill('.login-card input[type=password]', 'tajne-haslo');
  await page.click('.login-submit');
  await page.waitForSelector('.player .play');
  await page.evaluate(() => (location.hash = '#/settings/general'));
  await page.waitForSelector('[data-panel-language]');
  check((await page.$eval('#view', (e) => e.innerText)).includes('LANGUAGE in the .env file'), 'headless: the server language is set in .env');
  check(await page.$eval('[data-panel-language]', (e) => !e.disabled), 'headless: the panel language can be picked in the browser');
  await page.selectOption('[data-panel-language]', 'pl');
  await page.waitForFunction(() => document.documentElement.lang === 'pl');
  check((await page.evaluate(() => localStorage.getItem('vrcinema.lang'))) === 'pl', 'the panel language is kept in the browser');
  check(!(await requests()).some((r) => r.method === 'PUT' && r.path === '/api/desktop/config'), 'and the program settings are not touched');
  await page.evaluate(() => (location.hash = '#/settings/internet'));
  await page.waitForSelector('#view[data-section="internet"]');
  check(await page.$eval('[role=switch]', (e) => e.disabled), 'headless: no tunnel to switch');
  check(page.__errors.length === 0, 'no console errors (headless)', page.__errors);
  await page.close();

  // ---------- The tunnel switch of a remote session and the "lose access" question ----------
  const internet = async (s, opts = {}) => {
    await reset();
    await scenario(s);
    const p = await open('/admin#/settings/internet', opts);
    await p.waitForSelector('#view[data-section="internet"] [role=switch]');
    await p.waitForFunction(() => !document.querySelector('#view [role=switch]').disabled);
    return p;
  };
  const SWITCH = '#view [role=switch][aria-label^="Share the channel"]';
  page = await internet({ remote: true, sessionViaTunnel: true });
  check((await page.$eval(SWITCH, (e) => e.getAttribute('aria-checked'))) === 'true', 'remote: the switch shows the tunnel on (from the environment)');
  check((await page.$('[data-via-tunnel]')) !== null, 'remote through the tunnel: it says so under the switch');
  m = await mark();
  await page.click(SWITCH);
  await page.waitForSelector('.dialog [data-lose-access]');
  check((await text(page, '#dialog-title')) === 'Switch the tunnel off?', 'switching off asks first', await text(page, '#dialog-title'));
  check((await text(page, '[data-lose-access]')).includes('you will lose access to this panel'), 'and says the panel will be lost');
  check(await page.evaluate(() => document.activeElement?.textContent.trim() === 'Keep it on'), 'the safe answer has the focus');
  await shot(page, 'after-remote-lose-access-en-1100x680.png');
  await page.keyboard.press('Enter');
  await sleep(400);
  check((await actionsSince(m)).length === 0, '"Keep it on": nothing is sent');
  check((await page.$eval(SWITCH, (e) => e.getAttribute('aria-checked'))) === 'true', 'and the switch stays on');
  await page.click(SWITCH);
  await page.waitForSelector('.dialog');
  await page.keyboard.press('Escape');
  await sleep(300);
  check((await actionsSince(m)).length === 0, 'Esc cancels too');
  await page.click(SWITCH);
  await page.click('.dialog .btn.danger');
  const off = await waitFor(async () => (await actionsSince(m)).find((r) => r.path === '/api/admin/tunnel'), 'the switch request');
  check(off.method === 'PUT' && JSON.stringify(off.body) === '{"enabled":false}', 'confirmed: PUT /api/admin/tunnel {enabled: false}', off);
  check(!(await actionsSince(m)).some((r) => r.path.startsWith('/api/desktop/')), 'and no window-only request');
  await page.waitForFunction((sel) => document.querySelector(sel).getAttribute('aria-checked') === 'false', SWITCH);
  check(true, 'the switch shows the tunnel off');
  m = await mark();
  await page.click(SWITCH);
  await waitFor(async () => (await actionsSince(m)).find((r) => r.path === '/api/admin/tunnel'), 'switching on');
  check((await page.$('.dialog')) === null, 'switching on never asks');
  await page.close();

  page = await internet({ remote: true, sessionViaTunnel: false });
  check((await page.$('[data-via-tunnel]')) === null, 'remote from the home network: no note');
  check(await page.$eval('#view .kv button', (b) => b.disabled), 'remote: "Reconnect" stays window-only');
  check((await page.$('.named-link')) === null, 'remote: no permanent link wizard (window only)');
  m = await mark();
  await page.click(SWITCH);
  const direct = await waitFor(async () => (await actionsSince(m)).find((r) => r.path === '/api/admin/tunnel'), 'switching off from the LAN');
  check((await page.$('.dialog')) === null && JSON.stringify(direct.body) === '{"enabled":false}', 'a session not through the tunnel switches it off without the question');
  await page.close();

  page = await internet({}, { inWindow: true });
  m = await mark();
  await page.click(SWITCH);
  const win = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PUT'), 'switching off in the window');
  check((await page.$('.dialog')) === null && win.path === '/api/desktop/config' && JSON.stringify(win.body) === '{"tunnelEnabled":false}', 'the window saves its settings, no question', win);
  await page.close();

  // ---------- Status strip ----------
  await reset();
  await scenario({ toolsMissing: ['ffmpeg', 'deno'], tickerSlow: true, updateOffer: { version: '0.3.0', notes: '' } });
  page = await open('/admin#/channel', { inWindow: true });
  await page.waitForSelector('.status-strip');
  const kinds = await page.$$eval('.status-note', (els) => els.map((e) => e.dataset.status));
  check(JSON.stringify(kinds) === '["tools_missing","ticker_slow","update"]', 'the strip lists what needs attention', kinds);
  check((await text(page, '[data-status="tools_missing"] .status-text')) === 'Some tools are missing (ffmpeg, deno) — the channel needs them.', 'missing tools named');
  check((await page.$eval('[data-status="tools_missing"] a', (a) => a.getAttribute('href'))) === '#/settings/tools', 'linked to Settings → Tools');
  await shot(page, 'after-status-strip-en-1100x680.png');
  await page.click('[data-status="ticker_slow"] a');
  await page.waitForSelector('#view[data-section="channel"]');
  check((await page.$('[data-status="ticker_slow"]')) === null, 'the notice of the section on show is left out');
  await page.click('[data-status="update"] .status-close');
  check((await page.$('[data-status="update"]')) === null, 'a notice can be dismissed');
  await page.evaluate(() => (location.hash = '#/channel'));
  await sleep(300);
  check((await page.$('[data-status="update"]')) === null && (await page.$('[data-status="ticker_slow"]')) !== null, 'a dismissed notice stays away, the others come back');
  await scenario({ toolsMissing: [], channelDown: true, tunnel: 'error' });
  await page.waitForSelector('[data-status="channel_down"]', { timeout: 8000 });
  check((await text(page, '[data-status="channel_down"] .status-text')) === 'The channel is not running: ffmpeg exited (code 1)', 'the channel down with its reason');
  check((await page.$eval('[data-status="tunnel_error"] a', (a) => a.getAttribute('href'))) === '#/settings/internet', 'a tunnel error links to Internet');
  await page.close();
  // A remote session: what the environment knows (no tools, no update offer).
  await reset();
  await scenario({ remote: true, tunnel: 'error', tickerSlow: true, toolsMissing: ['ffmpeg'] });
  page = await open('/admin');
  await page.waitForSelector('[data-status="tunnel_error"]');
  const remoteKinds = await page.$$eval('.status-note', (els) => els.map((e) => e.dataset.status));
  check(JSON.stringify(remoteKinds) === '["tunnel_error","ticker_slow"]', 'remote: the strip shows the tunnel and the ticker from the environment', remoteKinds);
  await page.close();

  // ---------- The first run and the wizard's title bar ----------
  await reset();
  await scenario({ firstRun: true });
  page = await open('/admin', { inWindow: true, wait: null });
  await page.waitForURL(`${base}/wizard`);
  await page.waitForSelector('.wz');
  check(true, 'the window\'s first run goes from /admin to the wizard');
  check((await page.$('.titlebar .wc-close')) !== null, 'the wizard (no frame) has the title bar with the window buttons');
  await shot(page, 'after-wizard-titlebar-1100x680.png');
  await page.close();

  // ---------- Phone 390×844: a remote session ----------
  await reset();
  await scenario({ remote: true, sessionViaTunnel: true, pending: 1 });
  page = await open('/admin', { width: 390, height: 844 });
  await page.waitForSelector('.qi-queued');
  await sleep(500);
  await shot(page, 'after-phone-channel-en-390x844.png');
  const layout = await page.evaluate(() => {
    const r = (s) => document.querySelector(s)?.getBoundingClientRect();
    const rail = r('.rail');
    return {
      railBottom: Math.round(rail.bottom),
      railTop: Math.round(rail.top),
      railWidth: Math.round(rail.width),
      tabs: [...document.querySelectorAll('.rail-item')].map((a) => Math.round(a.getBoundingClientRect().top)),
      video: r('.video'),
      player: r('.player'),
      queue: r('.q'),
      play: r('.player .play'),
      inner: innerHeight,
      top: document.querySelector('.ptop .pill')?.textContent ?? null,
    };
  });
  check(layout.railBottom === 844 && layout.railWidth === 390 && new Set(layout.tabs).size === 1 && layout.tabs.length === 4, 'four tabs in one row at the bottom', layout);
  check(layout.player.top >= layout.video.bottom - 1 && layout.queue.top >= layout.player.bottom - 1, 'preview, then the player bar, then the queue', layout);
  check(Math.round(layout.play.width) >= 56, 'a big play button', layout.play);
  check(layout.top === 'Online', 'the top bar shows the internet state', layout.top);
  check(await noSideScroll(page), 'Channel: no sideways scrolling');
  let small = await smallTargets(page, '.app');
  check(small.length === 0, 'Channel: every control at least 44 px', small);

  // Long press on a queue row opens its menu (touch); a short tap does not.
  const cdp = await ctx.newCDPSession(page);
  const touchRow = async (id, holdMs) => {
    await page.$eval(`.qi[data-id="${id}"]`, (e) => e.scrollIntoView({ block: 'center' }));
    const box = await page.$eval(`.qi[data-id="${id}"] .qi-text`, (e) => {
      const r = e.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    });
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: box.x, y: box.y }] });
    await sleep(holdMs);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  };
  await touchRow(9, 120);
  await sleep(600);
  check((await page.$('.menu')) === null, 'a short tap on a row opens nothing');
  await touchRow(9, 750);
  await page.waitForSelector('.menu [role=menuitem]');
  const touchItems = await page.$$eval('.menu [role=menuitem]', (els) => els.map((e) => e.textContent.trim()));
  check(touchItems.includes('Move up') && touchItems.includes('Move down'), 'a long press opens the row menu with Move up / Move down', touchItems);
  check(await page.$$eval('.menu [role=menuitem]', (els) => els.every((e) => e.getBoundingClientRect().height >= 44)), 'menu items are 44 px tall');
  await shot(page, 'after-phone-longpress-menu-en-390x844.png');
  m = await mark();
  await page.click('.menu [role=menuitem]:has-text("Move down")');
  const moved = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PATCH'), 'move down');
  check(moved.path === '/api/admin/items/9' && JSON.stringify(moved.body) === '{"position":2}', 'Move down from the long-press menu', moved);
  // The ⋮ button does the same with a tap.
  await page.click('.qi[data-id="8"] .qi-more');
  await page.waitForSelector('.menu');
  check((await page.$eval('.menu', (e) => e.getAttribute('aria-label'))) === 'Video menu', '⋮ opens the row menu');
  await page.keyboard.press('Escape');
  check(page.__errors.length === 0, 'no console errors on the phone Channel', page.__errors);

  for (const [view, name] of [['history', 'History'], ['share', 'Share'], ['settings', 'Settings']]) {
    await page.click(`.rail-${view}`);
    await page.waitForSelector(`#view[data-view="${view}"]`);
    await sleep(400);
    check((await page.$eval(`.rail-${view}`, (a) => a.getAttribute('aria-current'))) === 'page', `${name}: reached from its tab`);
    check(await noSideScroll(page), `${name}: no sideways scrolling`);
    small = await smallTargets(page, '#view');
    check(small.length === 0, `${name}: every control at least 44 px`, small);
    await shot(page, `after-phone-${view}-en-390x844.png`);
  }
  for (const section of ['internet', 'channel', 'general']) {
    await page.evaluate((s) => (location.hash = `#/settings/${s}`), section);
    await page.waitForSelector(`#view[data-section="${section}"]`);
    await sleep(350);
    check(await noSideScroll(page), `Settings → ${section}: no sideways scrolling`);
    small = await smallTargets(page, '.sform');
    check(small.length === 0, `Settings → ${section}: every control at least 44 px`, small);
    await shot(page, `after-phone-settings-${section}-en-390x844.png`);
  }
  // The question works on the phone as well.
  await page.evaluate(() => (location.hash = '#/settings/internet'));
  await page.waitForFunction(() => !document.querySelector('#view [role=switch]')?.disabled);
  await page.click(SWITCH);
  await page.waitForSelector('.dialog [data-lose-access]');
  check(await noSideScroll(page), 'the dialog fits the phone');
  await shot(page, 'after-phone-lose-access-en-390x844.png');
  await page.keyboard.press('Escape');
  await page.click('.rail-channel');
  await page.waitForSelector('#view[data-view="channel"] .player');
  check(true, 'back to Channel from its tab');
  check(page.__errors.length === 0, 'no console errors on the phone', page.__errors);
  await page.close();

  // The sign-in screen on a phone.
  await reset();
  await scenario({ remote: true, signedOut: true });
  page = await open('/admin', { width: 390, height: 844, wait: '.login-card' });
  check(await noSideScroll(page), 'sign-in: no sideways scrolling on a phone');
  small = await smallTargets(page, '.login-card');
  check(small.length === 0, 'sign-in: controls at least 44 px', small);
  await shot(page, 'after-phone-login-en-390x844.png');
  await page.close();

  // The window at 1100×680 keeps the desktop layout.
  await reset();
  page = await open('/admin', { inWindow: true });
  const desk = await page.evaluate(() => {
    const rail = document.querySelector('.rail').getBoundingClientRect();
    return { railW: Math.round(rail.width), railH: Math.round(rail.height), more: getComputedStyle(document.querySelector('.qi-more')).display, ptop: !!document.querySelector('.ptop') };
  });
  check(desk.railW === 72 && desk.railH > 300 && desk.more === 'none' && !desk.ptop, 'at 1100×680 the rail stays on the left, no ⋮ buttons, no phone top bar', desk);
  await shot(page, 'after-window-channel-en-1100x680.png');
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
  stream.cleanup();
}
