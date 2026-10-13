// Browser check of the History and Share views (history rows and their actions, the QR code
// following the address, the internet card, the permanent link wizard against the fake
// engine's stand-in for cloudflared) in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/history-share.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8196);
if (!shots) throw new Error('usage: history-share.mjs <shots dir> [port]');
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
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
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
  // The window opens the login page itself when cloudflared did not: count it instead.
  window.__opened = [];
  window.open = (url) => {
    window.__opened.push(url);
    return null;
  };
};

async function waitFor(fn, what, timeout = 6000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}
const mark = async () => (await requests()).length;
const since = async (m) => (await requests()).slice(m);
const actionsSince = async (m) => (await since(m)).filter((r) => r.method !== 'GET');
const lastAction = (m, what) => waitFor(async () => (await actionsSince(m)).at(-1), what);

async function openApp(hash, { remote = false } = {}) {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  page.on('response', (r) => r.status() >= 400 && !r.url().includes('/api/') && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${base}/app${hash}`);
  await page.waitForSelector('.player .play');
  page.__errors = errors;
  page.__remote = remote;
  return page;
}

const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const bodyText = (page) => page.$eval('#view', (e) => e.innerText);
const setLanguage = (page, code) => page.evaluate(async (c) => (await import('/static/app/store.js')).setLanguage(c), code);
const noRawKeys = (page) => page.$eval('#view', (e) => !/\b(history|share|permalink|ui|net|errors)\.[a-z_]+/.test(e.innerText));
const noOverflow = (page) => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);
const clickButton = (page, label) => page.click(`#view button:has-text("${label}")`);
const hasButton = async (page, label) => (await page.$(`#view button:has-text("${label}")`)) !== null;

try {
  // ---------- History ----------
  await reset();
  let page = await openApp('#/history');
  await page.waitForSelector('.hist-row');
  await page.screenshot({ path: join(shots, 'after-history-en-1100x680.png') });
  const rows = await page.$$eval('.hist-row', (r) => r.map((e) => ({ id: Number(e.dataset.id), outcome: e.dataset.outcome, title: e.querySelector('.qi-name').textContent, status: e.querySelector('.stt').textContent, sub: e.querySelector('.qi-sub').textContent, buttons: [...e.querySelectorAll('button')].map((b) => b.textContent.trim()), disabled: [...e.querySelectorAll('button')].map((b) => b.disabled) })));
  check(rows.length === 5, 'the history lists its five videos', rows.length);
  check(JSON.stringify(rows.map((r) => r.outcome)) === '["played","skipped","failed","played","failed"]', 'outcomes: played, skipped (played with a reason), failed', rows.map((r) => r.outcome));
  check(JSON.stringify(rows.map((r) => r.status)) === '["Played","Skipped","Error","Played","Error"]', 'status labels', rows.map((r) => r.status));
  check(rows[2].title === 'Untitled', 'the stored default title "Bez tytułu" shows the localized "Untitled"', rows[2].title);
  check(rows[0].buttons.join('|') === 'Play now|Add to queue' && rows[2].buttons.join('|') === 'Try again|Add to queue', 'buttons: Play now / Try again, Add to queue', rows.map((r) => r.buttons));
  check(rows[3].disabled[1] === true && rows[0].disabled[1] === false, 'a file from disk cannot be added to the queue again');
  check(rows[2].sub.includes('The file does not exist') && rows[4].sub.includes('Nie udało się pobrać filmu (stary wpis).'), 'a failure shows its reason; an old plain-text reason is shown as it is', [rows[2].sub, rows[4].sub]);
  check(rows[0].sub.startsWith('5:20 · Tomek'), 'details: length · requester · time', rows[0].sub);
  check(!rows[1].sub.includes('Skipped by the owner') && (await page.$eval('.hist-row[data-id="2"] .stt', (e) => e.title)) === 'Skipped by the owner', 'a skipped video keeps its reason in the label tooltip');
  check(await noOverflow(page), 'no horizontal overflow at 1100 px');

  let m = await mark();
  await page.click('.hist-row[data-id="1"] .hist-play');
  let act = await lastAction(m, 'play now request');
  check(act.method === 'POST' && act.path === '/api/admin/channels/main/play-now' && act.body.itemId === 1, 'Play now asks the engine to play that history item', act);
  m = await mark();
  await page.click('.hist-row[data-id="3"] .hist-play');
  act = await lastAction(m, 'retry request');
  check(act.path === '/api/admin/channels/main/play-now' && act.body.itemId === 3, 'Try again on a failed video plays it again through the same route', act);
  m = await mark();
  await page.click('.hist-row[data-id="3"] .hist-queue');
  act = await lastAction(m, 'add link request');
  check(act.path === '/api/admin/channels/main/items' && act.body.url === 'https://pixeldrain.com/u/aB3x', 'Add to queue adds the video\'s link', act);
  await page.waitForSelector('.toast');
  check((await text(page, '.toast')).startsWith('Added'), 'a message says it was added', await text(page, '.toast'));

  await setLanguage(page, 'pl');
  await page.waitForFunction(() => document.querySelector('.hist-row .stt')?.textContent === 'Odtworzony');
  const pl = await bodyText(page);
  check(pl.includes('Spróbuj ponownie') && pl.includes('Do kolejki') && pl.includes('Pominięty') && pl.includes('Błąd') && pl.includes('Odtwórz teraz'), 'Polish history', pl);
  check(await noRawKeys(page), 'no raw keys in Polish History');
  check((await text(page, '.hist-row[data-id="3"] .qi-name')) === 'Bez tytułu', 'the Polish placeholder for an untitled video');
  await setLanguage(page, 'en');

  await scenario({ history: [] });
  await page.waitForSelector('.q-empty', { timeout: 4000 });
  check((await text(page, '.q-empty')) === 'Nothing has been on the channel yet.', 'an empty history says so');
  check(page.__errors.length === 0, 'no console errors on History', page.__errors);
  await page.close();

  // ---------- Share: the window ----------
  await reset();
  page = await openApp('#/share');
  await page.waitForSelector('.qr-img');
  await page.waitForFunction(() => document.querySelector('.qr-img')?.complete && document.querySelector('.qr-img').naturalWidth > 0);
  await page.screenshot({ path: join(shots, 'after-share-en-1100x680.png') });
  const addr = 'https://proba-kanalu.trycloudflare.com';
  check((await text(page, '[data-link="share-channel"]')) === `${addr}/live/main.m3u8`, 'channel link', await text(page, '[data-link="share-channel"]'));
  check((await text(page, '[data-link="share-request"]')) === `${addr}/`, 'request page link', await text(page, '[data-link="share-request"]'));
  const open = await page.$eval('a.btn:has-text("Open")', (a) => [a.getAttribute('href'), a.target, a.rel].join(' '));
  check(open === `${addr}/ _blank noopener`, 'Open goes to the request page in a new tab', open);
  await page.click('#view .field:has([data-link="share-channel"]) button');
  check((await page.evaluate(() => navigator.clipboard.readText())) === `${addr}/live/main.m3u8`, 'Copy puts the channel link on the clipboard');
  check((await text(page, '#view .field:has([data-link="share-channel"]) button')) === 'Copied', 'the button says Copied');
  await page.click('#view .field:has([data-link="share-request"]) button');
  check((await page.evaluate(() => navigator.clipboard.readText())) === `${addr}/`, 'Copy puts the request page link on the clipboard');
  check((await text(page, '#share-internet .pill')) === 'Online', 'the internet card shows the state');
  check((await bodyText(page)).includes('Quick link: the address is random'), 'quick mode text');
  const qrFor = () => page.$eval('.qr-img', (i) => decodeURIComponent(new URL(i.src).searchParams.get('for')));
  check((await qrFor()) === `${addr}/`, 'the QR picture is asked for the request page address', await qrFor());
  check(await noOverflow(page), 'no horizontal overflow on Share at 1100 px');

  // The QR follows the address.
  m = await mark();
  await scenario({ tunnelUrl: 'https://druga-nazwa.trycloudflare.com' });
  await waitFor(async () => (await qrFor()) === 'https://druga-nazwa.trycloudflare.com/', 'QR follows the address', 6000);
  const qrReq = (await since(m)).filter((r) => r.method === 'GET' && r.path === '/api/desktop/tunnel/qr');
  check(qrReq.length >= 1, 'a changed address loads the QR code again', qrReq.length);
  await page.waitForFunction(() => document.querySelector('.qr-img')?.complete && document.querySelector('.qr-img').naturalWidth > 0);
  check((await text(page, '[data-link="share-request"]')) === 'https://druga-nazwa.trycloudflare.com/', 'and the links');

  // Reconnect rotates the quick address.
  m = await mark();
  await clickButton(page, 'Reconnect');
  act = await lastAction(m, 'restart request');
  check(act.method === 'POST' && act.path === '/api/desktop/tunnel/restart', 'Reconnect restarts the tunnel', act);
  await waitFor(async () => (await qrFor()) === 'https://proba-kanalu-1.trycloudflare.com/', 'QR after reconnect');
  check(true, 'after Reconnect the QR shows the new address');

  // No public address: no QR request, a note instead.
  await scenario({ tunnel: 'off' });
  await page.waitForSelector('.qr-none', { timeout: 6000 });
  m = await mark();
  await sleep(1500);
  check((await since(m)).every((r) => r.path !== '/api/desktop/tunnel/qr'), 'without an address the QR is not requested');
  check((await bodyText(page)).includes('The tunnel is off'), 'a note says the tunnel is off');
  check(!(await hasButton(page, 'Reconnect')) && !(await hasButton(page, 'Set up a permanent link')), 'no tunnel buttons with the tunnel off');
  await scenario({ tunnel: 'error' });
  await page.waitForFunction(() => document.querySelector('[aria-labelledby="share-internet"]')?.textContent.includes('Reason: No internet'), null, { timeout: 6000 });
  check((await text(page, '#share-internet .pill')) === 'No internet', 'an error shows its reason');
  await scenario({ tunnel: 'online' });
  await page.waitForSelector('.qr-img', { timeout: 6000 });

  await setLanguage(page, 'pl');
  await page.waitForFunction(() => document.querySelector('#share-channel')?.textContent === 'Link kanału');
  const plShare = await bodyText(page);
  check(plShare.includes('Strona zgłoszeń') && plShare.includes('Kod QR') && plShare.includes('Ustaw stały link') && plShare.includes('Połącz ponownie'), 'Polish Share', plShare);
  check(await noRawKeys(page), 'no raw keys in Polish Share');
  await setLanguage(page, 'en');
  check(page.__errors.length === 0, 'no console errors on Share', page.__errors);
  await page.close();

  // ---------- Share: a remote session ----------
  await reset();
  await scenario({ remote: true });
  page = await openApp('#/share', { remote: true });
  await page.waitForSelector('.qr-img');
  await page.waitForFunction(() => document.querySelector('.qr-img')?.complete && document.querySelector('.qr-img').naturalWidth > 0);
  const remoteReqs = await requests();
  check(remoteReqs.some((r) => r.path === '/api/admin/qr'), 'a remote session asks the panel route for the QR code');
  check(!remoteReqs.some((r) => r.path === '/api/desktop/tunnel/qr'), 'and never the window-only one');
  check((await text(page, '[data-link="share-channel"]')) === `${base}/live/main.m3u8`, 'remote: the channel link comes from the panel', await text(page, '[data-link="share-channel"]'));
  const windowOnly = await text(page, '.window-only');
  check(windowOnly === 'Available in the program on the computer', 'window-only parts say so', windowOnly);
  check(!(await hasButton(page, 'Reconnect')) && !(await hasButton(page, 'Set up a permanent link')), 'remote: no tunnel buttons');
  await page.screenshot({ path: join(shots, 'after-share-remote-en-1100x680.png') });
  check(page.__errors.filter((e) => !e.includes('403')).length === 0, 'no console errors in the remote Share', page.__errors);
  await page.close();

  // ---------- The permanent link wizard ----------
  const wizardPage = async (s = {}) => {
    await reset();
    await scenario(s);
    const p = await openApp('#/share');
    await p.waitForSelector('[aria-labelledby="share-internet"] button');
    return p;
  };
  const wizText = (p) => p.$eval('.pl-wizard', (e) => e.innerText);
  const opened = (p) => p.evaluate(() => window.__opened.slice());

  // The good path.
  page = await wizardPage();
  await clickButton(page, 'Set up a permanent link');
  await page.waitForSelector('.pl-wizard');
  check((await wizText(page)).includes('Step 1 of 2') && (await hasButton(page, 'Connect to Cloudflare')), 'step 1: connect to Cloudflare');
  m = await mark();
  await clickButton(page, 'Connect to Cloudflare');
  act = await lastAction(m, 'login request');
  check(act.path === '/api/desktop/tunnel/named/login' && act.body === null, 'Connect starts the login', act);
  await page.waitForSelector('.pl-wizard a[href^="https://dash.cloudflare.com"]');
  check((await opened(page)).length === 1 && (await opened(page))[0].startsWith('https://dash.cloudflare.com'), 'the window opens the login page once when cloudflared did not');
  await page.waitForSelector('.pl-input', { timeout: 6000 });
  check((await opened(page)).length === 1, 'the login page is not opened a second time');
  check((await wizText(page)).includes('Step 2 of 2'), 'step 2: the address');
  check(await page.$eval('.pl-input', (e) => document.activeElement === e), 'the address field has focus');
  await page.fill('.pl-input', 'https://Good.example.com/path');
  m = await mark();
  await clickButton(page, 'Create the permanent link');
  act = await lastAction(m, 'create request');
  check(act.path === '/api/desktop/tunnel/named/create' && act.body.hostname === 'Good.example.com' && act.body.overwrite === false, 'Create sends the cleaned address', act);
  await page.waitForFunction(() => document.querySelector('.pl-wizard')?.textContent.includes('Done. The channel now has a permanent address: https://good.example.com'), null, { timeout: 6000 }).catch(async () => {
    throw new Error(`FAILED (timeout): done step — ${await wizText(page)}`);
  });
  check(true, 'done: the new address is shown');
  await page.waitForFunction(() => document.querySelector('.pl-mode')?.textContent.includes('Permanent link: https://good.example.com'), null, { timeout: 6000 });
  check((await hasButton(page, 'Back to the quick tunnel')) && (await hasButton(page, 'Delete the permanent link')), 'permanent mode: back to quick / delete');
  await page.screenshot({ path: join(shots, 'after-wizard-done-en-1100x680.png') });
  await clickButton(page, 'Close');
  check((await page.$('.pl-wizard')) === null, 'Close closes the wizard');
  m = await mark();
  await clickButton(page, 'Back to the quick tunnel');
  act = await lastAction(m, 'quick request');
  check(act.path === '/api/desktop/tunnel/named/quick', 'Back to the quick tunnel', act);
  await page.waitForFunction(() => document.querySelector('.pl-mode')?.textContent.includes('Quick link'), null, { timeout: 6000 });
  check(await hasButton(page, 'Back to the permanent link (good.example.com)'), 'the saved link can be taken up again');
  m = await mark();
  await clickButton(page, 'Back to the permanent link (good.example.com)');
  act = await lastAction(m, 'back to named');
  check(act.method === 'PUT' && act.path === '/api/desktop/config' && act.body.tunnelMode === 'named', 'it switches the mode to named', act);
  await page.waitForFunction(() => document.querySelector('.pl-mode')?.textContent.includes('Permanent link: https://good.example.com'), null, { timeout: 6000 });
  await clickButton(page, 'Delete the permanent link');
  check((await bodyText(page)).includes('Delete the permanent link? The tunnel is removed'), 'delete asks first');
  await clickButton(page, 'Cancel');
  check(!(await bodyText(page)).includes('Delete the permanent link? The tunnel'), 'Cancel keeps it');
  await clickButton(page, 'Delete the permanent link');
  m = await mark();
  await page.click('#view .btn.danger:has-text("Delete the permanent link") >> nth=0');
  act = await lastAction(m, 'delete request');
  check(act.path === '/api/desktop/tunnel/named/delete', 'confirmed delete', act);
  await page.waitForSelector('#view button:has-text("Set up a permanent link")', { timeout: 6000 });
  check(true, 'after delete the setup button is back');
  check(page.__errors.length === 0, 'no console errors in the wizard', page.__errors);
  await page.close();

  // The name is taken: replace it.
  page = await wizardPage();
  await clickButton(page, 'Set up a permanent link');
  await clickButton(page, 'Connect to Cloudflare');
  await page.waitForSelector('.pl-input', { timeout: 6000 });
  await page.fill('.pl-input', 'taken.example.com');
  await clickButton(page, 'Create the permanent link');
  await page.waitForSelector('button:has-text("Replace the record")', { timeout: 6000 });
  check((await wizText(page)).includes('The name taken.example.com already has another DNS record'), 'dns_taken asks whether to replace the record');
  check((await text(page, '.pl-notice')).includes('already taken'), 'with the engine\'s message above');
  await clickButton(page, 'Choose another name');
  await page.waitForSelector('.pl-input');
  check((await page.$eval('.pl-input', (e) => e.value)) === 'taken.example.com', 'Choose another name returns to the field with the name');
  await clickButton(page, 'Create the permanent link');
  await page.waitForSelector('button:has-text("Replace the record")', { timeout: 6000 });
  m = await mark();
  await clickButton(page, 'Replace the record');
  act = await lastAction(m, 'overwrite request');
  check(act.body.hostname === 'taken.example.com' && act.body.overwrite === true, 'Replace the record sends overwrite', act);
  await page.waitForFunction(() => document.querySelector('.pl-wizard')?.textContent.includes('permanent address: https://taken.example.com'), null, { timeout: 6000 });
  check(true, 'and then it is done');
  await page.close();

  // No domain in the account: the message with the help link; the field is back.
  page = await wizardPage();
  await clickButton(page, 'Set up a permanent link');
  await clickButton(page, 'Connect to Cloudflare');
  await page.waitForSelector('.pl-input', { timeout: 6000 });
  await page.fill('.pl-input', 'nodomain.example.com');
  await clickButton(page, 'Create the permanent link');
  await page.waitForSelector('.pl-notice.err', { timeout: 6000 });
  check((await page.$eval('.pl-notice.err a', (a) => a.getAttribute('href'))) === 'https://example.invalid/add-domain', 'no_domain: a help link to add a domain');
  check((await page.$eval('.pl-input', (e) => e.value)) === 'nodomain.example.com', 'the typed name stays in the field');
  await page.close();

  // Cancelling the login, and a failed login.
  page = await wizardPage({ namedLogin: 'wait' });
  await clickButton(page, 'Set up a permanent link');
  await clickButton(page, 'Connect to Cloudflare');
  await page.waitForSelector('.pl-wizard a[href^="https://dash.cloudflare.com"]');
  m = await mark();
  await page.click('.pl-wizard button:has-text("Cancel")');
  act = await lastAction(m, 'cancel request');
  check(act.path === '/api/desktop/tunnel/named/login' && act.body.cancel === true, 'Cancel cancels the login', act);
  await page.waitForSelector('button:has-text("Try again")', { timeout: 6000 });
  check((await text(page, '.pl-notice')) === 'The login was cancelled.', 'a cancelled login says so and offers to try again');
  await page.close();

  page = await wizardPage({ namedLogin: 'fail', loginMs: 300 });
  await clickButton(page, 'Set up a permanent link');
  await clickButton(page, 'Connect to Cloudflare');
  await page.waitForSelector('button:has-text("Try again")', { timeout: 6000 });
  check((await text(page, '.pl-notice')).includes('did not answer'), 'a failed login shows the engine\'s reason');
  await page.close();

  // Leaving the view in the middle: polling stops, the wizard takes up where it is.
  page = await wizardPage({ namedLogin: 'wait', browserOpened: false });
  await clickButton(page, 'Set up a permanent link');
  await clickButton(page, 'Connect to Cloudflare');
  await page.waitForSelector('.pl-wizard a[href^="https://dash.cloudflare.com"]');
  check((await opened(page)).length === 1, 'login page opened once');
  await page.evaluate(() => (location.hash = '#/history'));
  await page.waitForSelector('.hist-row');
  await sleep(300);
  m = await mark();
  await sleep(2600);
  check((await since(m)).every((r) => !r.path.includes('/tunnel/named/')), 'away from Share the wizard does not poll the engine');
  await page.evaluate(() => (location.hash = '#/share'));
  await page.waitForSelector('.pl-wizard a[href^="https://dash.cloudflare.com"]', { timeout: 6000 });
  check((await wizText(page)).includes('Step 1 of 2'), 'back on Share the login goes on');
  await sleep(1500);
  check((await opened(page)).length === 1, 'the login page is not opened again after coming back');
  await page.click('.pl-wizard button:has-text("Cancel")');
  await page.waitForSelector('button:has-text("Try again")', { timeout: 6000 });
  await page.close();

  // A creation that finishes while away is found done on return.
  page = await wizardPage();
  await scenario({ namedLogin: 'done', loginMs: 100 });
  await clickButton(page, 'Set up a permanent link');
  await clickButton(page, 'Connect to Cloudflare');
  await page.waitForSelector('.pl-input', { timeout: 6000 });
  await page.fill('.pl-input', 'slow.example.com');
  await clickButton(page, 'Create the permanent link');
  await page.waitForSelector('.pl-input[disabled]');
  check((await text(page, '.pl-notice')).startsWith('Creating'), 'while creating the field is locked and the progress shows');
  await page.evaluate(() => (location.hash = '#/history'));
  await page.waitForSelector('.hist-row');
  await sleep(3200);
  await page.evaluate(() => (location.hash = '#/share'));
  await page.waitForFunction(() => document.querySelector('.pl-wizard')?.textContent.includes('permanent address: https://slow.example.com'), null, { timeout: 8000 }).catch(async () => {
    throw new Error(`FAILED (timeout): return to a finished creation — ${await page.$eval('#view', (e) => e.innerText)}`);
  });
  check(true, 'a creation finished while away shows its outcome on return');
  await page.close();

  // The same, but returning while it still runs.
  page = await wizardPage({ loginMs: 100 });
  await clickButton(page, 'Set up a permanent link');
  await clickButton(page, 'Connect to Cloudflare');
  await page.waitForSelector('.pl-input', { timeout: 6000 });
  await page.fill('.pl-input', 'slow.example.com');
  await clickButton(page, 'Create the permanent link');
  await page.waitForSelector('.pl-input[disabled]');
  await page.evaluate(() => (location.hash = '#/history'));
  await page.waitForSelector('.hist-row');
  await sleep(300);
  await page.evaluate(() => (location.hash = '#/share'));
  await page.waitForSelector('.pl-input[disabled]', { timeout: 3000 });
  check(true, 'back while the creation still runs, the wizard shows it working');
  await page.waitForFunction(() => document.querySelector('.pl-wizard')?.textContent.includes('permanent address: https://slow.example.com'), null, { timeout: 8000 });
  check(true, 'and then its outcome');
  await setLanguage(page, 'pl');
  await page.waitForFunction(() => document.querySelector('.pl-wizard button')?.textContent === 'Zamknij');
  check((await noRawKeys(page)) && (await bodyText(page)).includes('Gotowe. Kanał ma teraz stały adres'), 'Polish wizard');
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
}
