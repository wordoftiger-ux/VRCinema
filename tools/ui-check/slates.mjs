// Browser check of Settings → Channel → Slates (spec 2026-11-07): the last-frame style saves
// `slateFrameStyle`; in the program's window the program's own picker stores the file (no web
// file field: the window once went blank behind the web view's chooser) and the view still
// shows with the new preview; a cancelled pick changes nothing; "Restore default" removes it;
// a remote panel uploads a picked file as the raw body; a section that fails to draw shows a
// message with "Try again" (the rail still works); the switch saves `slateBackgroundAll`, the
// texts exist in Polish too, against the fake engine, in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/slates.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8198);
if (!shots) throw new Error('usage: slates.mjs <shots dir> [port]');
mkdirSync(shots, { recursive: true });
const pw = process.env.PLAYWRIGHT_CORE ? pathToFileURL(process.env.PLAYWRIGHT_CORE).href : 'playwright-core';
const { chromium } = await import(pw);

let passed = 0;
function check(cond, what, detail) {
  if (!cond) throw new Error(`FAILED: ${what}${detail === undefined ? '' : ` - ${JSON.stringify(detail)}`}`);
  passed += 1;
  console.log(`ok - ${what}`);
}
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

const engine = await startFakeEngine(port);
const base = `http://127.0.0.1:${port}`;
const profile = mkdtempSync(join(tmpdir(), 'brave-ui-check-'));
const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: BRAVE,
  headless: true,
  locale: 'en-US',
  viewport: { width: 1100, height: 760 },
  args: ['--disable-extensions', '--no-first-run'],
});
const call = (path, body) =>
  fetch(base + path, { method: body === undefined ? 'GET' : 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).then((r) => r.json());
const requests = () => call('/__test/requests');
const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};
// A 1×1 PNG.
const PNG = Buffer.from('89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360f8cf00000301010018dd8db00000000049454e44ae426082', 'hex');
const pngFile = join(profile, 'bg.png');
writeFileSync(pngFile, PNG);

try {
  await call('/__test/reset', {});
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/admin#/settings/channel`);
  await page.waitForSelector('#view[data-section="channel"] .slate-bg');
  const view = () => page.$eval('#view', (e) => e.innerText);
  check((await view()).includes('Last frame of the video'), 'the slates block is there');
  check((await view()).includes('Default background'), 'without a background the default is shown');

  // The style of the last frame.
  await page.selectOption('select[aria-label="Last frame of the video"]', 'blur');
  const style = await waitFor(async () => (await requests()).find((r) => r.method === 'PUT' && r.body?.slateFrameStyle), 'style saved');
  check(style.body.slateFrameStyle === 'blur', 'the style is saved', style.body);

  // The window: the program's own picker (the engine stores the file); the view still shows,
  // with the new preview, and nothing failed to draw.
  check(!(await page.$('.slate-bg input[type=file]')), 'the window has no web file field');
  await page.click('button[data-action="slate-bg-pick"]');
  await waitFor(async () => (await requests()).find((r) => r.method === 'POST' && r.path === '/api/desktop/slate-background/pick'), 'native pick');
  await page.waitForSelector('img.slate-bg-preview');
  await waitFor(() => page.$eval('img.slate-bg-preview', (i) => i.complete && i.naturalWidth === 1), 'the preview loads');
  check(true, 'the preview of the picked file loads');
  check((await view()).includes('Background saved.'), 'the owner is told', await view());
  const after = await page.evaluate(() => ({
    crash: Boolean(document.querySelector('[data-crash]')),
    rail: Boolean(document.querySelector('.rail')),
    blocks: document.querySelectorAll('#view[data-section="channel"] .set-rows > *').length,
    text: document.getElementById('root').innerText.length,
  }));
  check(!after.crash && after.rail && after.blocks > 5 && after.text > 500, 'after the pick the whole view still renders', after);
  await page.$eval('.slate-bg', (e) => e.scrollIntoView({ block: 'center' }));
  await page.screenshot({ path: join(shots, 'slates-channel-en.png') });
  await page.click('button[data-action="slate-bg-remove"]');
  await page.waitForSelector('.slate-bg-empty');
  check((await view()).includes('Default background restored.'), 'restore default');
  // A cancelled pick changes nothing.
  await call('/__test/scenario', { pickSlate: false });
  const picks = async () => (await requests()).filter((r) => r.path === '/api/desktop/slate-background/pick').length;
  await page.click('button[data-action="slate-bg-pick"]');
  await waitFor(async () => (await picks()) === 2, 'second pick');
  await sleep(300);
  check(Boolean(await page.$('.slate-bg-empty')) && !(await page.$('.slate-bg[data-busy="true"]')), 'cancelled: still the default, not busy');
  await call('/__test/scenario', { pickSlate: true });

  // A remote panel (no window): a picked file is sent as the raw body; the preview appears.
  await call('/__test/scenario', { remote: true });
  const remote = await ctx.newPage();
  remote.on('pageerror', (e) => errors.push(String(e)));
  await remote.goto(`${base}/admin#/settings/channel`);
  await remote.waitForSelector('#view[data-section="channel"] .slate-bg input[type=file]', { state: 'attached' });
  await remote.setInputFiles('.slate-bg input[type=file]', pngFile);
  const put = await waitFor(async () => (await requests()).find((r) => r.method === 'PUT' && r.path.endsWith('/slate-background')), 'upload');
  check(put.body.bytes === PNG.length && put.body.type === 'image/png', 'the file goes as it is', put.body);
  await remote.waitForSelector('img.slate-bg-preview');
  check(Boolean(await remote.$('#view[data-section="channel"] .set-rows')) && !(await remote.$('[data-crash]')), 'the remote panel still shows the view after the upload');
  await remote.click('button[data-action="slate-bg-remove"]');
  await remote.waitForSelector('.slate-bg-empty');
  await remote.close();
  await call('/__test/scenario', { remote: false });

  // A section that fails to draw: a message with "Try again" instead of a frozen window.
  await page.evaluate(async () => {
    (await import('/static/app/views/settings/common.js')).channelSettings.value = { linkMode: 'allowlist' };
  });
  await page.waitForSelector('[data-crash]');
  check(Boolean(await page.$('.rail')) && Boolean(await page.$('.snav')), 'the rail and the sections stay usable', await page.$eval('#root', (e) => e.innerText.slice(0, 200)));
  await page.screenshot({ path: join(shots, 'slates-crash-en.png') });
  await page.evaluate(async () => (await import('/static/app/views/settings/common.js')).loadChannelSettings());
  await page.click('[data-crash-retry]');
  await page.waitForSelector('#view[data-section="channel"] .slate-bg');
  check(!(await page.$('[data-crash]')), '"Try again" draws the section again');

  // The switch for all slates.
  await page.click('button[role=switch][aria-label="Behind all slates"]');
  await waitFor(async () => (await requests()).find((r) => r.method === 'PUT' && r.body?.slateBackgroundAll === true), 'switch saved');
  check(true, 'the switch saves slateBackgroundAll');

  // Polish texts, no raw keys, no horizontal overflow on a narrow window.
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await waitFor(async () => (await view()).includes('Ostatnia klatka filmu'), 'Polish texts');
  check(!/settings\.channel\.slate/.test(await view()), 'no raw keys');
  await page.setViewportSize({ width: 560, height: 760 });
  check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1), 'no overflow at 560 px');
  await page.screenshot({ path: join(shots, 'slates-channel-pl-narrow.png'), fullPage: false });
  check(errors.length === 0, 'no page errors', errors);
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
}
process.exit(0);
