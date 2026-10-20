// Browser check of the request page (`/`): the start language from navigator.languages, the
// language switch kept in localStorage, the device key made once and sent with a nick, the
// now-playing card, the queue with icons, the list of sites only in allowlist mode, the
// friendly errors (nick_taken with a suggestion, domain, local address, limits) and the
// layout at 320 px — in headless Brave with temporary profiles, against the fake engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/viewer-page.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8197);
if (!shots) throw new Error('usage: viewer-page.mjs <shots dir> [port]');
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

/** A browser with its own temporary profile and `locale` as navigator.languages. */
async function browser(locale, viewport = { width: 520, height: 900 }) {
  const profile = mkdtempSync(join(tmpdir(), 'brave-viewer-'));
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

async function open(ctx) {
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !m.text().includes('status of 4') && errors.push(m.text()));
  await page.goto(`${base}/`);
  await page.waitForSelector('.v-now');
  return { page, errors };
}

const text = (page, sel) => page.locator(sel).first().innerText();
const posts = async () => (await call('/__test/requests')).filter((r) => r.method === 'POST');
const sendRequest = async (page, url, nick) => {
  await page.fill('input[name=url]', url);
  if (nick !== undefined) await page.fill('input[name=nick]', nick);
  await page.click('.v-submit');
};

