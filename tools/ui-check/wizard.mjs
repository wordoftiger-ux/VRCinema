// Browser check of the setup wizard (spec 2026-11-08-wizard-design): the language applied at
// once, the tools download with progress, the picture (card test, encoder, resolution, light
// profile note), who can add videos, the info ticker, slates and audio, the internet (Skip in
// every state without a connection) with the remote panel, OBS and the VRChat chatbox, the
// summary (copy link for VRChat, test checklist, Settings links), "Back" and "Skip to the
// end", "Run setup again" from Settings → General, coming back after an interruption — against
// the fake engine, in headless Brave with a temporary profile. Every control must send the
// same request as its twin in Settings.
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
  viewport: { width: 960, height: 720 },
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

async function open({ trackFocus = false, path = '/wizard' } = {}) {
  const page = await ctx.newPage();
  if (trackFocus) {
    await page.addInitScript(() => {
      window.__focus = [];
      const original = HTMLElement.prototype.focus;
      HTMLElement.prototype.focus = function (...args) {
        window.__focus.push(this.tagName === 'H1' ? 'h1' : this.getAttribute('data-next') !== null ? 'next' : this.tagName.toLowerCase());
        return original.apply(this, args);
      };
    });
  }
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(base + path);
  page.errors = errors;
  return page;
}
const text = (page, sel) => page.locator(sel).first().innerText();
const heading = (page) => text(page, 'h1');
const has = (page, sel) => page.locator(sel).count().then((n) => n > 0);
const shot = (page, name) => page.screenshot({ path: join(shots, `${name}.png`), fullPage: true });
const sent = async (method, path) => (await requests()).filter((r) => r.method === method && r.path === path);
const SETTINGS = '/api/admin/channels/main/settings';
/** The last channel settings saved with `key` in the body. */
const savedSetting = async (key) => (await sent('PUT', SETTINGS)).filter((r) => r.body && key in r.body).at(-1)?.body;
const savedConfig = async (key) => (await sent('PUT', '/api/desktop/config')).filter((r) => r.body && key in r.body).at(-1)?.body;
const step = (page) => page.locator('main.wz').getAttribute('data-step');
async function next(page, id) {
  await page.click('[data-next]');
  await waitFor(async () => (await step(page)) === id, `step ${id}`);
}
const switchFor = (page, name) => page.getByRole('switch', { name, exact: true });

