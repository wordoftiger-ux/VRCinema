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

  // Fix round 2: a remote panel with a language picked in this browser opens in it — never
  // in the program's language first.
  await reset();
  await scenario({ remote: true, language: 'pl' });
  {
    const p = await ctx.newPage();
    await p.addInitScript(() => {
      window.__langs = [];
      // The document itself: the root element the parser makes comes after this script.
      new MutationObserver((list) => list.forEach((m) => window.__langs.push(m.target.getAttribute('lang')))).observe(document, { subtree: true, attributes: true, attributeFilter: ['lang'] });
    });
    await p.addInitScript(() => localStorage.setItem('vrcinema.lang', 'en'));
    await p.goto(`${base}/admin`);
    await p.waitForSelector('.player .play');
    await sleep(3500); // two info reads (program language pl) came and went
    const langs = await p.evaluate(() => window.__langs);
    check(langs.includes('en') && !langs.includes('pl') && (await p.evaluate(() => document.documentElement.lang)) === 'en', 'remote with a picked language: never the program language first', langs);
    await p.close();
  }

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

  // ---------- The tunnel in a remote session and the "lose access" question ----------
  const internet = async (s, opts = {}) => {
    await reset();
    await scenario(s);
    const p = await open('/admin#/settings/internet', opts);
    await p.waitForSelector('#view[data-section="internet"] [role=switch]');
    await p.waitForFunction(() => !document.querySelector('#view [role=switch]').disabled);
    return p;
  };
  const SWITCH = '#view [role=switch][aria-label^="Share the channel"]';
  const RECONNECT = '#view .kv button';
  const noteText = (p) => p.$eval('#view .set-note', (e) => e.textContent.trim()).catch(() => null);
  page = await internet({ remote: true, sessionViaTunnel: true });
  check((await page.$eval(SWITCH, (e) => e.getAttribute('aria-checked'))) === 'true', 'remote: the switch shows the tunnel on');
  check((await page.$('[data-via-tunnel]')) !== null, 'remote through the tunnel: it says so under the switch');
  m = await mark();
  await page.click(SWITCH);
  await page.waitForSelector('.dialog [data-lose-access="off"]');
  check((await text(page, '#dialog-title')) === 'Switch the tunnel off?', 'switching off asks first', await text(page, '#dialog-title'));
  const offText = await text(page, '[data-lose-access]');
  check(offText.includes('you will lose access to this panel') && offText.includes('a quick link gets a new address'), 'and says the panel will be lost and a quick address changes', offText);
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
  // Reconnecting asks too (a quick link gets a new address); the connection then ends.
  await page.click(RECONNECT);
  await page.waitForSelector('.dialog [data-lose-access="reconnect"]');
  check((await text(page, '[data-lose-access]')).includes('a quick link gets a new address'), 'reconnecting asks and names the new quick address');
  await page.click('.dialog .btn.danger');
  const rec = await waitFor(async () => (await actionsSince(m)).find((r) => r.path === '/api/admin/tunnel/restart'), 'the reconnect request');
  check(rec.method === 'POST', 'confirmed: POST /api/admin/tunnel/restart', rec);
  await waitFor(async () => (await noteText(page)) !== null, 'the reconnect note', 9000);
  check((await noteText(page)).startsWith('Done — the connection of this panel ended'), 'the dropped connection is reported as such, not as an error', await noteText(page));
  // Back to a quick link from the permanent one changes the address: asked too.
  await scenario({ named: { mode: 'named', host: 'kanal.example.com', loggedIn: true, progress: { operation: null, state: 'idle', running: false } } });
  await page.waitForSelector('.named-link[data-mode="named"] .pl-actions button:not(.danger)', { timeout: 8000 });
  await page.click('.named-link[data-mode="named"] .pl-actions button:not(.danger)');
  await page.waitForSelector('.dialog [data-lose-access="mode"]');
  check((await text(page, '#dialog-title')) === 'Change the link?', 'back to a quick link asks first');
  await page.keyboard.press('Escape');
  check(await page.$eval('.named-link .btn.danger', (b) => b.disabled), 'deleting the permanent link stays in the window');
  // Fix round 2: confirmed, the dropped connection of a link change is "connection ended" too.
  m = await mark();
  await page.click('.named-link[data-mode="named"] .pl-actions button:not(.danger)');
  await page.click('.dialog .btn.danger');
  await waitFor(async () => (await actionsSince(m)).some((r) => r.path === '/api/admin/tunnel/named/quick'), 'back to quick request');
  await waitFor(async () => (await page.$eval('.pl-notice', (e) => e.textContent).catch(() => ''))?.startsWith('Done — the connection'), 'the link change note', 9000);
  check(true, 'back to a quick link through the tunnel: "connection ended", not an error');
  // And back to the permanent link (its files are still there): asked, then the same.
  await page.waitForSelector('.named-link[data-mode="quick"] .pl-actions button.pri:not([disabled])', { timeout: 8000 });
  m = await mark();
  await page.click('.named-link[data-mode="quick"] .pl-actions button.pri');
  await page.waitForSelector('.dialog [data-lose-access="mode"]');
  await page.click('.dialog .btn.danger');
  const named = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PUT'), 'back to named request');
  check(named.path === '/api/admin/config' && JSON.stringify(named.body) === '{"tunnelMode":"named"}', 'back to the permanent link → PUT /api/admin/config {tunnelMode: named}', named);
  await waitFor(async () => (await page.$eval('.pl-notice', (e) => e.textContent).catch(() => ''))?.startsWith('Done — the connection'), 'the back-to-named note', 9000);
  check(true, 'and its dropped connection is "connection ended" too');
  m = await mark();
  await page.click(SWITCH);
  await page.click('.dialog .btn.danger');
  const off = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PUT'), 'the switch request');
  check(off.path === '/api/admin/config' && JSON.stringify(off.body) === '{"tunnelEnabled":false}', 'confirmed: PUT /api/admin/config {tunnelEnabled: false}', off);
  check(!(await actionsSince(m)).some((r) => r.path.startsWith('/api/desktop/')), 'and no window-only request');
  await waitFor(async () => (await noteText(page))?.startsWith('Done — the connection'), 'the switch-off note', 12000);
  check(true, 'switching off through the tunnel: "connection ended", not an error');
  await page.waitForFunction((sel) => document.querySelector(sel).getAttribute('aria-checked') === 'false', SWITCH);
  check(true, 'the switch shows the tunnel off');
  m = await mark();
  await page.click(SWITCH);
  const on = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PUT'), 'switching on');
  check((await page.$('.dialog')) === null && JSON.stringify(on.body) === '{"tunnelEnabled":true}', 'switching on never asks');
  check(page.__errors.filter((e) => !/ERR_EMPTY_RESPONSE|Failed to fetch|net::/.test(e)).length === 0, 'no console errors but the dropped connections', page.__errors);
  await page.close();

  page = await internet({ remote: true, sessionViaTunnel: false });
  check((await page.$('[data-via-tunnel]')) === null, 'remote from the home network: no note');
  m = await mark();
  await page.click(RECONNECT);
  await waitFor(async () => (await actionsSince(m)).find((r) => r.path === '/api/admin/tunnel/restart'), 'reconnect from the LAN');
  check((await page.$('.dialog')) === null, 'a session not through the tunnel reconnects without the question');
  m = await mark();
  await page.click(SWITCH);
  const direct = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PUT'), 'switching off from the LAN');
  check((await page.$('.dialog')) === null && direct.path === '/api/admin/config' && JSON.stringify(direct.body) === '{"tunnelEnabled":false}', 'and switches it off without the question', direct);
  await page.close();

  page = await internet({}, { inWindow: true });
  m = await mark();
  await page.click(SWITCH);
  const win = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PUT'), 'switching off in the window');
  check((await page.$('.dialog')) === null && win.path === '/api/desktop/config' && JSON.stringify(win.body) === '{"tunnelEnabled":false}', 'the window saves its settings, no question', win);
  await page.close();

  // ---------- Signing out (a password session) ----------
  await reset();
  await scenario({ remote: true });
  page = await open('/admin#/settings/general');
  await page.waitForSelector('#view [data-sign-out]');
  m = await mark();
  await page.click('#view [data-sign-out]');
  await page.waitForSelector('.login-card input[type=password]');
  check((await actionsSince(m)).some((r) => r.method === 'POST' && r.path === '/api/admin/logout'), 'Settings → General: Sign out → POST /api/admin/logout and the sign-in screen');
  await sleep(1500);
  check((await page.$('.rail')) === null, 'the panel does not come back after signing out');
  await page.fill('.login-card input[type=password]', 'tajne-haslo');
  await page.click('.login-submit');
  await page.waitForSelector('.player .play');
  check(true, 'signing in again opens the panel');
  await page.close();
  await reset();
  page = await open('/admin#/settings/general', { inWindow: true });
  await page.waitForSelector('#view[data-section="general"] select');
  check((await page.$('[data-sign-out]')) === null, 'the window has no sign out (its session comes from the tray)');
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
  // A remote session (fix round 1: the full info, as in the window): tools linked too.
  await reset();
  await scenario({ remote: true, tunnel: 'error', tickerSlow: true, toolsMissing: ['ffmpeg'] });
  page = await open('/admin');
  await page.waitForSelector('[data-status="tunnel_error"]');
  const remoteKinds = await page.$$eval('.status-note', (els) => els.map((e) => e.dataset.status));
  check(JSON.stringify(remoteKinds) === '["tools_missing","tunnel_error","ticker_slow"]', 'remote: the strip shows tools, tunnel and ticker', remoteKinds);
  check((await page.$eval('[data-status="tools_missing"] a', (a) => a.getAttribute('href'))) === '#/settings/tools', 'remote: the tools notice links to Tools (they can be fixed remotely)');
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
  // The browser's own long-press gesture (contextmenu) right after ours: dropped, the menu stays.
  const before = await page.$eval('.menu', (e) => e.style.left + '/' + e.style.top);
  const kept = await page.$eval('.qi[data-id="9"]', (row) => {
    const r = row.getBoundingClientRect();
    return row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.right - 60, clientY: r.top + 5 }));
  });
  await sleep(150);
  check(kept === false && (await page.$eval('.menu', (e) => e.style.left + '/' + e.style.top)) === before, 'a native contextmenu after the long press opens no second menu', { kept, before });
  m = await mark();
  await page.click('.menu [role=menuitem]:has-text("Move down")');
  const moved = await waitFor(async () => (await actionsSince(m)).find((r) => r.method === 'PATCH'), 'move down');
  check(moved.path === '/api/admin/items/9' && JSON.stringify(moved.body) === '{"position":2}', 'Move down from the long-press menu', moved);
  // The ⋮ button does the same with a tap.
  await page.click('.qi[data-id="8"] .qi-more');
  await page.waitForSelector('.menu');
  check((await page.$eval('.menu', (e) => e.getAttribute('aria-label'))) === 'Video menu', '⋮ opens the row menu');
  await page.keyboard.press('Escape');
  // The browser's gesture first (during the hold): its menu stays, ours does not follow.
  await page.$eval('.qi[data-id="10"]', (e) => e.scrollIntoView({ block: 'center' }));
  const pt = await page.$eval('.qi[data-id="10"] .qi-text', (e) => {
    const r = e.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: pt.x, y: pt.y }] });
  await sleep(200);
  await page.$eval('.qi[data-id="10"]', (row, p) => row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: p.x, clientY: p.y })), pt);
  await sleep(700);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  const left = await page.$eval('.menu', (e) => parseFloat(e.style.left));
  check(Math.abs(left - Math.min(pt.x, 390 - 4 - (await page.$eval('.menu', (e) => e.getBoundingClientRect().width)))) < 2, 'a native contextmenu first: one menu at its place, the long-press timer dropped', { left, pt });
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
  // Signing out from the phone's top bar.
  m = await mark();
  await page.click('.ptop [data-sign-out]');
  await page.waitForSelector('.login-card input[type=password]');
  check((await actionsSince(m)).some((r) => r.path === '/api/admin/logout'), 'the phone top bar signs out');
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
