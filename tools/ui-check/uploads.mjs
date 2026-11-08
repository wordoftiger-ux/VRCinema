// Browser check of video and audio files from viewers: the owner's switch and limits (Settings
// → Channel, audio length apart), the request page's file box (picker with audio types, the
// limits, the size checked before sending, progress, the nick and device key in headers, an
// MP3 queued, the engine's refusal shown), the phone width, and the
// "sent file" detail in the Channel view's queue — in headless Brave against the fake engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/uploads.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8203);
if (!shots) throw new Error('usage: uploads.mjs <shots dir> [port]');
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
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const scenario = (s) => call('/__test/scenario', s);
const requests = () => call('/__test/requests');
const uploads = () => call('/__test/uploads');
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

const profile = mkdtempSync(join(tmpdir(), 'brave-uploads-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1100, height: 760 },
  args: ['--disable-extensions', '--no-first-run'],
});

function watch(page) {
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 4\d\d/.test(m.text()) && errors.push(m.text()));
  return errors;
}

const SUBMIT = 'form:has(input[name=url]) .v-submit';

/** The first bytes of an MP4 file, `size` bytes in all. */
const mp4 = (size) => {
  const b = Buffer.alloc(size);
  b.write('\0\0\0\x20ftypisom', 0, 'latin1');
  return b;
};

