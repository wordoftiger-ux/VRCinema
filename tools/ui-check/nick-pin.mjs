// Browser check of the nick PIN: on the request page, setting a PIN (the recovery code shown
// once), "It's my nick" after a taken nick, adopting with the PIN on a new device (a new
// origin starts with no device key), the recovery code path, the texts in Polish, the layout at
// 320 px; in the panel (Settings → Channel → Viewer nicks) the PIN tag and Reset PIN with its
// confirmation — in headless Brave with temporary profiles, against the fake engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/nick-pin.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: nick-pin.mjs <shots dir> [port]');
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
const profiles = [];
const contexts = [];
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const scenario = (s) => call('/__test/scenario', s);
const posts = async (path) => (await call('/__test/requests')).filter((r) => r.method === 'POST' && r.path === path);
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

async function browser(locale, viewport = { width: 520, height: 1000 }) {
  const profile = mkdtempSync(join(tmpdir(), 'brave-nickpin-'));
  profiles.push(profile);
  const ctx = await chromium.launchPersistentContext(profile, {
    executablePath: BRAVE, headless: true, locale, viewport, args: ['--disable-extensions', '--no-first-run'],
  });
  await ctx.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: base });
  contexts.push(ctx);
  return ctx;
}

async function open(ctx, path = '/') {
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !m.text().includes('status of 4') && errors.push(m.text()));
  await page.goto(base + path);
  page.__errors = errors;
  return page;
}

const RAW = /\b(viewer|errors|settings)\.[a-z_]+/;
const noRawKeys = (page) => page.evaluate((src) => !new RegExp(src).test(document.body.innerText), RAW.source);
const noOverflow = (page) => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);

