// Browser check of the first-run wizard (language applied at once to the next steps, tools
// download with progress, graphics card test, the internet step with links and QR, "Skip" in
// every state without a connection, coming back to the wizard after an interruption) against
// the fake engine, in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/wizard.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8198);
if (!shots) throw new Error('usage: wizard.mjs <shots dir> [port]');
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
  viewport: { width: 900, height: 760 },
  args: ['--disable-extensions', '--no-first-run'],
});
await ctx.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: base });

const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const requests = () => call('/__test/requests');
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

async function open() {
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/wizard`);
  page.errors = errors;
  return page;
}
const text = (page, sel) => page.locator(sel).first().innerText();
const heading = (page) => text(page, 'h1');
const has = (page, sel) => page.locator(sel).count().then((n) => n > 0);
const shot = (page, name) => page.screenshot({ path: join(shots, `${name}.png`) });
const sent = async (method, path) => (await requests()).filter((r) => r.method === method && r.path === path);

try {
  // ---------- Step 1: the detected language is the default; a choice applies at once ----------
  await reset();
  await scenario({ firstRun: true, language: 'pl', tunnel: 'off', toolsMissing: ['yt-dlp', 'ffmpeg', 'deno', 'cloudflared'] });
  let page = await open();
  await waitFor(() => has(page, '.wz-lang'), 'language buttons');
  check((await heading(page)) === 'Witaj w VRCinema', 'step 1 renders in the detected language (pl)');
  check((await page.locator('.wz-lang').count()) === 6, 'six languages are offered');
  check((await page.locator('[data-lang=pl]').getAttribute('aria-checked')) === 'true', 'the detected language is selected');
  check((await text(page, '[data-lang=ja]')) === '日本語 (beta)', 'Japanese is marked beta');
  check((await text(page, '.wz-no')) === 'Krok 1 z 4', 'the step counter is Polish');
  await shot(page, '1-language-pl');

  await page.click('[data-lang=en]');
  await waitFor(async () => (await heading(page)) === 'Welcome to VRCinema', 'English applied live');
  const put = (await sent('PUT', '/api/desktop/config')).at(-1);
  check(put?.body?.language === 'en', 'selecting a language saves it with PUT /api/desktop/config', put?.body);
  check((await page.locator('[data-lang=en]').getAttribute('aria-checked')) === 'true', 'the choice is marked');
  check((await text(page, '.wz-no')) === 'Step 1 of 4', 'the counter follows the language');
  await shot(page, '1-language-en');
  await page.click('[data-lang=pl]');
  await waitFor(async () => (await heading(page)) === 'Witaj w VRCinema', 'back to Polish');

  // ---------- Step 2: the tools, with progress, in the chosen language ----------
  await page.click('[data-next]');
  await waitFor(async () => (await heading(page)) === 'Pobieranie narzędzi', 'step 2 in Polish');
  await waitFor(() => has(page, '.tool-row[data-kind=active]'), 'a tool is downloading');
  check((await page.locator('.tool-row').count()) === 4, 'four tool rows');
  check(/Pobieranie/.test(await text(page, '.tool-row[data-kind=active] .state')), 'progress text is Polish', await text(page, '.tool-row[data-kind=active] .state'));
  check((await page.locator('.tool-row[data-kind=active] [role=progressbar]').getAttribute('aria-valuenow')) !== null, 'the active bar has a value');
  check(!(await has(page, '[data-next]')), 'no Next while downloading');
  await shot(page, '2-tools-downloading');
  await waitFor(() => has(page, '[data-next]'), 'tools done', 20000);
  check((await page.locator('.tool-row[data-kind=ready]').count()) === 4, 'every tool ready');
  check((await sent('POST', '/api/desktop/tools/install')).length === 1, 'one install request');
  check((await text(page, '.wz-note.ok')) === 'Wszystkie narzędzia są gotowe.', 'done note');
  await shot(page, '2-tools-done');

  // ---------- Step 3: the graphics card ----------
  await page.click('[data-next]');
  await waitFor(() => has(page, '.wz-result'), 'encoder result');
  check((await heading(page)) === 'Test karty graficznej', 'step 3 title');
  check((await page.locator('.wz-result').getAttribute('data-encoder')) === 'h264_nvenc', 'detected encoder shown');
  check(/NVIDIA/.test(await text(page, '.wz-result strong')), 'encoder named');
  check((await sent('POST', '/api/desktop/encoder/detect')).length === 1, 'one detect request');
  await shot(page, '3-encoder');

  // ---------- Step 4: the internet — connecting offers Skip only ----------
  await scenario({ tunnelOnEnable: 'connecting' });
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-tunnel=connecting]'), 'connecting state');
  check((await heading(page)) === 'Twój kanał w internecie', 'step 4 title');
  check((await text(page, '.wz-note')) === 'Łączenie z internetem…', 'connecting text');
  check((await has(page, '[data-skip]')) && !(await has(page, '[data-retry]')), 'connecting: Skip, no Retry');
  check((await sent('PUT', '/api/desktop/config')).some((r) => r.body?.tunnelEnabled === true), 'the tunnel is switched on by the wizard');
  await shot(page, '4-connecting');

  await scenario({ tunnel: 'reconnecting' });
  await waitFor(() => has(page, '[data-tunnel=reconnecting]'), 'reconnecting state');
  check((await has(page, '[data-skip]')) && (await has(page, '[data-retry]')), 'reconnecting: Skip and Retry');
  check(/próbuje dalej/.test(await text(page, '.wz-note.err')), 'reconnecting message', await text(page, '.wz-note.err'));
  check(!/Allow Untrusted/i.test(await page.content()), 'no sentence about Allow Untrusted URLs');
  check(!(await has(page, 'input[type=url], input[type=text]')), 'no manual public-address field');
  await shot(page, '4-reconnecting');

  await scenario({ tunnel: 'error' });
  await waitFor(() => has(page, '[data-tunnel=error]'), 'error state');
  check((await text(page, '.wz-note.err')) === 'No internet', 'error shows the reason');
  check((await has(page, '[data-skip]')) && (await has(page, '[data-retry]')), 'error: Skip and Retry');
  await shot(page, '4-error');
  await page.click('[data-retry]');
  check(await waitFor(async () => (await sent('POST', '/api/desktop/tunnel/restart')).length === 1, 'retry restarts the tunnel'), 'Retry posts tunnel/restart');

  await scenario({ tunnel: 'online' });
  await waitFor(() => has(page, '[data-tunnel=online]'), 'online state');
  const address = (await call('/__test/scenario', {})).tunnelUrl;
  const channelLink = `${address}/live/main.m3u8`;
  check((await text(page, '[data-link=channel] .field-value')) === channelLink, 'channel link', await text(page, '[data-link=channel] .field-value'));
  check((await text(page, '[data-link=request] .field-value')) === `${address}/`, 'request page link');
  check(await page.locator('.qr-img').evaluate((img) => img.complete && img.naturalWidth > 0), 'the QR image loaded');
  check((await has(page, '[data-finish]')) && (await has(page, '[data-permanent]')) && !(await has(page, '[data-skip]')), 'online: Done and permanent link');
  await page.click('[data-link=channel] button');
  check((await page.evaluate(() => navigator.clipboard.readText())) === channelLink, 'copy puts the channel link on the clipboard');
  await shot(page, '4-online');
  await page.click('[data-finish]');
  await page.waitForURL(`${base}/app#/channel`);
  check((await sent('POST', '/api/desktop/wizard/complete')).length === 1, 'finishing completes the first run');
  check(page.url() === `${base}/app#/channel`, 'finishing opens the interface', page.url());
  check(page.errors.length === 0, 'no script errors', page.errors);
  await page.close();

  // ---------- Completed first run: /wizard goes on to the interface ----------
  page = await open();
  await page.waitForURL(`${base}/app`);
  check(true, 'a finished wizard redirects to /app');
  await page.close();

  // ---------- "I want a permanent link" opens Settings -> Internet ----------
  await reset();
  await scenario({ firstRun: true, tunnel: 'off', toolsMissing: [] });
  page = await open();
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'tools already installed');
  check((await text(page, '.wz-note.ok')) === 'The tools are already installed.', 'already installed note');
  check((await sent('POST', '/api/desktop/tools/install')).length === 0, 'nothing is downloaded when the tools are there');
  await page.click('[data-next]');
  await waitFor(() => has(page, '.wz-result'), 'encoder');
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-permanent]'), 'online');
  await page.click('[data-permanent]');
  await page.waitForURL(`${base}/app#/settings/internet`);
  check(true, 'the permanent link button opens Settings -> Internet');
  await page.close();

  // ---------- Interruption: reopening the wizard starts again, language kept ----------
  await reset();
  await scenario({ firstRun: true, language: 'en', tunnel: 'off', toolsMissing: ['yt-dlp', 'ffmpeg', 'deno', 'cloudflared'] });
  page = await open();
  await waitFor(() => has(page, '.wz-lang'), 'step 1');
  await page.click('[data-lang=pl]');
  await waitFor(async () => (await heading(page)) === 'Witaj w VRCinema', 'Polish');
  await page.click('[data-next]');
  await waitFor(() => has(page, '.tool-row[data-kind=active]'), 'downloading');
  await page.close(); // the window is closed in the middle of the download
  page = await open();
  await waitFor(() => has(page, '.wz-lang'), 'wizard comes back');
  check((await heading(page)) === 'Witaj w VRCinema', 'the wizard returns in the saved language');
  check((await page.locator('[data-lang=pl]').getAttribute('aria-checked')) === 'true', 'saved language selected');
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'the download finishes (watched, not restarted)', 20000);
  check(true, 'the interrupted download is picked up');
  await page.close();

  // ---------- Failed download: retry, or skip to the panel ----------
  await reset();
  await scenario({ firstRun: true, language: 'en', toolsFail: true, toolsMissing: ['yt-dlp'] });
  page = await open();
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-retry]'), 'download failure', 20000);
  check(/The tools could not be downloaded/.test(await text(page, '.wz-note.err')), 'failure message', await text(page, '.wz-note.err'));
  check(await has(page, '[data-skip]'), 'failed download offers skipping');
  await shot(page, '2-tools-failed');
  await scenario({ toolsFail: false });
  await page.click('[data-retry]');
  await waitFor(() => has(page, '[data-next]'), 'retry works', 20000);
  check(true, 'Retry downloads again');
  await page.close();
  await reset();
  await scenario({ firstRun: true, toolsFail: true, toolsMissing: ['yt-dlp'] });
  page = await open();
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-skip]'), 'failure', 20000);
  await page.click('[data-skip]');
  await page.waitForURL(`${base}/app#/channel`);
  check((await sent('POST', '/api/desktop/wizard/complete')).length === 1, 'skipping after a failed download completes the first run');
  await page.close();

  // ---------- Skip while still connecting ends the wizard ----------
  await reset();
  await scenario({ firstRun: true, tunnel: 'off', tunnelOnEnable: 'connecting', toolsMissing: [] });
  page = await open();
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'tools');
  await page.click('[data-next]');
  await waitFor(() => has(page, '.wz-result'), 'encoder');
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-tunnel=connecting]'), 'connecting');
  await page.click('[data-skip]');
  await page.waitForURL(`${base}/app#/channel`);
  check((await sent('POST', '/api/desktop/wizard/complete')).length === 1, 'Skip while connecting completes the first run');
  await page.close();

  // ---------- Not the program's window: no wizard ----------
  await reset();
  await scenario({ firstRun: true, remote: true });
  page = await open();
  await page.waitForURL(`${base}/app`);
  check(true, 'a remote session is sent to the interface, not the wizard');
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close();
  rmSync(profile, { recursive: true, force: true });
}