try {
  // ---------- Step 1: the detected language is the default; a choice applies at once ----------
  await reset();
  await scenario({ firstRun: true, language: 'pl', tunnel: 'off', toolsMissing: ['yt-dlp', 'ffmpeg', 'deno', 'cloudflared'], settingsPatch: { allowedDomains: [] } });
  let page = await open();
  await waitFor(() => has(page, '.wz-lang'), 'language buttons');
  check((await heading(page)) === 'Witaj w VRCinema', 'step 1 renders in the detected language (pl)');
  check((await page.locator('.wz-lang').count()) === 6, 'six languages are offered');
  check((await page.locator('[data-lang=pl]').getAttribute('aria-pressed')) === 'true', 'the detected language is selected');
  check((await text(page, '[data-lang=ja]')) === '日本語 (beta)', 'Japanese is marked beta');
  check((await text(page, '.wz-no')) === 'Krok 1 z 9', 'the step counter is Polish, nine steps on the first run');
  check(!(await has(page, '[data-to-end]')), 'no "Skip to the end" before the tools on the first run');
  await page.click('[data-lang=en]');
  await waitFor(async () => (await heading(page)) === 'Welcome to VRCinema', 'English applied live');
  check((await savedConfig('language'))?.language === 'en', 'selecting a language saves it with PUT /api/desktop/config');
  const frame = (code) => page.locator(`[data-lang=${code}]`).evaluate((el) => getComputedStyle(el).borderTopColor);
  const amber = await page.evaluate(() => {
    const probe = document.createElement('i');
    probe.style.color = 'var(--amber)';
    document.body.append(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
  });
  check((await frame('en')) === amber && (await frame('pl')) !== amber, 'the chosen language has the amber frame, the others not');
  check((await text(page, '.wz-no')) === 'Step 1 of 9', 'the counter follows the language');
  await waitFor(() => has(page, '[data-slate-language]'), 'slate language select');
  await page.selectOption('[data-slate-language]', 'de');
  await waitFor(async () => (await savedSetting('slateLanguage'))?.slateLanguage === 'de', 'slate language saved');
  check(true, 'the slate language is saved as in Settings → Channel');
  check((await text(page, '.wz-later')) === 'You can change this later in Settings → General, Settings → Channel.', 'step 1 names its Settings places', await text(page, '.wz-later'));
  await shot(page, '1-language');

  // ---------- Step 2: the tools ----------
  await page.click('[data-next]');
  await waitFor(() => has(page, '.tool-row[data-kind=active]'), 'a tool is downloading');
  check((await heading(page)) === 'Downloading the tools', 'step 2 title');
  check((await page.locator('.tool-row').count()) === 4, 'four tool rows');
  check(!(await has(page, '[data-next]')), 'no Next while downloading');
  await waitFor(() => has(page, '[data-next]'), 'tools done', 20000);
  check((await sent('POST', '/api/desktop/tools/install')).length === 1, 'one install request');

  // ---------- Step 3: the picture ----------
  await next(page, 'picture');
  await waitFor(() => has(page, '.wz-result'), 'encoder result');
  check((await heading(page)) === 'Picture and graphics card', 'step 3 title');
  check((await page.locator('.wz-result').getAttribute('data-encoder')) === 'h264_nvenc', 'detected encoder shown');
  check((await sent('POST', '/api/desktop/encoder/detect')).length === 1, 'one detect request');
  check(!(await has(page, '[data-light-profile]')), 'no light profile note on a computer with a card');
  check((await has(page, '[data-back]')) && (await has(page, '[data-to-end]')), 'Back and Skip to the end after the tools');
  await page.selectOption('select[name=videoEncoder]', 'libx264');
  await waitFor(async () => (await savedConfig('videoEncoder'))?.videoEncoder === 'libx264', 'encoder saved');
  check(true, 'the encoder choice is saved in the program settings');
  await page.selectOption('select[name=maxHeight]', '720');
  await waitFor(async () => (await savedSetting('maxHeight'))?.maxHeight === 720, 'max height saved');
  check(true, 'the highest resolution is saved as in Settings → Advanced');
  check((await page.locator('.wz-later').getAttribute('data-later')) === 'tools advanced', 'picture step points to Tools and Advanced');
  await shot(page, '3-picture');
  // Back skips the finished download.
  await page.click('[data-back]');
  await waitFor(async () => (await step(page)) === 'language', 'back to the language');
  check(true, 'Back from the picture goes to the language, past the tools');
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'tools step again');
  check((await text(page, '.wz-note.ok')) === 'The tools are already installed.', 'the tools are not downloaded again');
  await next(page, 'picture');
  await waitFor(() => has(page, '.wz-result'), 'encoder result again');

  // ---------- Step 4: who can add videos ----------
  await next(page, 'viewers');
  check((await heading(page)) === 'Who can add videos', 'step 4 title');
  await waitFor(() => has(page, 'input[name=linkMode]'), 'link modes');
  check(await page.locator('input[name=linkMode][value=all]').isChecked(), 'the default link mode is "all"');
  check((await switchFor(page, "Allow viewers to send small video and audio files").getAttribute('aria-checked')) === 'false', 'files from viewers are off by default');
  check((await switchFor(page, 'Allow viewers to add lists').getAttribute('aria-checked')) === 'false', 'viewer lists are off by default');
  check(/trust your viewers/.test(await text(page, '[data-security-note]')), 'one-line security note under files from viewers');
  await page.click('input[name=linkMode][value=allowlist]');
  await waitFor(async () => (await savedSetting('linkMode'))?.linkMode === 'allowlist', 'link mode saved');
  await waitFor(() => has(page, '[data-warning=allowlist_empty] [data-action=popular]'), 'empty allowlist warning with popular sites');
  check(await has(page, '[data-lists-later]'), 'the lists are edited in Settings');
  await page.click('[data-action=popular]');
  await waitFor(async () => ((await savedSetting('allowedDomains'))?.allowedDomains ?? []).includes('youtube.com'), 'popular sites saved');
  check(!(await has(page, '[data-warning=allowlist_empty]')), 'the warning is gone once sites are allowed');
  await switchFor(page, "Approve viewers' requests before they join the queue").click();
  await waitFor(async () => (await savedSetting('moderation'))?.moderation === true, 'moderation saved');
  check(true, 'approval is switched on with the Settings request');
  await page.fill('input[name=maxActivePerRequester]', '2');
  await page.fill('input[name=maxDurationMin]', '30');
  await page.click('form[data-form=limits] button[type=submit]');
  await waitFor(async () => (await savedSetting('maxActivePerRequester'))?.maxActivePerRequester === 2, 'limits saved');
  check((await savedSetting('maxDurationS')).maxDurationS === 1800, 'the longest video is saved in seconds');
  await shot(page, '4-viewers');

  // ---------- Step 5: the info ticker ----------
  await next(page, 'ticker');
  check((await heading(page)) === 'Info ticker', 'step 5 title');
  await waitFor(() => has(page, '[data-ticker=on]'), 'ticker on by default');
  await page.getByRole('radio', { name: 'Continuous' }).click();
  await waitFor(async () => (await savedSetting('tickerMode'))?.tickerMode === 'continuous', 'ticker mode saved');
  await page.getByRole('radiogroup', { name: 'QR code of the request page' }).getByRole('radio', { name: 'Always' }).click();
  await waitFor(async () => (await savedSetting('tickerQr'))?.tickerQr === 'always', 'ticker QR saved');
  await page.fill('input[aria-label="New message"]', 'Hello from the wizard');
  await page.click('[data-messages] form button[type=submit]');
  await waitFor(async () => ((await savedSetting('tickerMessages'))?.tickerMessages ?? []).includes('Hello from the wizard'), 'message saved');
  check(true, 'mode, QR and a message are saved as in Settings');
  await shot(page, '5-ticker');

  // ---------- Step 6: slates and audio ----------
  await next(page, 'look');
  check((await heading(page)) === 'Slates and audio', 'step 6 title');
  await waitFor(() => has(page, '[data-idle-qr]'), 'idle slate QR note');
  await page.selectOption('select[name=slateFrameStyle]', 'blur');
  await waitFor(async () => (await savedSetting('slateFrameStyle'))?.slateFrameStyle === 'blur', 'frame style saved');
  await page.selectOption('select[aria-label="Picture for audio"]', 'spectrum');
  await waitFor(async () => (await savedSetting('audioVisual'))?.audioVisual === 'spectrum', 'audio look saved');
  check(true, 'the last frame style and the audio look are saved');
  await shot(page, '6-look');

  // ---------- Step 7: the internet and the remote panel ----------
  await scenario({ tunnelOnEnable: 'connecting' });
  await next(page, 'internet');
  await waitFor(() => has(page, '[data-tunnel=connecting]'), 'connecting state');
  check((await heading(page)) === 'Your channel on the internet', 'step 7 title');
  check((await text(page, '[data-next]')) === 'Skip', 'without a connection Next reads Skip');
  check((await savedConfig('tunnelEnabled'))?.tunnelEnabled === true, 'the first run switches the tunnel on');
  await scenario({ tunnel: 'error' });
  await waitFor(() => has(page, '[data-tunnel=error]'), 'error state');
  check((await has(page, '[data-retry]')) && (await has(page, '[data-next]')), 'error: Retry and Skip');
  await page.click('[data-retry]');
  await waitFor(async () => (await sent('POST', '/api/desktop/tunnel/restart')).length === 1, 'retry restarts the tunnel');
  await scenario({ tunnel: 'online' });
  await waitFor(() => has(page, '[data-tunnel=online]'), 'online state');
  const address = (await call('/__test/scenario', {})).tunnelUrl;
  check((await text(page, '[data-link=channel] .field-value')) === `${address}/live/main.m3u8`, 'channel link');
  check(await page.locator('.qr-img').evaluate((img) => img.complete && img.naturalWidth > 0), 'the QR image loaded');
  check((await text(page, '[data-next]')) === 'Next' && (await has(page, '[data-permanent]')), 'online: Next and the permanent link');
  check(await has(page, '[data-block=remote] [role=switch]'), 'the remote panel switch is in the step');
  await page.fill('[data-block=remote] input[type=password]', 'sekretne-haslo');
  await page.click('[data-block=remote] form button[type=submit]');
  await waitFor(async () => (await savedConfig('password'))?.password === 'sekretne-haslo', 'password saved');
  check(true, 'the remote panel password is set with the Settings request');
  await shot(page, '7-internet');

  // ---------- Step 8: OBS and the VRChat chatbox ----------
  await next(page, 'extras');
  check((await heading(page)) === 'OBS and VRChat', 'step 8 title');
  await waitFor(() => has(page, '[data-obs-value=server]'), 'OBS address');
  check((await text(page, '[data-obs-value=server]')) === 'rtmp://127.0.0.1:1935/live', 'the OBS address is shown');
  check(await has(page, '[data-obs-value=key]'), 'the stream key is there (hidden until shown)');
  await switchFor(page, 'Allow OBS from other devices on the network').click();
  await waitFor(async () => (await savedConfig('obsLan'))?.obsLan === true, 'OBS LAN saved');
  await switchFor(page, 'Show the video in the chatbox').click();
  await waitFor(async () => (await savedSetting('oscEnabled'))?.oscEnabled === true, 'chatbox saved');
  check(/Settings → Advanced/.test(await text(page, '[data-advanced]')), 'the Advanced options are only named');
  check(!(await has(page, '[data-block=obs] select, [data-advanced] [role=switch]')), 'no Advanced controls in the wizard');
  await shot(page, '8-extras');

  // ---------- Step 9: the summary ----------
  await next(page, 'summary');
  check((await heading(page)) === 'Ready', 'step 9 title');
  check((await text(page, '.wz-no')) === 'Step 9 of 9', 'last step');
  await waitFor(async () => (await page.locator('[data-link=vrchat]').getAttribute('data-public')) === 'true', 'public link in the summary');
  await page.click('[data-copy-vrchat]');
  check((await page.evaluate(() => navigator.clipboard.readText())) === `${address}/live/main.m3u8`, 'Copy link for VRChat puts the channel link on the clipboard');
  check((await page.locator('[data-checklist] li').count()) === 3, 'a three-item test checklist');
  check((await page.locator('[data-settings]').count()) === 7, 'links to seven Settings sections');
  await shot(page, '9-summary');
  await page.click('[data-finish]');
  await page.waitForURL(`${base}/admin#/channel`);
  check((await sent('POST', '/api/desktop/wizard/complete')).length === 1, 'finishing completes the first run');
  check(page.errors.length === 0, 'no script errors', page.errors);
  await page.close();

  // ---------- Completed first run: /wizard goes on to the interface ----------
  page = await open();
  await page.waitForURL(`${base}/admin`);
  check(true, 'a finished wizard redirects to /admin');
  await page.close();

  // ---------- Run setup again from Settings → General ----------
  await reset();
  await scenario({ firstRun: false, tunnel: 'off', toolsMissing: [] });
  await call('/api/desktop/encoder/detect', {});
  const detectsBefore = (await sent('POST', '/api/desktop/encoder/detect')).length;
  page = await open({ path: '/admin#/settings/general' });
  await waitFor(() => has(page, '[data-run-wizard]'), 'Run setup again in Settings → General');
  check((await text(page, '[data-run-wizard]')) === 'Run setup again', 'the button text');
  await page.click('[data-run-wizard]');
  await page.waitForURL(`${base}/wizard?again=1`);
  await waitFor(() => has(page, '.wz-lang'), 'the wizard opens again');
  check((await heading(page)) === 'Setup', 'a run again has its own title');
  check((await text(page, '.wz-no')) === 'Step 1 of 8', 'no tools step when every tool is there');
  check(await has(page, '[data-to-end]'), 'Skip to the end from the first step on a run again');
  await next(page, 'picture');
  await waitFor(() => has(page, '.wz-result'), 'saved encoder result');
  check((await sent('POST', '/api/desktop/encoder/detect')).length === detectsBefore, 'a run again shows the saved result without a new test');
  await page.click('[data-retry]');
  await waitFor(async () => (await sent('POST', '/api/desktop/encoder/detect')).length === detectsBefore + 1, 'Test the card repeats the test');
  check(true, '"Test the card" tests again');
  for (const id of ['viewers', 'ticker', 'look', 'internet']) await next(page, id);
  await waitFor(() => has(page, '[data-tunnel=off]'), 'a run again keeps the tunnel off');
  await sleep(1200);
  check(!(await savedConfig('tunnelEnabled')), 'a run again does not switch the tunnel on by itself');
  await scenario({ tunnelOnEnable: 'online' });
  await page.click('[data-connect]');
  await waitFor(() => has(page, '[data-tunnel=online]'), 'Connect switches the tunnel on');
  check((await savedConfig('tunnelEnabled'))?.tunnelEnabled === true, 'Connect saves tunnelEnabled');
  await page.click('[data-to-end]');
  await waitFor(async () => (await step(page)) === 'summary', 'Skip to the end');
  check(true, 'Skip to the end opens the summary');
  await page.click('[data-settings=channel]');
  await page.waitForURL(`${base}/admin#/settings/channel`);
  check((await sent('POST', '/api/desktop/wizard/complete')).length === 1, 'a Settings link in the summary ends the wizard there');
  check(page.errors.length === 0, 'no script errors on a run again', page.errors);
  await page.close();

  // ---------- Summary without a public address ----------
  await reset();
  await scenario({ firstRun: false, tunnel: 'off', toolsMissing: [], lightProfile: true });
  page = await open({ path: '/wizard?again=1' });
  await waitFor(() => has(page, '[data-to-end]'), 'step 1');
  await next(page, 'picture');
  await waitFor(() => has(page, '[data-light-profile]'), 'light profile note');
  check(/Raspberry Pi/.test(await text(page, '[data-light-profile]')), 'a light computer gets the 720p note');
  await page.click('[data-to-end]');
  await waitFor(() => has(page, '[data-local]'), 'local note');
  check((await page.locator('[data-link=vrchat]').getAttribute('data-public')) === 'false', 'no public address: the local link');
  check(/^http:\/\/127\.0\.0\.1:\d+\/live\/main\.m3u8$/.test(await text(page, '[data-link=vrchat] code')), 'the local link is shown', await text(page, '[data-link=vrchat] code'));
  await page.close();

  // ---------- "I want a permanent link" opens Settings -> Internet ----------
  await reset();
  await scenario({ firstRun: true, tunnel: 'off', toolsMissing: [] });
  page = await open();
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'tools already installed');
  check((await sent('POST', '/api/desktop/tools/install')).length === 0, 'nothing is downloaded when the tools are there');
  await next(page, 'picture');
  await page.click('[data-to-end]');
  await waitFor(async () => (await step(page)) === 'summary', 'summary');
  await page.click('[data-back]');
  await waitFor(async () => (await step(page)) === 'extras', 'Back from the summary');
  await page.click('[data-back]');
  await waitFor(async () => (await step(page)) === 'internet', 'internet');
  await waitFor(() => has(page, '[data-permanent]'), 'online');
  await page.click('[data-permanent]');
  await page.waitForURL(`${base}/admin#/settings/internet`);
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
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'the download finishes (watched, not restarted)', 20000);
  check(true, 'the interrupted download is picked up');
  await page.close();

  // ---------- Failed download: retry, or skip to the next step ----------
  await reset();
  await scenario({ firstRun: true, language: 'en', toolsFail: true, toolsMissing: ['yt-dlp'] });
  page = await open();
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-retry]'), 'download failure', 20000);
  check(/The tools could not be downloaded/.test(await text(page, '.wz-note.err')), 'failure message');
  check(/Settings → Tools/.test(await text(page, '.wz-body > .wz-hint')), 'the failure says where to download later');
  await shot(page, '2-tools-failed');
  await page.click('[data-skip]');
  await waitFor(async () => (await step(page)) === 'picture', 'Skip goes on to the picture');
  check((await sent('POST', '/api/desktop/wizard/complete')).length === 0, 'skipping a failed download does not end the wizard');
  await page.click('[data-back]');
  await waitFor(async () => (await step(page)) === 'language', 'back');
  await scenario({ toolsFail: false });
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'the download works on a second visit', 20000);
  check(true, 'the tools step downloads again after a failure');
  await page.close();

  // ---------- Focus: each step focuses its heading; polls never take the focus ----------
  const activeIs = (page, sel) => page.evaluate((s) => document.activeElement === document.querySelector(s), sel);
  await reset();
  await scenario({ firstRun: true, tunnel: 'off', tunnelOnEnable: 'connecting', toolsMissing: [] });
  page = await open({ trackFocus: true });
  await waitFor(() => has(page, '.wz-lang'), 'step 1');
  check(await page.evaluate(() => document.activeElement?.tagName === 'H1'), 'step 1: the heading has the focus');
  await page.click('[data-next]');
  await waitFor(() => has(page, '[data-next]'), 'tools done');
  await waitFor(() => activeIs(page, '[data-next]'), 'Next is focused when the tools are done');
  for (const id of ['picture', 'viewers', 'ticker', 'look', 'internet']) await next(page, id);
  await waitFor(() => has(page, '[data-tunnel=connecting]'), 'connecting');
  await page.focus('[data-next]');
  await sleep(3000);
  check(await activeIs(page, '[data-next]'), 'polling does not take the focus from Skip (3 s)');
  const calls = await page.evaluate(() => window.__focus);
  check(calls.filter((c) => c === 'h1').length === 7, 'the heading is focused once per step', calls);
  await page.close();

  // ---------- A failed language save is shown ----------
  await reset();
  await scenario({ firstRun: true, language: 'pl' });
  page = await open();
  await waitFor(() => has(page, '.wz-lang'), 'step 1');
  await scenario({ configFail: true });
  await page.click('[data-lang=en]');
  await waitFor(() => has(page, '[role=alert]'), 'the failed save is shown');
  check((await heading(page)) === 'Witaj w VRCinema', 'the language did not change after the failed save');
  await page.close();

  // ---------- The engine answers late at the start ----------
  await reset();
  await scenario({ firstRun: true, infoFail: true });
  page = await open();
  await sleep(1200);
  await scenario({ infoFail: false });
  await waitFor(() => has(page, '.wz-lang'), 'the wizard appears once the engine answers', 12000);
  check(page.url() === `${base}/wizard`, 'no redirect to /admin while the engine is slow', page.url());
  await page.close();

  // ---------- Not the program's window: no wizard, and no "Run setup again" ----------
  await reset();
  await scenario({ firstRun: true, remote: true });
  page = await open({ path: '/wizard?again=1' });
  await page.waitForURL(`${base}/admin`);
  check(true, 'a remote session is sent to the interface, not the wizard');
  await page.goto(`${base}/admin#/settings/general`);
  await waitFor(() => has(page, '[data-panel-language]'), 'remote Settings → General');
  check(!(await has(page, '[data-run-wizard]')), 'a remote panel has no "Run setup again"');
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close();
  rmSync(profile, { recursive: true, force: true });
}