try {
  // ---------- Start language from navigator.languages ----------
  const de = await browser('de-DE');
  let { page, errors } = await open(de);
  check((await page.evaluate(() => document.documentElement.lang)) === 'de', 'de-DE starts in German');
  check((await page.locator('.v-lang select').inputValue()) === 'de', 'the switcher shows German');
  check((await text(page, '.v-head h1')) !== 'viewer.title', 'German text is loaded (no raw keys)');
  const stateCalls = (await call('/__test/requests')).filter((r) => r.path === '/api/channels/main/state');
  check(stateCalls.length > 0 && stateCalls.every((r) => r.lang === 'de'), 'requests carry X-VRCinema-Lang: de', stateCalls.map((r) => r.lang));
  check(errors.length === 0, 'no errors in the console (de)', errors);
  await page.close();

  const ko = await browser('ko-KR');
  ({ page, errors } = await open(ko));
  check((await page.evaluate(() => document.documentElement.lang)) === 'en', 'ko falls back to English');
  check((await text(page, '.v-head h1')) === 'Video channel', 'English title');

  // ---------- The switch is kept ----------
  await page.locator('.v-lang select').selectOption('pl');
  await waitFor(async () => (await text(page, '.v-head h1')) === 'Kanał filmowy', 'Polish after the switch');
  check((await page.evaluate(() => localStorage.getItem('vrcinema.lang'))) === 'pl', 'the choice is in localStorage');
  check((await page.evaluate(() => document.title)) === 'Kanał filmowy', 'the tab title follows the language');
  await page.reload();
  await page.waitForSelector('.v-now');
  check((await page.evaluate(() => document.documentElement.lang)) === 'pl', 'the choice survives a reload (browser is English)');
  await waitFor(async () => (await call('/__test/requests')).some((r) => r.path === '/api/channels/main/state' && r.lang === 'pl'), 'state requested with lang pl');
  const names = await page.locator('.v-lang option').allInnerTexts();
  check(names.length === 6 && names.includes('日本語 (beta)'), 'six languages, Japanese marked beta', names);

  // ---------- Now playing, the queue, Polish default title ----------
  check((await text(page, '.v-now-title')) === 'Never Gonna Give You Up', 'now playing title');
  check((await page.locator('.v-now svg.rbadge').count()) === 1, 'the requester of the playing video has an icon');
  const bar = page.locator('.v-progress');
  check((await bar.getAttribute('aria-valuenow')) === '80' && (await bar.getAttribute('aria-valuemax')) === '213', 'progress from positionS/durationS', await bar.getAttribute('aria-valuenow'));
  check((await text(page, '.v-now .v-time')) === '1:20 / 3:33', 'progress text', await text(page, '.v-now .v-time'));
  const rows = page.locator('.v-item');
  check((await rows.count()) === 4, 'four videos in the queue');
  check((await page.locator('.v-item svg.rbadge').count()) === 3, 'icons for the host and the two viewers with a key (not the anonymous one)');
  check((await page.locator('.v-item .live').count()) === 1, 'one LIVE tag');
  check((await rows.nth(2).innerText()).includes('NA ŻYWO'), 'LIVE tag in Polish');
  check((await rows.nth(3).innerText()).startsWith('Bez tytułu'), 'the stored default title in Polish');
  await page.locator('.v-lang select').selectOption('en');
  await waitFor(async () => (await rows.nth(3).innerText()).startsWith('Untitled'), 'the stored default title is localized');
  check((await rows.nth(1).innerText()).includes('added by the host'), 'the owner item says the host added it (viewer.added_by_host, not the owner-side "added here")');

  // ---------- Live: no bar ----------
  await scenario({ playing: 'live' });
  await waitFor(async () => (await page.locator('.v-progress').count()) === 0 && (await page.locator('.v-now .live').count()) === 1, 'live: no bar, LIVE tag');
  check((await text(page, '.v-now')).includes('Live stream'), 'live stream text');
  await scenario({ playing: 'none' });
  await waitFor(async () => (await text(page, '.v-now')).includes('Nothing is playing'), 'idle card');
  await scenario({ playing: 'film' });
  await waitFor(async () => (await page.locator('.v-progress').count()) === 1, 'film again');

  // ---------- Link mode: sites only in allowlist ----------
  check((await page.locator('.v-sites').count()) === 0, 'mode all: no list of sites');
  check((await text(page, '.v-rules')).includes('Paste a link to a video.'), 'mode all: generic hint');
  await scenario({ settingsPatch: { linkMode: 'allowlist' } });
  await waitFor(async () => (await page.locator('.v-sites li').count()) === 2, 'allowlist: the list of sites');
  check((await page.locator('.v-sites').innerText()).includes('youtube.com'), 'allowlist: youtube.com listed');
  await scenario({ settingsPatch: { linkMode: 'blocklist' } });
  await waitFor(async () => (await page.locator('.v-sites').count()) === 0, 'blocklist: no list');
  await scenario({ settingsPatch: { linkMode: 'all' } });

  // ---------- The device key: made once, sent with a nick ----------
  check((await page.evaluate(() => localStorage.getItem('vrcinema.viewer.deviceKey'))) === null, 'no key before a nick is used');
  await sendRequest(page, 'https://www.youtube.com/watch?v=anon');
  await waitFor(async () => (await posts()).length === 1, 'anonymous request sent');
  check(!('deviceKey' in (await posts())[0].body), 'anonymous request without a nick or key');
  await waitFor(async () => (await text(page, '.v-toast')).includes('Added'), 'success toast');
  check((await page.locator('.v-toast-slot[role=status]').count()) === 1, 'the toast is in a live region');
  await sendRequest(page, 'https://www.youtube.com/watch?v=one', 'Ola');
  await waitFor(async () => (await posts()).length === 2, 'first request with a nick');
  const first = (await posts())[1].body;
  check(/^[0-9a-f]{32}$/.test(first.deviceKey) && first.name === 'Ola', 'device key is 32 hex and goes with the nick', first);
  check((await page.evaluate(() => localStorage.getItem('vrcinema.viewer.deviceKey'))) === first.deviceKey, 'the key is stored');
  await waitFor(async () => (await page.locator('.v-own-badge svg').count()) === 1, 'own icon next to the nick field');
  check((await page.locator('.v-own-badge').getAttribute('aria-label')) === 'Your icon on this device', 'own icon has a name');
  await page.reload();
  await page.waitForSelector('.v-now');
  check((await page.locator('input[name=nick]').inputValue()) === 'Ola', 'the nick is remembered');
  check((await page.locator('.v-own-badge svg').count()) === 1, 'own icon remembered');
  await sendRequest(page, 'https://www.youtube.com/watch?v=two');
  await waitFor(async () => (await posts()).length === 3, 'second request with a nick');
  const second = (await posts())[2].body;
  check(second.deviceKey === first.deviceKey && second.name === 'Ola', 'the same key is reused after a reload');
  await shot(page, 'viewer-pl-switch');
  async function shot(p, name) {
    await p.screenshot({ path: join(shots, `${name}.png`), fullPage: true });
  }

  // ---------- Errors ----------
  await scenario({ viewerNickTaken: ['tomek'] });
  await page.fill('input[name=nick]', 'Tomek');
  await page.evaluate(() => localStorage.setItem('vrcinema.viewer.deviceKey', 'f'.repeat(32)));
  await sendRequest(page, 'https://www.youtube.com/watch?v=taken');
  await page.waitForSelector('.v-error[data-code=nick_taken]');
  check((await text(page, '.v-error')).includes('already taken'), 'nick_taken is shown in English', await text(page, '.v-error'));
  const suggestion = await page.locator('.v-error .btn').innerText();
  check(/^Use “Tomek\d\d”$/.test(suggestion), 'a different nick is suggested', suggestion);
  check((await page.locator('input[name=url]').getAttribute('aria-invalid')) === 'true', 'the field is marked invalid');
  await shot(page, 'viewer-nick-taken');
  await page.locator('.v-error .btn').click();
  check(/^Tomek\d\d$/.test(await page.locator('input[name=nick]').inputValue()), 'the suggestion fills the nick');
  // Final wave M4: the button goes away with the error; the focus goes to the nick field.
  await page.waitForFunction(() => document.activeElement?.name === 'nick', null, { timeout: 2000 }).catch(() => {});
  check(await page.evaluate(() => document.activeElement?.name === 'nick'), 'the focus moves to the nick field', await page.evaluate(() => document.activeElement?.outerHTML?.slice(0, 80)));
  await page.click('.v-submit');
  await waitFor(async () => (await page.locator('.v-error').count()) === 0, 'the suggested nick goes through');
  // The error follows a language switch.
  await page.evaluate(() => localStorage.setItem('vrcinema.viewer.deviceKey', 'e'.repeat(32)));
  await sendRequest(page, 'https://vimeo.com/1', 'Tomek');
  await page.waitForSelector('.v-error[data-code=nick_taken]');
  await page.locator('.v-lang select').selectOption('pl');
  await waitFor(async () => (await text(page, '.v-error')).includes('zajęty'), 'the error is translated after a language switch');
  await page.locator('.v-lang select').selectOption('en');
  await scenario({ viewerNickTaken: [] });

  const refused = [
    ['domain_not_allowed', 'not allowed'],
    ['local_address', 'local network'],
    ['too_many_active', 'You already have 5 videos'],
    ['too_many_requests', 'Too many requests'],
  ];
  for (const [code, fragment] of refused) {
    await scenario({ viewerError: code });
    await page.fill('input[name=nick]', 'Ola');
    await sendRequest(page, 'https://example.org/v');
    await page.waitForSelector(`.v-error[data-code=${code}]`);
    check((await text(page, '.v-error')).includes(fragment), `${code} is shown`, await text(page, '.v-error'));
  }
  await scenario({ viewerError: null });
  // Final wave M3: a request that never reached the server is not retried: the page says so.
  await page.route('**/api/channels/main/requests', (r) => r.abort());
  await page.fill('input[name=nick]', 'Ola');
  await sendRequest(page, 'https://example.org/lost');
  await page.waitForSelector('.v-error[data-code=network]');
  check(
    (await text(page, '.v-error')) === 'Could not send the request. Check the connection and try again.',
    'a request that could not be sent says so (no "trying again")',
    await text(page, '.v-error'),
  );
  await page.unroute('**/api/channels/main/requests');
  // The aborted request is logged by the browser itself; that one is expected.
  const aborted = errors.findIndex((e) => e.includes('net::ERR_FAILED'));
  if (aborted >= 0) errors.splice(aborted, 1);
  // Final wave M4: a slow channel never gets a second state request while one is on its way.
  await scenario({ viewerStateDelayMs: 4000 });
  await page.waitForTimeout(9000);
  const { maxViewerInFlight } = await call('/__test/stats');
  await scenario({ viewerStateDelayMs: 0 });
  check(maxViewerInFlight === 1, 'one state request at a time while the channel answers slowly', maxViewerInFlight);
  await page.fill('input[name=nick]', '');
  await sendRequest(page, 'ftp://x');
  await page.waitForSelector('.v-error[data-code=invalid_link]');
  check((await text(page, '.v-error')).includes('valid link'), 'invalid_link is shown');

  // ---------- Moderation: the pending message ----------
  await scenario({ settingsPatch: { moderation: true } });
  await waitFor(async () => (await text(page, '.v-rules')).includes('approves'), 'moderation hint');
  await sendRequest(page, 'https://www.youtube.com/watch?v=mod');
  await waitFor(async () => (await page.locator('.v-toast').count()) === 1 && (await text(page, '.v-toast')).includes('approval'), 'pending toast');
  await scenario({ settingsPatch: { moderation: false } });

  // ---------- A key that is not usable: private mode keeps it in memory ----------
  check(errors.length === 0, 'no errors in the console (ko/pl/en)', errors);
  await shot(page, 'viewer-en');
  await page.close();

  // ---------- Japanese, German and the phone width ----------
  await scenario({ settingsPatch: { linkMode: 'allowlist' } });
  for (const [locale, name] of [['ja-JP', 'ja'], ['de-DE', 'de']]) {
    const ctx = await browser(locale, { width: 320, height: 700 });
    const opened = await open(ctx);
    check((await opened.page.evaluate(() => document.documentElement.lang)) === name, `${name} starts in ${name}`);
    const wide = await opened.page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    check(wide <= 0, `${name}: no horizontal scroll at 320 px`, wide);
    const small = await opened.page.evaluate(() =>
      [...document.querySelectorAll('button, input, select, summary')].filter((e) => e.getBoundingClientRect().height < 24 && e.getBoundingClientRect().width > 0).map((e) => e.tagName),
    );
    check(small.length === 0, `${name}: controls are not tiny`, small);
    await shot(opened.page, `viewer-${name}-320`);
    if (name === 'ja') {
      const labels = await opened.page.locator('.v-lang option').allInnerTexts();
      check(labels.includes('日本語 (beta)'), 'ja: the Japanese option is marked beta');
    }
    check(opened.errors.length === 0, `no errors in the console (${name})`, opened.errors);
    await opened.page.close();
  }
  await scenario({ settingsPatch: { linkMode: 'all' } });

  // ---------- No storage: the page still works ----------
  const blocked = await browser('en-US');
  const bp = await blocked.newPage();
  await bp.addInitScript(() => {
    Object.defineProperty(window, 'localStorage', { get() { throw new Error('blocked'); } });
  });
  await bp.goto(`${base}/`);
  await bp.waitForSelector('.v-now');
  await bp.fill('input[name=url]', 'https://www.youtube.com/watch?v=nostorage');
  await bp.fill('input[name=nick]', 'Memory');
  await bp.click('.v-submit');
  await waitFor(async () => (await posts()).some((r) => r.body.name === 'Memory' && /^[0-9a-f]{32}$/.test(r.body.deviceKey)), 'a nick works without storage');
  await bp.close();

  console.log(`\n${passed} checks passed`);
} finally {
  for (const c of contexts) await c.close().catch(() => {});
  await engine.close();
  for (const p of profiles) rmSync(p, { recursive: true, force: true });
}
process.exit(0);