try {
  // ---------- The request page while the owner has not allowed files ----------
  const page = await ctx.newPage();
  const errors = watch(page);
  await page.goto(`${base}/`);
  await page.waitForSelector('input[name=url]');
  check((await page.locator('.v-file').count()) === 0, 'files off by default: no file box');

  // ---------- Settings → Channel: the switch and the limits ----------
  const app = await ctx.newPage();
  await app.addInitScript(FAKE_TAURI);
  const appErrors = watch(app);
  await app.goto(`${base}/admin#/settings/channel`);
  await app.waitForSelector('#view[data-section="channel"]');
  const sw = 'button[role=switch][aria-label="Allow viewers to send small video and audio files"]';
  await app.waitForSelector(sw);
  check((await app.$eval(sw, (b) => b.getAttribute('aria-checked'))) === 'false', 'the switch is off by default');
  check(await app.$eval('form[data-form="viewer-uploads"] input[name=maxMb]', (i) => i.disabled), 'the limits wait for the switch');
  check((await app.locator('form[data-form="viewer-uploads"] + .set-sub').innerText()).includes('100 MB'), 'the tunnel limit is explained');
  let mark = (await requests()).length;
  await app.click(sw);
  await waitFor(async () => (await requests()).slice(mark).some((r) => r.method === 'PUT' && JSON.stringify(r.body) === '{"viewerUploads":true}'), 'the switch saves {viewerUploads: true}');
  await app.waitForFunction(() => !document.querySelector('form[data-form="viewer-uploads"] input[name=maxMb]').disabled);
  await app.fill('form[data-form="viewer-uploads"] input[name=maxMb]', '500');
  await app.evaluate(() => {
    const form = document.querySelector('form[data-form="viewer-uploads"]');
    form.noValidate = true; // the browser's own maximum would stop it first
    form.requestSubmit();
  });
  await app.waitForFunction(() => [...document.querySelectorAll('.set-note.err')].some((n) => n.textContent.includes('1–200 MB')));
  check(true, 'a file size above 200 MB is refused on the page');
  mark = (await requests()).length;
  await app.fill('form[data-form="viewer-uploads"] input[name=maxMb]', '1');
  await app.fill('form[data-form="viewer-uploads"] input[name=maxMin]', '5');
  await app.fill('form[data-form="viewer-uploads"] input[name=audioMaxMin]', '20');
  await app.click('form[data-form="viewer-uploads"] button[type=submit]');
  await waitFor(
    async () => (await requests()).slice(mark).some((r) => r.method === 'PUT' && JSON.stringify(r.body) === '{"viewerUploadMaxMb":1,"viewerUploadMaxS":300,"viewerUploadAudioMaxS":1200,"viewerUploadQuotaMb":2048}'),
    'the limits save in MB and seconds, audio apart',
  );
  await app.locator('form[data-form="viewer-uploads"]').scrollIntoViewIfNeeded();
  await app.screenshot({ path: join(shots, 'settings-uploads.png') });

  // ---------- The request page: the file box ----------
  await page.waitForSelector('.v-file', { timeout: 8000 });
  const box = page.locator('.v-file');
  await waitFor(async () => (await box.innerText()).includes('Up to 1 MB; videos up to 5 min, audio up to 20 min. Audio airs with a picture made from it.'), 'the limits are shown, audio apart');
  check((await box.innerText()).includes('Or send a video or audio file'), 'the box offers audio too', await box.innerText());
  const accept = await page.$eval('.v-file input[type=file]', (i) => i.accept.split(','));
  check(['audio/*', '.mp3', '.flac', '.ogg', '.oga', '.opus', '.m4a', '.aac', '.wav', 'video/*', '.mp4'].every((x) => accept.includes(x)), 'the picker takes audio files', accept);
  check((await page.locator(SUBMIT).isDisabled()), 'nothing to send yet');
  await page.setInputFiles('.v-file input[type=file]', { name: 'big.mp4', mimeType: 'video/mp4', buffer: mp4(1048577) });
  await page.waitForSelector('.v-file[data-file=chosen]');
  check((await page.locator(SUBMIT).innerText()) === 'Send file', 'the button sends the file');
  await page.click(SUBMIT);
  await page.waitForSelector('.v-error[data-code=upload_too_big]');
  check((await page.locator('.v-error').innerText()).includes('at most 1 MB'), 'a file over the limit is refused in the page');
  check((await uploads()).length === 0, 'and nothing was sent');

  await page.click('[data-action=file-remove]');
  await page.waitForSelector('.v-file[data-file=none]');
  await scenario({ uploadDelayMs: 700 });
  await page.setInputFiles('.v-file input[type=file]', { name: 'Mój klip.mp4', mimeType: 'video/mp4', buffer: mp4(200000) });
  await page.fill('input[name=nick]', 'Filmowiec');
  await page.click(SUBMIT);
  await waitFor(async () => /Sending… \d+%|Checking the file…/.test(await page.locator(SUBMIT).innerText()), 'the button shows the progress');
  await waitFor(async () => (await page.locator('.v-toast').count()) === 1 && (await page.locator('.v-toast').innerText()).includes('Mój klip'), 'the toast names the file');
  const sent = (await uploads())[0];
  check(sent?.size === 200000 && sent.name === 'Mój klip.mp4' && sent.nick === 'Filmowiec' && /^[0-9a-f]{32}$/.test(sent.key ?? '') && sent.lang === 'en', 'the raw file with its name, the nick and the device key', sent);
  check((await page.locator('.v-file[data-file=none]').count()) === 1, 'the box is empty again');
  await waitFor(async () => (await page.locator('.v-item').allInnerTexts()).some((t) => t.includes('Mój klip')), 'the file is in the queue on the page');

  await scenario({ uploadDelayMs: 0 });
  const mp3 = Buffer.alloc(4000);
  mp3.write('ID3\x04', 0, 'latin1');
  await page.setInputFiles('.v-file input[type=file]', { name: 'Piosenka.mp3', mimeType: 'audio/mpeg', buffer: mp3 });
  await page.click(SUBMIT);
  await waitFor(async () => (await page.locator('.v-item').allInnerTexts()).some((t) => t.includes('Piosenka')), 'an audio file is queued like a video');
  check((await uploads()).at(-1)?.name === 'Piosenka.mp3', 'the audio file was sent as it is');

  await page.setInputFiles('.v-file input[type=file]', { name: 'notes.mp4', mimeType: 'video/mp4', buffer: Buffer.from('hello, not a video') });
  await page.click(SUBMIT);
  await page.waitForSelector('.v-error[data-code=upload_not_video]');
  check((await page.locator('.v-error').innerText()).includes('isn’t a video or audio file'), 'the engine’s refusal is shown');
  await page.screenshot({ path: join(shots, 'viewer-upload.png'), fullPage: true });

  // Phone width: no horizontal scroll, controls large enough.
  await page.setViewportSize({ width: 320, height: 800 });
  check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1), 'no horizontal scroll at 320 px');
  const small = await page.evaluate(() =>
    [...document.querySelectorAll('.v-file button, .v-file label.btn')].filter((e) => e.getBoundingClientRect().height < 40).map((e) => e.outerHTML.slice(0, 60)),
  );
  check(small.length === 0, 'the file box controls are touch-sized', small);
  await page.screenshot({ path: join(shots, 'viewer-upload-320.png'), fullPage: true });
  check(errors.length === 0, 'no errors on the request page', errors);

  // ---------- The owner turns it off: the box goes ----------
  await app.click(sw);
  await page.waitForFunction(() => !document.querySelector('.v-file'), null, { timeout: 8000 });
  check(true, 'off again: the box goes away');

  // ---------- The Channel view: the detail of the queued file ----------
  await app.goto(`${base}/admin`);
  await app.waitForSelector('.qi');
  const rows = await app.locator('.qi .qi-sub').allInnerTexts();
  check(rows.some((r) => r.includes('sent file') && r.includes('Filmowiec')), 'the queue says "sent file" with the nick', rows);
  check(appErrors.length === 0, 'no errors in the panel', appErrors);

  console.log(`\n${passed} checks passed`);
} catch (e) {
  console.error(e.message);
  process.exitCode = 1;
} finally {
  await ctx.close();
  engine.close?.();
  rmSync(profile, { recursive: true, force: true });
  process.exit(process.exitCode ?? 0);
}
