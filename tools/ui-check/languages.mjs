// Browser check of every view in one or more languages (task 17, Review Focus 5): screenshots and
// DOM checks that nothing is cut, nothing scrolls sideways, no raw key and no English text of
// the dictionary is left on screen. Against the fake engine, in headless Brave with a temporary
// profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/languages.mjs <shots dir> [port] [lang …]
//
// Languages default to de. Covered: the window at 1100×680 and 960×600 (Channel with requests
// waiting, menus, History, Share, the permanent link wizard, every Settings section, the
// shortcuts dialog, the stats overlay, notices), the remote panel on a phone (390×844, sign-in
// too), the request page at 320 and 390 px, and the first-run wizard (every step) at 960×600.
// Exits 1 on the first failed check.

import { mkdtempSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';
import { makeTestStream } from './hls-fixture.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8194);
const languages = process.argv.length > 4 ? process.argv.slice(4) : ['de'];
if (!shots) throw new Error('usage: languages.mjs <shots dir> [port] [lang …]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

const LOCALES = join(fileURLToPath(new URL('.', import.meta.url)), '..', '..', 'public', 'locales');
const dict = (lang) => JSON.parse(readFileSync(join(LOCALES, `${lang}.json`), 'utf8'));
const en = dict('en');
const texts = (v) => (typeof v === 'string' ? [v] : Object.values(v));

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` — ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}

const stream = await makeTestStream();
const engine = await startFakeEngine(port, { hlsDir: stream.dir });
const base = `http://127.0.0.1:${port}`;
const profiles = [];
const contexts = [];
async function browser(locale, viewport) {
  const profile = mkdtempSync(join(tmpdir(), 'brave-languages-'));
  profiles.push(profile);
  const ctx = await chromium.launchPersistentContext(profile, {
    executablePath: BRAVE,
    headless: true,
    locale,
    viewport,
    args: ['--disable-extensions', '--no-first-run'],
  });
  contexts.push(ctx);
  return ctx;
}

const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const scenario = (s) => call('/__test/scenario', s);
const reset = () => call('/__test/reset', {});
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

const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
  window.open = () => null;
};

/**
 * What a screen shows wrong: elements whose text does not fit (a button, a label, a tab, a menu
 * item, a heading, a pill …; deliberately shortened texts with an ellipsis and text fields do not
 * count), a page wider than the window, raw keys, and texts that are the English text of a key
 * whose translation differs.
 */
function problemsOnScreen({ english, ignore, namespaces }) {
  const visible = (e) => {
    const r = e.getBoundingClientRect();
    const s = getComputedStyle(e);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
  };
  const cut = [...document.querySelectorAll('button, a, label, summary, select, h1, h2, h3, h4, h5, [role=tab], [role=menuitem], [role=radio], .rail-item, .rail-label, .pill, .badge, .tag, .set-row *, .dialog *, .menu *, .stats *, .banner *, .wz *, .v-head *, .tool-row *')]
    .filter((e) => visible(e) && !e.matches('input, textarea, svg, svg *, img, video, canvas, [role=slider], [role=progressbar], .sr-only'))
    .filter((e) => getComputedStyle(e).textOverflow !== 'ellipsis')
    .filter((e) => e.scrollWidth > e.clientWidth + 1 || (e.matches('button, .rail-item, .pill') && e.scrollHeight > e.clientHeight + 1))
    .filter((e) => getComputedStyle(e).overflowX !== 'auto' && getComputedStyle(e).overflowY !== 'auto')
    .map((e) => `${e.tagName.toLowerCase()}.${String(e.className).trim().replace(/\s+/g, '.')} "${e.textContent.trim().slice(0, 60)}" ${e.scrollWidth}×${e.scrollHeight} > ${e.clientWidth}×${e.clientHeight}`);
  const view = document.querySelector('#view');
  const wide = document.documentElement.scrollWidth > window.innerWidth + 1 || (view && view.scrollWidth > view.clientWidth + 1);
  // What sticks out on the right (the culprits of a sideways scroll).
  const scrolled = (e) => { for (let a = e.parentElement; a && a !== view; a = a.parentElement) if (/auto|scroll/.test(getComputedStyle(a).overflowX)) return true; return false; };
  const sideways = wide ? [...document.querySelectorAll("body *")].filter((e) => visible(e) && !scrolled(e) && e.getBoundingClientRect().right > (view ? view.getBoundingClientRect().right : window.innerWidth) + 1).slice(-5).map((e) => `${e.tagName.toLowerCase()}.${String(e.className).trim()} "${e.textContent.trim().slice(0, 40)}" right ${Math.round(e.getBoundingClientRect().right)}`) : null;
  const shown = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const node = walker.currentNode;
    const t = node.textContent.trim();
    if (t && node.parentElement && visible(node.parentElement) && !node.parentElement.closest('script, style')) shown.push(t);
  }
  for (const e of document.querySelectorAll('[aria-label], [title], [placeholder]')) {
    for (const a of ['aria-label', 'title', 'placeholder']) if (e.getAttribute(a)) shown.push(e.getAttribute(a).trim());
  }
  const raw = shown.filter((t) => /^[a-z_]+(\.[a-z0-9_]+)+$/.test(t) && namespaces.includes(t.split('.')[0]));
  const set = new Set(english);
  const untranslated = [...new Set(shown.filter((t) => set.has(t) && !ignore.includes(t)))];
  return { cut, sideways, raw, untranslated };
}

