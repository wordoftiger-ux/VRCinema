// Browser check of the Settings view (every section saves the right fields, validation of the
// ticker messages, the program language switching the interface at once, a remote session and a
// server without a window seeing the window-only parts disabled or replaced) against the fake
// engine, in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/settings.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots (every section in en and de) go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8197);
if (!shots) throw new Error('usage: settings.mjs <shots dir> [port]');
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
/** Waits for a request that is not a GET and resolves to the whole list of them since `m`. */
const actions = async (m, what) => {
  await lastAction(m, what);
  return actionsSince(m);
};
const noActions = async (m, what, ms = 400) => {
  await sleep(ms);
  check((await actionsSince(m)).length === 0, what, await actionsSince(m));
};

async function openApp(hash) {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  // The checks send refused values on purpose: the browser logs their 400 answers.
  page.on('console', (m) => m.type() === 'error' && !/status of 400/.test(m.text()) && errors.push(m.text()));
  page.on('response', (r) => r.status() >= 400 && !r.url().includes('/api/') && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${base}/admin${hash}`);
  await page.waitForSelector('.player .play');
  page.__errors = errors;
  return page;
}

const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const bodyText = (page) => page.$eval('#view', (e) => e.innerText);
const setLanguage = (page, code) => page.evaluate(async (c) => (await import('/static/app/store.js')).setLanguage(c), code);
const RAW = /\b(settings|ticker|units|net|share|permalink|errors|ui)\.[a-z_]+/;
const noRawKeys = (page) => page.$eval('#view', (e, source) => !new RegExp(source).test(e.innerText), RAW.source);
const noOverflow = (page) => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);
const goto = async (page, section) => {
  await page.evaluate((s) => (location.hash = `#/settings/${s}`), section);
  await page.waitForSelector(`#view[data-section="${section}"]`);
};
const sw = (label) => `button[role=switch][aria-label="${label}"]`;
const isOn = (page, label) => page.$eval(sw(label), (b) => b.getAttribute('aria-checked') === 'true');
const note = (page) => page.$eval('.set-note', (e) => e.textContent.trim()).catch(() => null);
const clickText = (page, label) => page.click(`#view button:has-text("${label}")`);
const shot = (page, name) => page.screenshot({ path: join(shots, name) });
const seg = (label, value) => `[role=radiogroup][aria-label="${label}"] button[data-value="${value}"]`;
const putSettings = (a) => a.filter((r) => r.method === 'PUT' && r.path === '/api/admin/channels/main/settings');
const putConfigs = (a) => a.filter((r) => r.method === 'PUT' && r.path === '/api/desktop/config');

const SECTIONS = ['general', 'internet', 'channel', 'tools', 'remote', 'advanced', 'log', 'about'];

try {
  await reset();
  let page = await openApp('#/settings/general');
  await page.waitForSelector('#view[data-section="general"] .row2');

  // ---------- Navigation ----------
  const links = await page.$$eval('.snav a', (as) => as.map((a) => [a.dataset.section, a.textContent.trim()]));
  check(JSON.stringify(links.map((l) => l[0])) === JSON.stringify(SECTIONS), 'the list has the eight sections of the spec', links);
  check(links.map((l) => l[1]).join('|') === 'General|Internet|Channel|Tools|Remote panel|Advanced|Log|About', 'their names', links);
  check((await text(page, '.sform h1')) === 'General', 'the heading names the section');

  // ---------- General ----------
  await page.waitForSelector('.sel[aria-label="Language"]:not([disabled])');
  const langOptions = await page.$$eval('.sel[aria-label="Language"] option', (o) => o.map((x) => [x.value, x.textContent]));
  check(langOptions.map((o) => o[0]).join() === 'en,pl,es,de,fr,ja', 'the languages: en, pl, es, de, fr, ja', langOptions);
  check(langOptions.at(-1)[1] === '日本語 (beta)' && langOptions[1][1] === 'Polski', 'Japanese is labelled beta, names are their own', langOptions);
  check(await page.$eval('.sel[aria-label="Language"]', (s) => s.value === 'en'), 'the current language is selected');
  await shot(page, 'after-settings-general-en.png');

  let m = await mark();
  await page.selectOption('.sel[aria-label="Language"]', 'pl');
  let a = await actions(m, 'language request');
  check(putConfigs(a).length === 1 && JSON.stringify(putConfigs(a)[0].body) === '{"language":"pl"}', 'the language choice saves {language}', a);
  await page.waitForFunction(() => document.querySelector('.sform h1')?.textContent === 'Ogólne');
  check((await text(page, '.snav a[data-section="internet"]')) === 'Internet' && (await text(page, '.snav a[data-section="remote"]')) === 'Panel zdalny', 'the interface switches to Polish at once');
  check((await page.$eval('.rail-settings .rail-label', (e) => e.textContent)) === 'Ustawienia', 'also the rail');
  check(await noRawKeys(page), 'no raw keys in Polish General');
  await page.selectOption('.sel[aria-label="Język"]', 'en');
  await page.waitForFunction(() => document.querySelector('.sform h1')?.textContent === 'General');

  m = await mark();
  await page.click(sw('Start VRCinema with Windows'));
  a = await actions(m, 'autostart request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"autostart":true}', 'autostart saves {autostart: true}', a);
  await page.waitForFunction(() => document.querySelector('.set-note.ok')?.textContent.includes('start with Windows'));
  check(await isOn(page, 'Start VRCinema with Windows'), 'the switch shows the saved state');
  m = await mark();
  await page.click(sw('Allow opening the page from other devices on the home network'));
  a = await actions(m, 'lan request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"lanAccess":true}', 'home network access saves {lanAccess: true}', a);
  await page.waitForFunction(() => document.querySelector('.set-note.ok')?.textContent.includes('Other devices'));

  const portInput = '.num[aria-label="Server port"]';
  check((await page.$eval(portInput, (i) => i.value)) === String(port), 'the port field shows the port in use');
  m = await mark();
  await page.fill(portInput, '80');
  // The browser's own range check would stop a bad port first: ours is checked with it off.
  await page.evaluate(() => {
    const form = document.querySelector('.num[aria-label="Server port"]').form;
    form.noValidate = true;
    form.requestSubmit();
  });
  await page.waitForFunction(() => document.querySelector('.set-note.err'));
  check((await note(page)) === 'The port must be a number from 1024 to 65535.', 'a port out of range is refused with the engine\'s own text', await note(page));
  await noActions(m, 'and nothing was sent');
  await page.fill(portInput, String(port));
  await clickText(page, 'Save port');
  await page.waitForFunction(() => document.querySelector('.set-note')?.textContent.includes('already the current'));
  await noActions(m, 'the current port is not sent again');
  await page.fill(portInput, '9000');
  await clickText(page, 'Save port');
  a = await actions(m, 'port request');
  check(JSON.stringify(putConfigs(a).at(-1).body) === '{"port":9000}', 'a new port saves {port}', a);
  await page.waitForFunction(() => document.querySelector('.set-note.ok')?.textContent.includes('restarted on port 9000'));
  check(true, 'and says the server restarted on it');
  check((await page.$eval('#view', (e) => e.innerText)).includes('interrupted for a moment'), 'the restart warning is shown');
  check(await noRawKeys(page), 'no raw keys in General');
  check(await noOverflow(page), 'no horizontal overflow on General');

  // A dropped connection is "restarting" only for a new port or network access
  const verdict = await page.evaluate(async () => {
    const { saveConfig } = await import('/static/app/views/settings/common.js');
    const real = window.fetch;
    window.fetch = (url, init) => (String(url).includes('/api/desktop/config') && init?.method === 'PUT' ? Promise.reject(new TypeError('down')) : real(url, init));
    const t0 = performance.now();
    const other = await saveConfig({ autostart: true });
    const ms = performance.now() - t0;
    window.fetch = real;
    return { other, ms };
  });
  check(verdict.other.ok === false && !verdict.other.restarting && typeof verdict.other.error === 'string' && verdict.ms < 1400, 'a dropped connection on an ordinary setting is an error, not "restarting"', verdict);
  const portVerdict = await page.evaluate(async () => {
    const { saveConfig } = await import('/static/app/views/settings/common.js');
    const real = window.fetch;
    let first = true;
    window.fetch = (url, init) => {
      if (String(url).includes('/api/desktop/config') && init?.method === 'PUT' && first) {
        first = false;
        return Promise.reject(new TypeError('down'));
      }
      return real(url, init);
    };
    const r = await saveConfig({ lanAccess: false });
    window.fetch = real;
    return r;
  });
  check(portVerdict.restarting === true, 'a dropped connection on a network access change is "restarting"', portVerdict);

  // Notes are text in the language they were made in: a language switch clears them
  await page.click(sw('Start VRCinema with Windows'));
  await page.waitForSelector('.set-note');
  await page.selectOption('.sel[aria-label="Language"]', 'pl');
  await page.waitForFunction(() => document.querySelector('.sform h1')?.textContent === 'Ogólne');
  check((await page.$('.set-note')) === null, 'a note is cleared by a language switch (no stale text in the old language)');
  await page.selectOption('.sel[aria-label="Język"]', 'en');
  await page.waitForFunction(() => document.querySelector('.sform h1')?.textContent === 'General');

  // ---------- Internet ----------
  await goto(page, 'internet');
  await page.waitForSelector('.named-link');
  check((await page.$eval('.kv[data-tunnel-state]', (e) => e.dataset.tunnelState)) === 'online' && (await text(page, '.kv .name')) === 'Online.', 'the tunnel state is shown');
  check((await bodyText(page)).includes('Channel address: https://proba-kanalu.trycloudflare.com'), 'with the channel address');
  check((await bodyText(page)).includes('Quick link: the address is random'), 'the quick link block (named-wizard component) is reused');
  check(!(await bodyText(page)).includes('Public address of the channel'), 'the manual address is not offered with the tunnel on');
  await shot(page, 'after-settings-internet-en.png');
  m = await mark();
  await page.click(sw('Pause when the internet is gone'));
  a = await actions(m, 'pause request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"pauseOnTunnelLoss":false}', 'the pause on loss saves its field (default on, now off)', a);
  m = await mark();
  await page.click(sw('Pause when nobody is watching'));
  a = await actions(m, 'pause viewers request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"pauseWhenNoViewers":true}', 'the pause without viewers saves its field', a);
  m = await mark();
  await clickText(page, 'Reconnect');
  a = await actions(m, 'reconnect request');
  check(a[0].path === '/api/desktop/tunnel/restart', 'Reconnect restarts the tunnel', a);
  await clickText(page, 'Set up a permanent link');
  await page.waitForSelector('.pl-wizard');
  check(true, 'the permanent link wizard opens in this section');
  await clickText(page, 'Cancel').catch(() => {});

  m = await mark();
  await page.click(sw('Share the channel on the internet (built-in Cloudflare tunnel)'));
  a = await actions(m, 'tunnel off request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"tunnelEnabled":false}', 'the tunnel switch saves {tunnelEnabled}', a);
  await page.waitForSelector('input[aria-label="Public address of the channel"]');
  check(!(await page.$('.named-link')) && !(await page.$(sw('Pause when the internet is gone'))), 'with the tunnel off the link type and pauses go away');
  check(await page.$(sw('Trust Cloudflare headers')) !== null, 'and the manual address and the Cloudflare trust appear');
  m = await mark();
  await page.fill('input[aria-label="Public address of the channel"]', 'ftp://x.example');
  await page.evaluate(() => document.querySelector('input[aria-label="Public address of the channel"]').form.requestSubmit());
  await page.waitForFunction(() => document.querySelector('.set-note.err'));
  check((await note(page)) === 'The public address must start with http:// or https://.', 'a bad public address shows the engine\'s message', await note(page));
  await page.fill('input[aria-label="Public address of the channel"]', '  https://tv.example.com  ');
  await page.evaluate(() => document.querySelector('input[aria-label="Public address of the channel"]').form.requestSubmit());
  a = await actions(m, 'public url request');
  check(putConfigs(a).at(-1).body.publicUrl === 'https://tv.example.com', 'the public address saves {publicUrl}', a);
  m = await mark();
  await page.click(sw('Trust Cloudflare headers'));
  a = await actions(m, 'trust request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"trustCloudflare":true}', 'the Cloudflare trust saves {trustCloudflare}', a);
  check(await noRawKeys(page), 'no raw keys in Internet');
  await scenario({ tunnel: 'online' });

  // ---------- Channel ----------
  await goto(page, 'channel');
  await page.waitForSelector('.domains[data-list="allowed"]');
  await shot(page, 'after-settings-channel-en.png');
  const chips = (list) => page.$$eval(`.domains[data-list="${list}"] .chip span`, (s) => s.map((x) => x.textContent));
  check((await chips('allowed')).join() === 'youtube.com,youtu.be', 'the allowed list shows its chips');
  check((await page.$eval('input[name=linkMode]:checked', (i) => i.value)) === 'all', 'the link mode is "any site"');
  m = await mark();
  await page.click('input[name=linkMode][value=allowlist]');
  a = await actions(m, 'link mode request');
  check(JSON.stringify(putSettings(a)[0].body) === '{"linkMode":"allowlist"}', 'the mode radio saves {linkMode}', a);
  await page.waitForFunction(() => document.querySelector('.domains[data-list="allowed"]:not(.idle)'));
  check(await page.$eval('.domains[data-list="blocked"]', (e) => e.classList.contains('idle')), 'the list not in use is dimmed');

  m = await mark();
  await page.fill('input[aria-label="Add a site to the list: Allowed sites"]', 'Twitch.tv https://www.vimeo.com/watch/1');
  await clickText(page, 'Add');
  a = await actions(m, 'add domains request');
  check(JSON.stringify(putSettings(a)[0].body.allowedDomains) === '["youtube.com","youtu.be","twitch.tv","vimeo.com"]', 'sites are added normalized (no scheme, www or path)', a);
  await page.waitForFunction(() => document.querySelectorAll('.domains[data-list="allowed"] .chip').length === 4);
  m = await mark();
  await page.fill('input[aria-label="Add a site to the list: Allowed sites"]', 'nonsense');
  await clickText(page, 'Add');
  await page.waitForFunction(() => document.querySelector('.domains[data-list="allowed"] .set-note.err'));
  await noActions(m, 'a text that is not a site is refused on the page');
  m = await mark();
  await page.click('.domains[data-list="allowed"] .chip:has-text("twitch.tv") button');
  a = await actions(m, 'remove domain request');
  check(JSON.stringify(putSettings(a)[0].body.allowedDomains) === '["youtube.com","youtu.be","vimeo.com"]', 'the × removes a chip and saves the list', a);
  m = await mark();
  await page.click('.domains[data-list="allowed"] [data-action=popular]');
  a = await actions(m, 'popular request');
  const popular = putSettings(a)[0].body.allowedDomains;
  check(popular.includes('twitch.tv') && popular.includes('kick.com') && popular.includes('cda.pl') && new Set(popular).size === popular.length && popular[0] === 'youtube.com', '"Add popular sites" adds the engine\'s set without duplicates', popular);
  m = await mark();
  await page.fill('input[aria-label="Add a site to the list: Blocked sites"]', 'tiktok.com');
  await page.press('input[aria-label="Add a site to the list: Blocked sites"]', 'Enter');
  a = await actions(m, 'blocked list request');
  check(JSON.stringify(putSettings(a)[0].body) === '{"blockedDomains":["tiktok.com"]}', 'the blocked list saves {blockedDomains}', a);

  // An empty list in the mode that uses it: a warning
  const warning = (kind) => page.$(`[data-warning=${kind}]`);
  await page.click('input[name=linkMode][value=blocklist]');
  await page.waitForFunction(() => document.querySelector('input[name=linkMode][value=blocklist]')?.checked);
  check((await warning('blocklist_empty')) === null, 'a blocked list with a site raises no warning');
  await page.click('.domains[data-list="blocked"] .chip button');
  await page.waitForSelector('[data-warning=blocklist_empty]');
  check((await text(page, '[data-warning=blocklist_empty]')) === 'The list of blocked sites is empty, so nothing is blocked: links from any site are accepted.', 'an empty blocked list in the blocklist mode warns that nothing is blocked');
  await page.click('input[name=linkMode][value=allowlist]');
  await page.waitForFunction(() => !document.querySelector('[data-warning=blocklist_empty]'));
  check((await warning('allowlist_empty')) === null, 'an allowed list with sites raises no warning');
  await page.evaluate(async () => (await import('/static/app/views/settings/common.js')).saveChannel({ allowedDomains: [] }));
  await page.waitForSelector('[data-warning=allowlist_empty]');
  check((await text(page, '[data-warning=allowlist_empty]')) === 'The list of allowed sites is empty, so every link from viewers will be refused.', 'an empty allowed list in the allowlist mode warns that every link is refused');
  await page.click('input[name=linkMode][value=all]');
  await page.waitForFunction(() => !document.querySelector('[data-warning]'));
  check(true, 'in the "any site" mode empty lists raise no warning');
  await page.evaluate(async () => (await import('/static/app/views/settings/common.js')).saveChannel({ allowedDomains: ['youtube.com'], linkMode: 'all' }));

  // Limits
  m = await mark();
  await page.fill('input[name=maxDurationMin]', '60');
  await page.fill('input[name=maxActivePerRequester]', '2');
  await page.fill('input[name=requestLimitCount]', '4');
  await page.fill('input[name=requestLimitWindowMin]', '15');
  await page.click('.limits button[type=submit]');
  a = await actions(m, 'limits request');
  check(JSON.stringify(putSettings(a)[0].body) === '{"maxDurationS":3600,"maxActivePerRequester":2,"requestLimitCount":4,"requestLimitWindowS":900}', 'limits save seconds from the minutes typed', a);
  m = await mark();
  await page.fill('input[name=maxDurationMin]', '0');
  await page.evaluate(() => {
    const form = document.querySelector('.limits');
    form.noValidate = true; // the browser's own minimum would stop it first
    form.requestSubmit();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('.set-note.err')].some((n) => n.textContent.includes('whole number')));
  await noActions(m, 'a limit below 1 is refused on the page');
  await page.fill('input[name=maxDurationMin]', '60');

  // Moderation, stream
  for (const [label, key, def] of [
    ['Approve viewers\' requests before they join the queue', 'moderation', true],
    ['Always convert the picture', 'forceTranscode', true],
    ['Mark transitions between videos in the stream', 'discontinuityTags', false],
  ]) {
    m = await mark();
    await page.click(sw(label));
    a = await actions(m, `${key} request`);
    check(JSON.stringify(putSettings(a)[0].body) === `{"${key}":${def}}`, `${key} saves its field`, a);
  }
  m = await mark();
  await page.selectOption('.sel[aria-label="Language of the slates"]', 'ja');
  a = await actions(m, 'slate language request');
  check(JSON.stringify(putSettings(a)[0].body) === '{"slateLanguage":"ja"}', 'the slate language saves {slateLanguage}', a);
  await page.waitForFunction(() => [...document.querySelectorAll('.set-note.ok')].some((n) => n.textContent === 'Language of the slates saved. The video on air starts again from where it is.'));
  check(true, 'with the ticker on, the slate language says the video restarts from where it is');

  // Ticker
  check((await page.$eval('[data-ticker]', (e) => e.dataset.ticker)) === 'on', 'the ticker block shows it is on');
  const tick = async (selector, body, what) => {
    m = await mark();
    await page.click(selector);
    a = await actions(m, what);
    check(JSON.stringify(putSettings(a)[0].body) === JSON.stringify(body), what, a);
  };
  check((await text(page, '[data-restart-note]')).includes('starts the video on air again from where it is'), 'a note by the ticker controls says structural changes restart the video');
  await tick(seg('Mode', 'continuous'), { tickerMode: 'continuous' }, 'ticker mode saves {tickerMode}');
  const restartSaid = () => page.waitForFunction(() => [...document.querySelectorAll('.set-note.ok')].some((n) => n.textContent === 'Info ticker settings saved. The video on air starts again from where it is.'), null, { timeout: 3000 });
  await restartSaid();
  check(true, 'a structural change says the video restarts from where it is');
  check(await page.$eval(seg('Interval', '300'), (b) => b.disabled), 'the interval is off in the continuous mode');
  await tick(seg('Mode', 'interval'), { tickerMode: 'interval' }, 'ticker mode back to interval');
  await tick(seg('Interval', '300'), { tickerIntervalS: 300 }, 'the interval saves {tickerIntervalS}');
  check((await text(page, `${seg('Interval', '300')}`)) === '5 min' && (await text(page, seg('Interval', '60'))) === '1 min', 'intervals are 1, 2, 5, 10 minutes');
  const hint = () => page.$eval('[data-ticker] .row2:has([aria-label="Scrolling speed"]) small', (e) => e.textContent);
  const secondsOf = async () => Number((await hint()).match(/about (\d+) s/)?.[1]);
  const normalSeconds = await secondsOf();
  await tick(seg('Scrolling speed', 'fast'), { tickerSpeed: 'fast' }, 'the speed saves {tickerSpeed}');
  await page.waitForFunction((n) => /about (\d+) s/.test(document.querySelector('[data-ticker] .row2:has([aria-label="Scrolling speed"]) small').textContent) && Number(document.querySelector('[data-ticker] .row2:has([aria-label="Scrolling speed"]) small').textContent.match(/about (\d+) s/)[1]) < n, normalSeconds);
  check((await secondsOf()) < normalSeconds && normalSeconds > 10, 'the hint gives the resulting visibility in seconds (faster is shorter)', [normalSeconds, await secondsOf()]);
  await tick(seg('QR code of the request page', 'always'), { tickerQr: 'always' }, 'the QR choice saves {tickerQr}');
  await restartSaid();
  await tick(sw('Show the next video'), { tickerShowNext: false }, 'show next saves {tickerShowNext}');
  await page.waitForFunction(() => [...document.querySelectorAll('.set-note.ok')].some((n) => n.textContent === 'Info ticker settings saved.'));
  check(true, 'a text-only change (next video) does not claim a restart');
  await tick(sw('Show the address of the request page'), { tickerShowAddress: true }, 'show address saves {tickerShowAddress}');
  await tick(sw('Video progress line'), { tickerProgress: false }, 'progress saves {tickerProgress}');
  await tick(sw('Show the info ticker'), { tickerEnabled: false }, 'the switch saves {tickerEnabled}');
  check((await page.$eval('[data-ticker]', (e) => e.dataset.ticker)) === 'off' && (await page.$eval(seg('Mode', 'continuous'), (b) => b.disabled)), 'with the ticker off its choices are disabled');
  await tick(sw('Show the info ticker'), { tickerEnabled: true }, 'and on again');

  // Messages
  const messages = () => page.$$eval('[data-messages] .msg code', (c) => c.map((x) => x.textContent));
  check((await messages()).join() === 'Welcome to the Friday show!', 'the message list shows the saved messages');
  m = await mark();
  await page.fill('input[aria-label="New message"]', 'Please do not spam the queue');
  await page.click('[data-messages] form:last-of-type button[type=submit]');
  a = await actions(m, 'add message request');
  check(JSON.stringify(putSettings(a)[0].body) === '{"tickerMessages":["Welcome to the Friday show!","Please do not spam the queue"]}', 'a message is added and the list saved', a);
  m = await mark();
  await page.fill('input[aria-label="New message"]', 'x'.repeat(201));
  check((await text(page, '[data-messages] .count.over')) === '201/200', 'the counter shows 201/200 in red');
  await page.click('[data-messages] form:last-of-type button[type=submit]');
  await page.waitForFunction(() => document.querySelector('[data-messages] .set-note.err'));
  check((await text(page, '[data-messages] .set-note.err')) === 'A message can have at most 200 characters.', 'a message over 200 characters is refused', await text(page, '[data-messages] .set-note.err'));
  await noActions(m, 'and not sent');
  m = await mark();
  await page.fill('input[aria-label="New message"]', 'y'.repeat(200));
  await page.click('[data-messages] form:last-of-type button[type=submit]');
  a = await actions(m, '200 characters request');
  check(putSettings(a)[0].body.tickerMessages.at(-1).length === 200, 'exactly 200 characters is accepted', a.length);
  // up to 10
  for (let i = 4; i <= 10; i += 1) {
    await page.fill('input[aria-label="New message"]', `message ${i}`);
    await page.click('[data-messages] form:last-of-type button[type=submit]');
    await waitFor(async () => (await messages()).length === i, `message ${i} added`);
  }
  check((await messages()).length === 10, 'ten messages are allowed');
  m = await mark();
  await page.fill('input[aria-label="New message"]', 'one too many');
  await page.click('[data-messages] form:last-of-type button[type=submit]');
  await page.waitForFunction(() => [...document.querySelectorAll('[data-messages] .set-note.err')].some((n) => n.textContent === 'There can be at most 10 messages.'));
  await noActions(m, 'an eleventh message is refused and not sent');
  // edit and remove
  m = await mark();
  await page.click('[data-messages] .msg:first-child button:has-text("Edit")');
  await page.fill('[data-messages] .msg:first-child input', 'Welcome back!');
  await page.click('[data-messages] .msg:first-child button[type=submit]');
  a = await actions(m, 'edit request');
  check(putSettings(a)[0].body.tickerMessages[0] === 'Welcome back!' && putSettings(a)[0].body.tickerMessages.length === 10, 'editing a message saves the list', a);
  m = await mark();
  await page.click('[data-messages] .msg:first-child button:has-text("Remove")');
  a = await actions(m, 'remove message request');
  check(putSettings(a)[0].body.tickerMessages.length === 9 && !putSettings(a)[0].body.tickerMessages.includes('Welcome back!'), 'removing a message saves the list', a);
  // an engine-side refusal (the page's own check is bypassed) is shown too
  m = await mark();
  const engineAnswer = await page.evaluate(async () => (await import('/static/app/api.js')).api('PUT', '/api/admin/channels/main/settings', { tickerMessages: ['z'.repeat(201)] }).catch((e) => e.message));
  check(engineAnswer.includes('at most 200 characters'), 'the engine refuses it as well', engineAnswer);
  await page.evaluate(() => (location.hash = '#/settings/channel'));

  // Encoder info and the "cannot keep up" banner
  check((await page.$eval('.encoder-info', (e) => e.dataset.encoder)) === 'h264_nvenc' && (await text(page, '.encoder-info')).includes('NVIDIA'), 'the encoder in use is shown');
  check((await page.$('[data-banner=ticker_slow]')) === null, 'no slow banner while the computer keeps up');
  await scenario({ tickerSlow: true });
  await page.waitForSelector('[data-banner=ticker_slow]', { timeout: 6000 });
  check((await text(page, '[data-banner=ticker_slow]')).startsWith('The computer cannot keep up with the info ticker'), 'the slow banner appears');
  await scenario({ tickerSlow: false, encoder: 'libx264' });
  await page.waitForFunction(() => !document.querySelector('[data-banner=ticker_slow]') && document.querySelector('.encoder-info')?.dataset.encoder === 'libx264', null, { timeout: 6000 });
  check((await text(page, '.encoder-info')).includes('processor'), 'the banner goes away and the processor warning shows');
  await scenario({ encoder: 'h264_nvenc' });

  // Nicks
  await page.waitForSelector('[data-nicks] .nick-row');
  const rows = await page.$$eval('[data-nicks] .nick-list:first-of-type .nick-row', (r) => r.map((e) => [e.dataset.skeleton, e.querySelector('b').textContent, e.dataset.blocked]));
  check(rows.length === 3 && rows[0][1] === 'Kasia' && rows[2][2] === 'true', 'the nicks are listed with their blocked state', rows);
  check(!(await page.$eval('[data-nicks]', (e) => e.innerText)).match(/\b\d+\.\d+\.\d+\.\d+\b/), 'no address is shown');
  m = await mark();
  await page.click('.nick-row[data-skeleton="kasia"] [data-action=release]');
  a = await actions(m, 'release request');
  check(a[0].method === 'POST' && a[0].path === '/api/admin/nicks/kasia/release', 'Release posts the nick\'s skeleton', a[0]);
  await page.waitForFunction(() => !document.querySelector('.nick-row[data-skeleton="kasia"]'));
  m = await mark();
  const weird = encodeURIComponent('a/b?c#d%e');
  await page.click(`.nick-row[data-skeleton="${'a/b?c#d%e'}"] [data-action=release]`);
  a = await actions(m, 'weird release request');
  check(a[0].path === `/api/admin/nicks/${weird}/release`, 'a skeleton with / ? # % is encoded in the URL', a[0].path);
  check(!(await page.$('.nick-row[data-skeleton="troll"] [data-action=block]')), 'a blocked nick has no block button');
  check((await text(page, '[data-blocked-list] .nick-row b')) === 'Troll', 'the blocked devices are listed');
  m = await mark();
  await page.click('[data-blocked-list] [data-action=unblock]');
  a = await actions(m, 'unblock request');
  check(a[0].path === '/api/admin/devices/unblock' && JSON.stringify(a[0].body) === '{"id":1}', 'Unblock posts the block id', a[0]);
  await page.waitForFunction(() => !document.querySelector('[data-blocked-list]'));
  await page.waitForFunction(() => document.querySelector('.nick-row[data-skeleton="troll"]')?.dataset.blocked === 'false');
  m = await mark();
  await page.click('.nick-row[data-skeleton="troll"] [data-action=block]');
  await page.waitForSelector('.dialog');
  await noActions(m, 'Block device asks first');
  await page.click('.dialog button:has-text("Block device")');
  a = await actions(m, 'block request');
  check(a[0].path === '/api/admin/devices/block' && JSON.stringify(a[0].body) === '{"skeleton":"troll"}', 'confirming posts the skeleton', a[0]);
  await page.waitForSelector('[data-blocked-list]');
  check(await noRawKeys(page), 'no raw keys in Channel');
  check(await noOverflow(page), 'no horizontal overflow on Channel');
  check(page.__errors.length === 0, 'no console errors so far', page.__errors);

  // ---------- Tools ----------
  await goto(page, 'tools');
  await page.waitForSelector('.tool[data-tool=ffmpeg] .state');
  check((await text(page, '.tool[data-tool=yt-dlp] .state')) === 'version 2025.10.1', 'the tools list shows versions');
  await shot(page, 'after-settings-tools-en.png');
  m = await mark();
  await page.click('.tool[data-tool=yt-dlp] button');
  a = await actions(m, 'update request');
  check(a[0].path === '/api/desktop/tools/update' && JSON.stringify(a[0].body) === '{"tool":"yt-dlp"}', 'Update posts the tool', a[0]);
  await page.waitForFunction(() => document.querySelector('.tool-row[data-kind=active] .state')?.textContent.startsWith('Downloading'));
  check((await text(page, '.tool-row[data-kind=active] .state')).startsWith('Downloading 19.1 MB of 38.1 MB'), 'a progress row shows the download', await text(page, '.tool-row[data-kind=active] .state'));
  check((await page.$eval('.tool-row[data-kind=active] .bar', (b) => b.getAttribute('aria-valuenow'))) === '50', 'with a progress bar');
  await page.waitForFunction(() => document.querySelector('[data-job=finished] .set-note.ok'), null, { timeout: 8000 });
  check((await note(page)) === 'The tool is updated.', 'the end is announced');
  m = await mark();
  await clickText(page, 'Repair tools');
  a = await actions(m, 'repair request');
  check(a[0].path === '/api/desktop/tools/repair', 'Repair posts repair', a[0]);
  await waitFor(async () => (await page.$$('.tool-row')).length === 4, 'four rows for a repair');
  await page.waitForFunction(() => document.querySelector('[data-job=finished] .set-note.ok'), null, { timeout: 12000 });
  m = await mark();
  await page.selectOption('.sel[aria-label="Picture conversion"]', 'libx264');
  a = await actions(m, 'encoder request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"videoEncoder":"libx264"}', 'the converter saves {videoEncoder}', a);
  check((await text(page, '[data-encoder-state]')) === 'The card has not been tested yet.', 'no test yet');
  m = await mark();
  await clickText(page, 'Test the card');
  a = await actions(m, 'detect request');
  check(a[0].path === '/api/desktop/encoder/detect', 'the test posts encoder/detect', a[0]);
  await page.waitForFunction(() => document.querySelector('[data-encoder-state]')?.textContent.includes('NVIDIA'));
  check((await text(page, '[data-encoder-state]')) === 'Result of the last test: conversion on an NVIDIA card.', 'the result is shown', await text(page, '[data-encoder-state]'));
  await scenario({ tools: [{ name: 'yt-dlp', installed: true, version: '1', installedAt: 0 }, { name: 'ffmpeg', installed: true, version: '1', installedAt: 0 }, { name: 'deno', installed: false, version: null, installedAt: 0 }, { name: 'cloudflared', installed: true, version: '1', installedAt: 0 }] });
  await goto(page, 'log');
  await goto(page, 'tools');
  await page.waitForFunction(() => document.querySelector('.tool[data-tool=deno] button')?.textContent.trim() === 'Download');
  m = await mark();
  await page.click('.tool[data-tool=deno] button');
  a = await actions(m, 'download request');
  check(a[0].path === '/api/desktop/tools/install' && a[0].body === null, 'Download of a tool that is not installed uses the install route', a[0]);
  await page.waitForFunction(() => document.querySelector('[data-job=finished] .set-note.ok'), null, { timeout: 12000 });
  await scenario({ tools: [{ name: 'yt-dlp', installed: true, version: '2025.10.1', installedAt: 0 }, { name: 'ffmpeg', installed: true, version: '7.1', installedAt: 0 }, { name: 'deno', installed: true, version: '2.0.0', installedAt: 0 }, { name: 'cloudflared', installed: true, version: '2025.9.0', installedAt: 0 }] });
  await scenario({ toolsMissing: ['cloudflared'] });
  await page.waitForSelector('[data-banner=tools_missing]', { timeout: 6000 });
  check((await text(page, '[data-banner=tools_missing]')).includes('cloudflared'), 'missing tools show a banner with the names');
  await scenario({ toolsMissing: [] });
  check(await noRawKeys(page), 'no raw keys in Tools');

  // ---------- Remote panel ----------
  await goto(page, 'remote');
  await page.waitForSelector(sw('Allow logging in to the panel with a password from other devices (for example through the public address)'));
  const remoteSwitch = sw('Allow logging in to the panel with a password from other devices (for example through the public address)');
  check((await page.$eval('[data-password-state]', (e) => e.dataset.passwordState)) === 'unset', 'no password is set');
  await shot(page, 'after-settings-remote-en.png');
  m = await mark();
  await page.click(remoteSwitch);
  await page.waitForFunction(() => document.querySelector('.set-note.info')?.textContent.includes('First set a password'));
  await noActions(m, 'the remote panel cannot be switched on without a password');
  await page.fill('input[aria-label="New password"]', 'short');
  await page.evaluate(() => document.querySelector('input[aria-label="New password"]').form.requestSubmit());
  await sleep(300);
  await noActions(m, 'a short password is not sent (the browser\'s own minimum length)');
  await page.fill('input[aria-label="New password"]', 'longenough1');
  await page.click('form:has(input[aria-label="New password"]) button[type=submit]');
  a = await actions(m, 'password request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"password":"longenough1","remoteAdmin":true}', 'the password goes with the pending {remoteAdmin: true}', putConfigs(a)[0].body);
  await page.waitForFunction(() => document.querySelector('[data-password-state]')?.dataset.passwordState === 'set');
  check((await page.$eval('input[aria-label="New password"]', (i) => i.value)) === '' && (await isOn(page, 'Allow logging in to the panel with a password from other devices (for example through the public address)')), 'the field is cleared and the panel is on');
  m = await mark();
  await page.click(remoteSwitch);
  a = await actions(m, 'remote off request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"remoteAdmin":false}', 'switching off saves {remoteAdmin: false}', a);
  check(await noRawKeys(page), 'no raw keys in Remote panel');

  // ---------- Advanced ----------
  await goto(page, 'advanced');
  await page.waitForSelector('[data-cookies-path]');
  check((await text(page, '[data-cookies-path]')) === 'No file chosen.', 'no cookies file at first');
  await shot(page, 'after-settings-advanced-en.png');
  m = await mark();
  await clickText(page, 'Choose a file…');
  a = await actions(m, 'pick request');
  check(a[0].path === '/api/desktop/pick-cookies', 'Choose a file posts pick-cookies', a[0]);
  await page.waitForFunction(() => document.querySelector('[data-cookies-path]')?.textContent === 'C:/cookies.txt');
  check(true, 'the chosen path is shown');
  m = await mark();
  await clickText(page, 'Do not use the file');
  a = await actions(m, 'clear request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"ytdlpCookies":null}', 'clearing saves {ytdlpCookies: null}', a);
  m = await mark();
  await page.fill('input[aria-label="pixeldrain API key (optional)"]', '  abc123  ');
  await page.evaluate(() => document.querySelector('input[aria-label="pixeldrain API key (optional)"]').form.requestSubmit());
  a = await actions(m, 'pixeldrain request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"pixeldrainApiKey":"abc123"}', 'the key saves {pixeldrainApiKey}', a);
  // Fix round 2: the key is write-only — the field empties, says a key is saved, never shows it.
  const PD = 'input[aria-label="pixeldrain API key (optional)"]';
  await page.waitForSelector(`${PD}[data-key-saved="yes"]`);
  check((await page.$eval(PD, (e) => [e.value, e.placeholder].join('|'))) === '|•••• saved — type a new key to replace it', 'a saved key: empty field with the "saved" placeholder', await page.$eval(PD, (e) => [e.value, e.placeholder].join('|')));
  check(!(await page.content()).includes('abc123'), 'the key is nowhere in the page');
  const cfgAnswers = (await requests()).filter((r) => r.path.endsWith('/config'));
  check(cfgAnswers.length > 0, 'the settings were read again');
  const cfgNow = await (await fetch(`${base}/api/desktop/config`)).json();
  check(cfgNow.hasPixeldrainKey === true && !JSON.stringify(cfgNow).includes('abc123'), 'GET config says only hasPixeldrainKey', cfgNow);
  check(await page.$eval('#view .sform form.inline button[type=submit]', (b) => b.disabled && b.textContent.trim() === 'Replace'), 'Replace waits for a new key');
  m = await mark();
  await page.click('[data-remove-key]');
  a = await actions(m, 'remove key');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"pixeldrainApiKey":null}', 'Remove key saves {pixeldrainApiKey: null}', a);
  await page.waitForSelector(`${PD}[data-key-saved="no"]`);
  check((await page.$('[data-remove-key]')) === null, 'no key: nothing to remove');
  check(await noRawKeys(page), 'no raw keys in Advanced');

  // ---------- Log ----------
  await goto(page, 'log');
  await page.waitForFunction(() => document.querySelector('[data-log]')?.textContent.includes('Tunnel: online'));
  await shot(page, 'after-settings-log-en.png');
  m = await mark();
  await clickText(page, 'Refresh');
  await waitFor(async () => (await since(m)).some((r) => r.path === '/api/desktop/log'), 'log read');
  await page.click('#view button:has-text("Copy")');
  check((await page.evaluate(() => navigator.clipboard.readText())).includes('VRCinema 0.2.0: start'), 'Copy puts the log on the clipboard');
  m = await mark();
  await setLanguage(page, 'pl');
  await page.waitForFunction(() => document.querySelector('[data-log]')?.textContent.includes('Tunel: w sieci'));
  const logReq = (await since(m)).filter((r) => r.path === '/api/desktop/log' && r.lang === 'pl');
  check(logReq.length >= 1, 'the log is read again in the new language (the engine renders it per language)');
  check(await noRawKeys(page), 'no raw keys in the Polish Log');
  await setLanguage(page, 'en');
  await page.waitForFunction(() => document.querySelector('.sform h1')?.textContent === 'Log');

  // ---------- About ----------
  await goto(page, 'about');
  await page.waitForSelector('[data-version]');
  check((await text(page, '[data-version]')) === 'VRCinema, version 0.2.0', 'the version is shown');
  await shot(page, 'after-settings-about-en.png');
  await clickText(page, 'Check for updates');
  await page.waitForFunction(() => document.querySelector('.set-note.ok')?.textContent.includes('You have the latest version (0.2.0)'));
  check(true, 'a check with nothing new says so');
  await scenario({ updateOffer: { version: '0.3.0', notes: 'Fixes.' } });
  await clickText(page, 'Check for updates');
  await page.waitForSelector('[data-banner=update]');
  check((await text(page, '[data-banner=update] p')) === 'A new version is available: 0.3.0.', 'a new version is offered');
  m = await mark();
  await clickText(page, 'Skip this version');
  a = await actions(m, 'skip request');
  check(JSON.stringify(putConfigs(a)[0].body) === '{"skippedUpdateVersion":"0.3.0"}', 'skipping saves {skippedUpdateVersion}', a);
  await page.waitForFunction(() => !document.querySelector('[data-banner=update]'));
  await clickText(page, 'Check for updates');
  await page.waitForSelector('[data-banner=update]');
  m = await mark();
  await clickText(page, 'Update');
  a = await actions(m, 'apply request');
  check(a[0].path === '/api/desktop/update/apply', 'Update installs the version', a[0]);
  await page.waitForFunction(() => document.querySelector('.set-note.ok')?.textContent.includes('Version 0.3.0 is installed'));
  await scenario({ updateOffer: null });
  check(await noRawKeys(page), 'no raw keys in About');
  check(page.__errors.length === 0, 'no console errors on the window sections', page.__errors);

  // ---------- Every section in German (screenshots) ----------
  await setLanguage(page, 'de');
  for (const section of SECTIONS) {
    await goto(page, section);
    await sleep(350);
    check(await noOverflow(page), `no horizontal overflow on ${section} in German`);
    await shot(page, `after-settings-${section}-de.png`);
  }
  await page.close();

  // ---------- A remote session (fix round 1: the remote panel is full, spec section 2) ----------
  await reset();
  await scenario({ remote: true });
  page = await openApp('#/settings/general');
  await page.waitForSelector('[data-sign-out]');
  const enabledIn = (sel) => page.$$eval(sel, (els) => els.filter((e) => !e.disabled).length);
  check(await page.$eval('select[aria-label="Language"]', (e) => !e.disabled), 'remote: the program language can be changed');
  check(await page.$eval(sw('Start VRCinema with Windows'), (e) => !e.disabled), 'remote: starting with Windows can be changed');
  check(await page.$eval(sw('Allow opening the page from other devices on the home network'), (e) => e.disabled), 'remote: home network access stays in the window');
  check(await page.$eval('input[aria-label="Server port"]', (e) => e.disabled), 'remote: the port stays in the window');
  check((await page.$$('.window-only-hint')).length === 2, 'and both say so under their title');
  check((await page.$('.set-window-only')) === null, 'no whole-section window-only note');
  m = await mark();
  await page.selectOption('select[aria-label="Language"]', 'pl');
  a = await actions(m, 'remote program language');
  check(a[0].path === '/api/admin/config' && JSON.stringify(a[0].body) === '{"language":"pl"}', 'the program language goes to PUT /api/admin/config', a[0]);
  await page.waitForFunction(() => document.documentElement.lang === 'pl');
  check(true, 'with no language picked in this browser the panel follows it');
  await page.selectOption('select[aria-label="Język"]', 'en');
  await page.waitForFunction(() => document.documentElement.lang === 'en');
  await shot(page, 'after-settings-general-remote-en.png');
  await goto(page, 'internet');
  await page.waitForFunction(() => !document.querySelector('#view [role=switch]')?.disabled);
  check(await page.$eval('#view .kv button', (b) => !b.disabled), 'internet: Reconnect works remotely');
  check(await page.$eval(sw('Pause when the internet is gone'), (e) => !e.disabled), 'internet: the pauses work remotely');
  m = await mark();
  await page.click(sw('Pause when nobody is watching'));
  a = await actions(m, 'remote pause setting');
  check(a[0].path === '/api/admin/config' && JSON.stringify(a[0].body) === '{"pauseWhenNoViewers":true}', 'a pause setting → PUT /api/admin/config', a[0]);
  check(await page.$eval('.named-link button.pri', (b) => b.disabled), 'internet: setting up a permanent link stays in the window');
  await goto(page, 'tools');
  await page.waitForFunction(() => document.querySelectorAll('#view .kv.tool button:not([disabled])').length === 4);
  check(await page.$eval('#view .sform .pl-actions button', (b) => !b.disabled), 'tools: update, repair and the GPU test work remotely');
  await goto(page, 'remote');
  await page.waitForSelector('.set-window-only');
  check((await enabledIn('#view .sform button, #view .sform input')) === 0, 'remote panel: password and switch stay in the window');
  await goto(page, 'advanced');
  await page.waitForSelector('[data-cookies-path]');
  check(await page.$eval('[data-cookies-path] + .pl-actions button', (b) => b.disabled), 'advanced: choosing the cookies file stays in the window (a dialog on the computer)');
  check(await page.$eval('input[aria-label^="pixeldrain API key"]', (e) => !e.disabled), 'advanced: the pixeldrain key works remotely');
  await goto(page, 'log');
  await page.waitForFunction(() => document.querySelector('[data-log]')?.textContent.includes('VRCinema'));
  check(true, 'log: read remotely');
  await goto(page, 'about');
  check(await page.$eval('#view .sform .pl-actions button', (b) => !b.disabled), 'about: checking for an update works remotely');
  let all = await requests();
  const windowCalls = all.filter((r) => r.path.startsWith('/api/desktop/') && r.path !== '/api/desktop/info');
  check(windowCalls.length === 0, 'no window route was asked beyond the first info (403)', windowCalls.map((r) => r.path));
  check(['/api/admin/info', '/api/admin/config', '/api/admin/tools', '/api/admin/log'].every((r) => all.some((x) => x.path === r)), 'the panel routes were used');
  await goto(page, 'channel');
  await page.waitForSelector('.domains[data-list="allowed"]');
  m = await mark();
  await page.click(sw('Always convert the picture'));
  a = await actions(m, 'remote channel setting');
  check(JSON.stringify(putSettings(a)[0].body) === '{"forceTranscode":true}', 'the channel settings work in a remote session');
  await page.waitForSelector('.nick-row');
  check(true, 'and so do the nicks');
  check(page.__errors.filter((e) => !/403|window_only/.test(e)).length === 0, 'no unexpected console errors in the remote session', page.__errors);
  await page.close();

  // ---------- A server without a window ----------
  await reset();
  await scenario({ headless: true });
  page = await openApp('#/settings/general');
  await page.waitForSelector('.set-window-only');
  check(!(await page.$('.sel[aria-label="Language"]')), 'the headless server has no program language choice');
  check((await bodyText(page)).includes('set the language with LANGUAGE in the .env file'), 'it says the language is set with LANGUAGE in .env', await bodyText(page));
  await shot(page, 'after-settings-general-headless-en.png');
  await setLanguage(page, 'pl').catch(() => {});
  await page.close();

  // ---------- Not known yet: nothing window-only is asked ----------
  await reset();
  await scenario({ infoFail: true });
  page = await openApp('#/settings/general');
  await sleep(1200);
  all = await requests();
  check(all.filter((r) => r.path.startsWith('/api/desktop/') && r.path !== '/api/desktop/info').length === 0, 'while it is unknown nothing window-only is asked');
  check((await page.$('.set-window-only')) === null, 'and no window-only note is shown yet');
  check((await page.$$eval('#view .row2 select, #view .row2 button, #view .row2 input', (els) => els.every((e) => e.disabled))), 'the controls wait disabled');
  await scenario({ infoFail: false });
  await page.waitForFunction(() => document.querySelector('.sel[aria-label="Language"]:not([disabled])'), null, { timeout: 40000 });
  check(true, 'once the window answers the controls come alive');
  await page.close();

  // ---------- A narrow window ----------
  await reset();
  page = await openApp('#/settings/channel');
  await page.setViewportSize({ width: 600, height: 800 });
  await page.waitForSelector('.domains[data-list="allowed"]');
  await sleep(300);
  check(await noOverflow(page), 'no horizontal overflow on Channel at 600 px');
  await shot(page, 'after-settings-channel-600x800.png');
  check(page.__errors.length === 0, 'no console errors', page.__errors);
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
}