try {
  // ---------- Setting a PIN ----------
  const first = await browser('en-US');
  let page = await open(first);
  await page.waitForSelector('.v-now');
  check((await page.locator('details.v-pin').getAttribute('open')) === null, 'the PIN section starts closed');
  await page.click('details.v-pin summary');
  await page.click('[data-tab=set]');
  // An empty nick is refused on the page itself.
  await page.fill('[data-form=set] input[name=pin]', '2580');
  await page.click('[data-form=set] .v-submit');
  await page.waitForSelector('.v-pin .v-error[data-code=nick_pin_no_nick]');
  check((await page.locator('.v-pin .v-error').innerText()).includes('Type your nick first'), 'no nick: asked to type it');
  check((await posts('/api/channels/main/nick/protect')).length === 0, 'nothing sent without a nick');
  await page.fill('input[name=nick]', 'Kasia');
  await page.fill('[data-form=set] input[name=pin]', '2580');
  check((await page.locator('[data-form=set] input[name=pin]').getAttribute('type')) === 'password', 'the PIN field hides the PIN');
  await page.click('[data-form=set] .v-submit');
  await page.waitForSelector('[data-recovery]');
  const code = await page.locator('[data-recovery] input').inputValue();
  check(/^CODE-\d{4}-ABCD-EFGH$/.test(code), 'the recovery code is shown', code);
  const sent = (await posts('/api/channels/main/nick/protect')).at(-1).body;
  check(sent.name === 'Kasia' && sent.pin === '2580' && /^[0-9a-f]{32}$/.test(sent.deviceKey), 'protect sends nick, PIN and device key', sent);
  check((await page.evaluate(() => Object.values(localStorage).some((v) => v.includes('2580') || v.includes('CODE-')))) === false, 'neither the PIN nor the code is stored on the page');
  check((await page.locator('.v-own-badge').count()) === 1, 'the icon is shown next to the nick');
  await page.click('[data-action=copy-code]');
  await waitFor(async () => (await page.locator('[data-action=copy-code]').innerText()) === 'Code copied', 'Copy confirms');
  check((await page.evaluate(() => navigator.clipboard.readText())) === code, 'the code is on the clipboard');
  await page.screenshot({ path: join(shots, 'pin-code-en.png'), fullPage: true });
  await page.click('[data-action=code-done]');
  check((await page.locator('[data-recovery]').count()) === 0, 'Done hides the code for good');
  // Changing it asks for the current PIN.
  await page.fill('[data-form=set] input[name=pin]', '86420');
  await page.click('[data-form=set] .v-submit');
  await page.waitForSelector('[data-form=set] input[name=currentPin]');
  check((await page.locator('.v-pin .v-error').getAttribute('data-code')) === 'nick_pin_required', 'a second PIN needs the current one');
  check(page.__errors.length === 0, 'no errors in the console (set)', page.__errors);

  // ---------- A new device (or a new tunnel address): taken, then adopted with the PIN ----------
  const second = await browser('pl-PL', { width: 320, height: 900 });
  page = await open(second);
  await page.waitForSelector('.v-now');
  await page.fill('input[name=url]', 'https://www.youtube.com/watch?v=x');
  await page.fill('input[name=nick]', 'kasia');
  await page.click('form .v-submit');
  await page.waitForSelector('.v-error[data-code=nick_taken] [data-action=its-mine]');
  check((await page.locator('[data-action=its-mine]').innerText()).includes('To mój nick'), 'Polish: “To mój nick: wpisz PIN”');
  await page.click('[data-action=its-mine]');
  await waitFor(() => page.locator('details.v-pin').evaluate((d) => d.open), 'the section opens');
  check((await page.locator('[data-tab=adopt]').getAttribute('aria-pressed')) === 'true', 'on the “I have a PIN” tab');
  await waitFor(() => page.evaluate(() => document.activeElement?.name === 'pin'), 'the PIN field has the focus');
  await page.fill('[data-form=adopt] input[name=pin]', '1111');
  await page.click('[data-form=adopt] .v-submit');
  await page.waitForSelector('.v-pin .v-error[data-code=nick_pin_wrong]');
  check((await page.locator('.v-pin .v-error').innerText()).includes('nie pasują'), 'a wrong PIN: the Polish error');
  await page.fill('[data-form=adopt] input[name=pin]', '2580');
  await page.check('[data-form=adopt] input[name=signOutOthers]');
  await page.click('[data-form=adopt] .v-submit');
  await waitFor(async () => (await page.locator('input[name=nick]').inputValue()) === 'Kasia', 'the nick as the channel writes it');
  const adopted = (await posts('/api/channels/main/nick/adopt')).at(-1).body;
  check(adopted.pin === '2580' && adopted.signOutOthers === true && adopted.name === 'kasia', 'adopt sends the PIN and the sign-out choice', adopted);
  check((await page.evaluate(() => localStorage.getItem('vrcinema.viewer.nick'))) === 'Kasia', 'the nick is saved for this device');
  check(await noOverflow(page), 'no horizontal scroll at 320 px');
  check(await noRawKeys(page), 'no raw keys (pl)');
  await page.screenshot({ path: join(shots, 'pin-adopt-pl-320.png'), fullPage: true });
  // The request now goes through.
  await page.click('form .v-submit');
  await waitFor(async () => (await posts('/api/channels/main/requests')).some((r) => r.body.name === 'Kasia' && r.body.deviceKey === adopted.deviceKey), 'the adopted nick is used for a request');

  // ---------- The recovery code ----------
  const third = await browser('en-US');
  page = await open(third);
  await page.waitForSelector('.v-now');
  await page.fill('input[name=nick]', 'Kasia');
  await page.click('details.v-pin summary');
  await page.click('[data-tab=adopt]');
  await page.click('[data-action=toggle-code]');
  check((await page.locator('[data-form=adopt] input[name=signOutOthers]').count()) === 0, 'no sign-out choice with the code (it always signs out)');
  await page.fill('[data-form=adopt] input[name=recoveryCode]', code.toLowerCase());
  await page.fill('[data-form=adopt] input[name=newPin]', '97531');
  await page.click('[data-form=adopt] .v-submit');
  await page.waitForSelector('[data-recovery]');
  const next = await page.locator('[data-recovery] input').inputValue();
  check(next !== code && /^CODE-/.test(next), 'a new code is shown after using the old one', next);
  check(page.__errors.length === 0, 'no errors in the console (recovery)', page.__errors);

  // ---------- Every language: no raw keys in the section ----------
  for (const lang of ['de', 'es', 'fr', 'ja']) {
    await page.locator('.v-lang select').selectOption(lang);
    await waitFor(() => page.evaluate((l) => document.documentElement.lang === l, lang), `${lang} loaded`);
    await page.click('[data-action=code-done]').catch(() => {});
    check(await noRawKeys(page), `no raw keys (${lang})`);
  }

  // ---------- The panel: the PIN tag and Reset PIN ----------
  const owner = await browser('en-US', { width: 1100, height: 760 });
  const admin = await owner.newPage();
  await admin.addInitScript(() => {
    window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
  });
  await admin.goto(`${base}/admin#/settings/channel`);
  await admin.waitForSelector('.nick-row[data-skeleton="kasia"] [data-pin]');
  check((await admin.locator('.nick-row[data-skeleton="kasia"] [data-pin]').innerText()) === 'PIN, 2 devices', 'the PIN tag with its devices');
  check((await admin.locator('.nick-row[data-skeleton="troll"] [data-action=reset-pin]').count()) === 0, 'no Reset PIN without a PIN');
  await admin.click('.nick-row[data-skeleton="kasia"] [data-action=reset-pin]');
  await admin.waitForSelector('.dialog');
  check((await admin.locator('.dialog').innerText()).includes('loses its PIN and recovery code'), 'the confirmation explains the reset');
  await admin.screenshot({ path: join(shots, 'pin-reset-dialog.png') });
  await admin.click('.dialog button:has-text("Reset PIN")');
  await waitFor(async () => (await posts('/api/admin/nicks/kasia/reset-pin')).length === 1, 'reset posts the skeleton');
  await admin.waitForFunction(() => !document.querySelector('.nick-row[data-skeleton="kasia"] [data-pin]'));
  check((await admin.locator('[data-nicks]').innerText()).includes('The PIN of Kasia is reset.'), 'the confirmation note');
  await admin.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await waitFor(async () => (await admin.locator('[data-nicks]').innerText()).includes('Resetuj PIN') === false
    && (await admin.evaluate(() => document.documentElement.lang)) === 'pl', 'the panel in Polish');
  check(await admin.evaluate((src) => !new RegExp(src).test(document.querySelector('#view').innerText), RAW.source), 'panel: no raw keys (pl)');

  console.log(`\n${passed} checks passed`);
} finally {
  for (const c of contexts) await c.close().catch(() => {});
  await engine.close();
  for (const p of profiles) rmSync(p, { recursive: true, force: true });
}
process.exit(0);