// Texts the fake engine answers in English whatever the language (the engine localizes them).
const FAKE_ENGLISH = ['Skipped by the owner'];

async function assertScreen(page, lang, where, { ignore = FAKE_ENGLISH } = {}) {
  const tr = dict(lang);
  // English texts of the keys whose translation differs (a text the user would read in English),
  // except those that are also a text of the language (German "Pause" is English "Pause").
  const own = new Set(Object.values(tr).flatMap(texts));
  const english = Object.keys(en)
    .filter((k) => JSON.stringify(tr[k]) !== JSON.stringify(en[k]))
    .flatMap((k) => texts(en[k]))
    .filter((t) => !t.includes('{') && t.length > 2 && !own.has(t));
  const namespaces = [...new Set(Object.keys(en).map((k) => k.split('.')[0]))];
  const p = await page.evaluate(problemsOnScreen, { english, ignore, namespaces });
  check(p.cut.length === 0, `${lang} ${where}: no text is cut`, p.cut);
  check(!p.sideways, `${lang} ${where}: no horizontal scrolling`, p.sideways);
  check(p.raw.length === 0, `${lang} ${where}: no raw key on screen`, p.raw);
  check(p.untranslated.length === 0, `${lang} ${where}: no English text left`, p.untranslated);
}

async function openPage(ctx, path, { inWindow = true, width = 1100, height = 680, wait = '.player .play', storedLang = null } = {}) {
  const page = await ctx.newPage();
  await page.setViewportSize({ width, height });
  if (inWindow) await page.addInitScript(FAKE_TAURI);
  if (storedLang) await page.addInitScript((l) => localStorage.setItem('vrcinema.lang', l), storedLang);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.__errors = errors;
  await page.goto(`${base}${path}`);
  if (wait) await page.waitForSelector(wait);
  return page;
}
const setLanguage = (page, code) => page.evaluate(async (c) => (await import('/static/app/store.js')).setLanguage(c), code);
const shot = (page, name) => page.screenshot({ path: join(shots, `${name}.png`) });
const clickText = (page, label) => page.click(`#view button:has-text("${label}")`);

