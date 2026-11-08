// Browser check of Settings → Channel → VRChat chatbox (spec 2026-11-08-ctl-osc): the switch saves
// `oscEnabled`, the address form checks and saves `oscHost`/`oscPort`, the text saves
// `oscTemplate`, the select saves `oscRepeatS`, the texts exist in Polish, no overflow on a narrow
// window; against the fake engine, in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/osc.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: osc.mjs <shots dir> [port]');
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
const put = (pred, what) => waitFor(async () => (await requests()).find((r) => r.method === 'PUT' && r.path.endsWith('/settings') && pred(r.body ?? {})), what);
const FAKE_TAURI = () => {
  window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
};

try {
  await call('/__test/reset', {});
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/admin#/settings/channel`);
  await page.waitForSelector('#view[data-section="channel"] [data-block="osc"]');
  const block = () => page.$eval('[data-block="osc"]', (e) => e.closest('section').innerText);
  check((await block()).includes('VRChat chatbox'), 'the VRChat block is there');
  check((await block()).includes('Action Menu → Options → OSC → Enabled'), 'it says where OSC is switched on');
  check((await page.inputValue('input[name=oscHost]')) === '127.0.0.1', 'the default address');
  check((await page.inputValue('input[name=oscPort]')) === '9000', 'the default port');

  await page.click('button[role=switch][aria-label="Show the video in the chatbox"]');
  await put((b) => b.oscEnabled === true, 'switch saved');
  check((await block()).includes('The chatbox shows the video on air.'), 'the switch is confirmed');

  // A bad address is refused in the page; a good one is saved.
  await page.fill('input[name=oscHost]', 'bad host!');
  await page.click('form[data-form="osc-target"] button[type=submit]');
  await waitFor(async () => (await block()).includes('Enter an IP address or a host name'), 'bad address refused');
  check(!(await requests()).some((r) => r.method === 'PUT' && r.body?.oscHost === 'bad host!'), 'nothing sent for a bad address');
  await page.fill('input[name=oscHost]', '192.168.1.20');
  await page.fill('input[name=oscPort]', '9001');
  await page.click('form[data-form="osc-target"] button[type=submit]');
  const target = await put((b) => b.oscHost === '192.168.1.20', 'address saved');
  check(target.body.oscPort === 9001, 'the port goes as a number', target.body);

  await page.fill('input[name=oscTemplate]', 'Now: {title} ({nick})');
  await page.click('form[data-form="osc-template"] button[type=submit]');
  await put((b) => b.oscTemplate === 'Now: {title} ({nick})', 'template saved');
  check((await block()).includes('Text saved.'), 'the text is confirmed');

  await page.selectOption('select[aria-label="Send again"]', '30');
  await put((b) => b.oscRepeatS === 30, 'repeat saved');
  await page.$eval('[data-block="osc"]', (e) => e.closest('section').scrollIntoView({ block: 'start' }));
  await page.screenshot({ path: join(shots, 'osc-channel-en.png') });

  // Polish texts, no raw keys, no horizontal overflow on a narrow window.
  await page.evaluate(async () => (await import('/static/app/store.js')).setLanguage('pl'));
  await waitFor(async () => (await block()).includes('Chatbox VRChat'), 'Polish texts');
  check(!/settings\.channel\.osc/.test(await block()), 'no raw keys');
  check((await block()).includes('Co 30 s'), 'the select in Polish');
  await page.setViewportSize({ width: 560, height: 760 });
  check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1), 'no overflow at 560 px');
  await page.$eval('[data-block="osc"]', (e) => e.closest('section').scrollIntoView({ block: 'start' }));
  await page.screenshot({ path: join(shots, 'osc-channel-pl-narrow.png') });
  check(errors.length === 0, 'no page errors', errors);
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  await engine.close?.();
  rmSync(profile, { recursive: true, force: true });
}
process.exit(0);