async function windowViews(lang, tr) {
  const ctx = await browser('en-US', { width: 1100, height: 680 });
  await reset();
  await scenario({ language: lang, pending: 2 });
  const page = await openPage(ctx, '/admin');
  await page.waitForSelector('.approvals .qi');
  await page.waitForFunction((t) => document.querySelector('.rail-channel .rail-label')?.textContent === t, tr['nav.channel']);
  for (const [w, h] of [[1100, 680], [960, 600]]) {
    await page.setViewportSize({ width: w, height: h });
    await page.evaluate(() => (location.hash = '#/'));
    await page.waitForSelector('.approvals .qi');
    await sleep(300);
    await assertScreen(page, lang, `channel ${w}×${h}`);
    await shot(page, `${lang}-channel-${w}x${h}`);

    // The player menu and a queue row's menu.
    await page.click('.nowp', { button: 'right' });
    await page.waitForSelector('.menu [role=menuitem]');
    await assertScreen(page, lang, `player menu ${w}×${h}`);
    await shot(page, `${lang}-player-menu-${w}x${h}`);
    await page.keyboard.press('Escape');
    await page.click('.q .qi-queued >> nth=0', { button: 'right' });
    await page.waitForSelector('.menu [role=menuitem]');
    await assertScreen(page, lang, `queue menu ${w}×${h}`);
    await shot(page, `${lang}-queue-menu-${w}x${h}`);
    await page.keyboard.press('Escape');

    // The shortcuts dialog and the stats overlay.
    await page.click('.view');
    await page.keyboard.press('Shift+Slash');
    await page.waitForSelector('.dialog');
    await assertScreen(page, lang, `shortcuts dialog ${w}×${h}`);
    await shot(page, `${lang}-shortcuts-${w}x${h}`);
    await page.keyboard.press('Escape');
    await page.keyboard.press('i');
    await page.waitForSelector('.stats');
    await sleep(400);
    await assertScreen(page, lang, `stats overlay ${w}×${h}`);
    await shot(page, `${lang}-stats-${w}x${h}`);
    await page.keyboard.press('i');

    for (const view of ['history', 'share']) {
      await page.evaluate((v) => (location.hash = `#/${v}`), view);
      await page.waitForSelector(`#view[data-view="${view}"]`);
      await sleep(350);
      await assertScreen(page, lang, `${view} ${w}×${h}`);
      await shot(page, `${lang}-${view}-${w}x${h}`);
    }
    // The permanent link wizard (Share), step 1 and step 2.
    await clickText(page, tr['permalink.setup']);
    await page.waitForSelector('.pl-wizard');
    await assertScreen(page, lang, `permanent link wizard ${w}×${h}`);
    // Once connected (the first round), the wizard starts at the address.
    if (await page.$(`#view button:has-text("${tr['permalink.connect']}")`)) await clickText(page, tr['permalink.connect']);
    await page.waitForSelector('.pl-input', { timeout: 6000 });
    await page.fill('.pl-input', 'taken.example.com');
    await clickText(page, tr['permalink.create']);
    await page.waitForSelector(`#view button:has-text("${tr['permalink.overwrite']}")`, { timeout: 6000 });
    await assertScreen(page, lang, `permanent link name taken ${w}×${h}`);
    await shot(page, `${lang}-permalink-${w}x${h}`);
    await page.reload();
    await page.waitForSelector('.player .play');

    for (const section of ['general', 'internet', 'channel', 'tools', 'remote', 'advanced', 'log', 'about']) {
      await page.evaluate((s) => (location.hash = `#/settings/${s}`), section);
      await page.waitForSelector(`#view[data-section="${section}"]`);
      await sleep(400);
      // The log lines and the version notes come from the program as they are.
      await assertScreen(page, lang, `settings ${section} ${w}×${h}`);
      await shot(page, `${lang}-settings-${section}-${w}x${h}`);
    }
  }
  check(page.__errors.length === 0, `${lang} window: no errors in the console`, page.__errors);
  await page.close();

  // Notices at the top, a dialog of a session that came through the tunnel.
  await reset();
  await scenario({ language: lang, toolsMissing: ['ffmpeg', 'deno'], tickerSlow: true, updateOffer: { version: '0.3.0', notes: '' }, tunnel: 'error' });
  const noticed = await openPage(ctx, '/admin', { width: 960, height: 600 });
  await sleep(1500);
  await assertScreen(noticed, lang, 'notices 960×600');
  await shot(noticed, `${lang}-notices-960x600`);
  await noticed.close();
}

async function phoneViews(lang, tr) {
  const ctx = await browser('en-US', { width: 390, height: 844 });
  await reset();
  await scenario({ remote: true, signedOut: true, language: lang });
  const login = await openPage(ctx, '/admin', { inWindow: false, width: 390, height: 844, wait: '.login-card input[type=password]', storedLang: lang });
  await sleep(300);
  await assertScreen(login, lang, 'phone sign-in 390×844');
  await shot(login, `${lang}-phone-login-390x844`);
  await login.close();

  await reset();
  await scenario({ remote: true, sessionViaTunnel: true, pending: 1, language: lang });
  const page = await openPage(ctx, '/admin', { inWindow: false, width: 390, height: 844, storedLang: lang });
  await page.waitForFunction((t) => document.body.innerText.includes(t), tr['queue.title']);
  for (const hash of ['#/', '#/history', '#/share', '#/settings', '#/settings/general', '#/settings/internet', '#/settings/channel', '#/settings/tools', '#/settings/remote', '#/settings/advanced', '#/settings/log', '#/settings/about']) {
    await page.evaluate((h) => (location.hash = h), hash);
    await sleep(500);
    const name = hash.replace('#/', '').replace('/', '-') || 'channel';
    await assertScreen(page, lang, `phone ${name} 390×844`);
    await shot(page, `${lang}-phone-${name}-390x844`);
  }
  // The question before switching the tunnel off from a session that came through it.
  await page.evaluate(() => (location.hash = '#/settings/internet'));
  await sleep(400);
  await page.click(`button[role=switch][aria-label="${tr['settings.internet.tunnel']}"]`);
  await page.waitForSelector('.dialog');
  await assertScreen(page, lang, 'phone lose-access dialog 390×844');
  await shot(page, `${lang}-phone-lose-access-390x844`);
  await page.keyboard.press('Escape');
  await page.close();
}

async function viewerPage(lang, tr) {
  for (const width of [320, 390]) {
    const ctx = await browser(lang, { width, height: 760 });
    await reset();
    await scenario({ playing: 'film', settingsPatch: { linkMode: 'allowlist', moderation: true } });
    const page = await openPage(ctx, '/', { inWindow: false, width, height: 760, wait: '.v-now' });
    check((await page.evaluate(() => document.documentElement.lang)) === lang, `${lang} request page starts in ${lang}`);
    await page.evaluate(() => document.querySelectorAll('details').forEach((d) => (d.open = true)));
    await sleep(300);
    await assertScreen(page, lang, `request page ${width} px`);
    await page.screenshot({ path: join(shots, `${lang}-viewer-${width}.png`), fullPage: true });
    // A refused nick and its suggestion.
    await scenario({ viewerNickTaken: ['tomek'] });
    await page.fill('input[name=url]', 'https://www.youtube.com/watch?v=x');
    await page.fill('input[name=nick]', 'Tomek');
    await page.click('.v-submit');
    await page.waitForSelector('.v-error');
    await sleep(200);
    await assertScreen(page, lang, `request page, nick taken ${width} px`);
    await page.screenshot({ path: join(shots, `${lang}-viewer-nick-taken-${width}.png`), fullPage: true });
    await scenario({ viewerNickTaken: [] });
    check(page.__errors.length === 0, `${lang} request page: no errors`, page.__errors);
    await page.close();
  }
}

async function wizard(lang, tr) {
  const ctx = await browser('en-US', { width: 960, height: 600 });
  await reset();
  await scenario({ firstRun: true, language: lang, tunnel: 'off', toolsMissing: ['yt-dlp', 'ffmpeg', 'deno', 'cloudflared'] });
  const page = await openPage(ctx, '/wizard', { width: 960, height: 600, wait: '.wz-lang' });
  await waitFor(async () => (await page.locator('h1').first().innerText()) === tr['wizard.welcome_title'], 'wizard in the language');
  await assertScreen(page, lang, 'wizard step 1 960×600');
  await shot(page, `${lang}-wizard-1`);
  await page.click('[data-next]');
  await page.waitForSelector('.tool-row[data-kind=active]');
  await assertScreen(page, lang, 'wizard step 2 (downloading) 960×600');
  await shot(page, `${lang}-wizard-2-downloading`);
  await waitFor(() => page.locator('[data-next]').count(), 'tools done', 20000);
  await assertScreen(page, lang, 'wizard step 2 (done) 960×600');
  await page.click('[data-next]');
  await page.waitForSelector('.wz-result');
  await assertScreen(page, lang, 'wizard step 3 960×600');
  await shot(page, `${lang}-wizard-3`);
  await page.click('[data-next]');
  await waitFor(() => page.locator('[data-tunnel=online]').count(), 'online', 10000);
  await sleep(400);
  await assertScreen(page, lang, 'wizard step 4 online 960×600');
  await shot(page, `${lang}-wizard-4-online`);
  check(page.__errors.length === 0, `${lang} wizard: no errors`, page.__errors);
  await page.close();
}

try {
  for (const lang of languages) {
    const tr = dict(lang);
    await windowViews(lang, tr);
    await phoneViews(lang, tr);
    await viewerPage(lang, tr);
    await wizard(lang, tr);
  }
  console.log(`\n${passed} checks passed`);
} finally {
  for (const ctx of contexts) await ctx.close().catch(() => {});
  for (const p of profiles) rmSync(p, { recursive: true, force: true });
  await engine.close();
  stream.cleanup();
}
