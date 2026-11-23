// Browser check of the request page's layout B: the three tabs on a phone (390 and 320 px: no
// horizontal scroll), the queue rows with pictures, "mine" and "in ~" times, "Your requests",
// the skip-vote card in each mode (signal, count, percent, percent with nobody watching, off),
// the Add tab still sending a link and a title request, and at 1280 px the two columns with the
// "Add a film" drawer (Esc, close button and backdrop close it) — headless Brave, fake engine.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/viewer-b.mjs <shots dir> [port]

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: viewer-b.mjs <shots dir> [port]');
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
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const posts = async (path) => (await call('/__test/requests')).filter((r) => r.method === 'POST' && r.path === path);

async function waitFor(fn, what, timeout = 6000) {
  const start = Date.now();
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() - start > timeout) throw new Error(`FAILED (timeout): ${what}`);
    await sleep(50);
  }
}

const profiles = [];
const contexts = [];
async function open(viewport, locale = 'en-US') {
  const profile = mkdtempSync(join(tmpdir(), 'brave-viewer-b-'));
  profiles.push(profile);
  const ctx = await chromium.launchPersistentContext(profile, {
    executablePath: BRAVE,
    headless: true,
    locale,
    viewport,
    args: ['--disable-extensions', '--no-first-run'],
  });
  contexts.push(ctx);
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !m.text().includes('status of 4') && errors.push(m.text()));
  await page.goto(`${base}/`);
  await page.waitForSelector('.v-now');
  return { page, errors, ctx };
}

const noScroll = (page) => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth <= 0);
const text = (page, sel) => page.locator(sel).first().innerText();
const shot = (page, name) => page.screenshot({ path: join(shots, `${name}.png`), fullPage: false });

try {
  // ---------- Phone, 390 px ----------
  let { page, errors } = await open({ width: 390, height: 800 });
  check((await page.locator('.v-tabs .v-tab').count()) === 3, 'three tabs at the bottom');
  check((await page.locator('.v-tab[aria-current=page]').getAttribute('data-tab')) === 'now', 'Now is the first tab');
  check((await page.locator('[data-action=add-film]').count()) === 0, 'no "Add a film" header button on a phone');
  check((await page.locator('.v-tab[data-tab=queue] .v-badge').innerText()) === '4', 'the Queue tab has a count badge');
  check((await text(page, '.v-now-title')) === 'Never Gonna Give You Up', 'Now: the title');
  check((await page.locator('.v-hero img').count()) === 1, 'Now: the big picture is the thumbnail');
  check((await text(page, '.v-upnext')).includes('in ~'), 'Now: "Up next" has the first queued film with its start', await text(page, '.v-upnext'));
  check(await noScroll(page), 'no horizontal scroll at 390 px (Now)');
  await shot(page, 'viewer-b-phone-now');

  // Tabs switch, and they are real buttons (reachable with Tab).
  await page.click('.v-tab[data-tab=queue]');
  await page.waitForSelector('.v-queue');
  check((await page.locator('.v-now').count()) === 0, 'the Now view is gone on the Queue tab');
  const rows = page.locator('.v-queue .v-item');
  check((await rows.count()) === 4, 'Queue: four rows');
  check((await page.locator('.v-queue .v-item .v-pic img').count()) === 2, 'Queue: the two films with a public picture show it (others a gradient)');
  const etas = await page.locator('.v-queue .v-eta').allInnerTexts();
  check(etas[0] === 'in ~1 min' || etas[0].startsWith('in ~'), 'Queue: the first start is "in ~..."', etas);
  check(etas.length === 3 && /^in ~1 h \d+ min$/.test(etas[2]) && (await rows.nth(3).locator('.v-eta').count()) === 0, 'Queue: after a live stream no start is shown', etas);
  check((await rows.nth(0).getAttribute('data-mine')) === 'yes' && (await rows.nth(0).innerText()).includes('Yours'), 'Queue: my item is marked');
  check((await rows.nth(1).getAttribute('data-mine')) === 'no', 'Queue: someone else\'s is not');
  await shot(page, 'viewer-b-phone-queue');
  await page.focus('.v-tab[data-tab=now]');
  await page.keyboard.press('Tab');
  check(await page.evaluate(() => document.activeElement?.dataset?.tab === 'queue'), 'the tabs are reachable with the keyboard');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Tab');
  await page.keyboard.press('Enter');
  await page.waitForSelector('.v-add');
  check(await noScroll(page), 'no horizontal scroll at 390 px (Add)');
  await shot(page, 'viewer-b-phone-add');

  // ---------- Add: a link and a title request still go out ----------
  await page.fill('input[name=url]', 'https://www.youtube.com/watch?v=abc');
  await page.fill('input[name=nick]', 'Ola');
  await page.click('.v-submit');
  await waitFor(async () => (await posts('/api/channels/main/requests')).length === 1, 'the link request was sent');
  check((await posts('/api/channels/main/requests'))[0].body.url === 'https://www.youtube.com/watch?v=abc', 'the link is sent');
  await page.waitForSelector('.v-queue');
  check((await page.locator('.v-queue .v-item').count()) === 5, 'after a request the page shows the queue with the new film');
  check((await page.locator('.v-queue .v-item[data-mine=yes]').count()) === 2, 'both my films are marked');
  await page.click('.v-tab[data-tab=add]');
  await page.click('[data-mode=film]');
  await page.fill('input[type=search], input[name=q]', 'Example');
  await page.waitForSelector('.v-film-row');
  await page.locator('.v-film-row').first().click();
  await page.click('.v-submit');
  await waitFor(async () => (await posts('/api/channels/main/requests')).some((r) => r.body.title), 'the title request was sent');
  await page.waitForSelector('.v-requests');
  check((await page.locator('.v-requests .v-item').count()) === 1, '"Your requests" shows the title waiting for approval');
  check((await text(page, '.v-requests')).includes('waiting for approval'), 'it says so');
  check((await page.locator('.v-requests .v-pic img').count()) === 1, 'with the poster as the picture');
  await shot(page, 'viewer-b-phone-requests');

  // ---------- The skip-vote card in each mode ----------
  await page.click('.v-tab[data-tab=now]');
  await page.waitForSelector('.v-skip');
  check((await text(page, '.v-skip-count')) === 'Votes to skip: 1', 'signal mode: votes only', await text(page, '.v-skip-count'));
  check((await page.locator('.v-skip .v-bar').count()) === 0, 'signal mode: no bar');
  await page.click('.v-skip-btn');
  await waitFor(async () => (await text(page, '.v-skip-count')) === 'Votes to skip: 2', 'my vote counts');
  check((await page.locator('.v-skip-btn').getAttribute('aria-pressed')) === 'true' && (await text(page, '.v-skip-btn')) === 'Take back my vote', 'the button now takes the vote back');
  const sent = (await posts('/api/channels/main/skip-vote'))[0].body;
  check(sent.vote === true && sent.itemId === 7 && /^[0-9a-f]{32}$/.test(sent.deviceKey), 'the vote carries the film and the device key', sent);
  await shot(page, 'viewer-b-phone-vote');
  await page.click('.v-skip-btn');
  await waitFor(async () => (await text(page, '.v-skip-count')) === 'Votes to skip: 1', 'withdrawn');
  check((await posts('/api/channels/main/skip-vote')).at(-1).body.vote === false, 'a second press withdraws');
  await scenario({ settingsPatch: { skipVoteMode: 'count', skipVoteCount: 3 } });
  await waitFor(async () => (await text(page, '.v-skip-count')) === '1 of 3 votes', 'count mode shows votes of needed');
  check((await page.locator('.v-skip .v-bar span').evaluate((e) => e.style.width)) === '33.3333%', 'count mode: the bar');
  await scenario({ settingsPatch: { skipVoteMode: 'percent', skipVotePercent: 50 }, skipWatching: 4 });
  await waitFor(async () => (await text(page, '.v-skip-count')) === '1 of 2 votes', 'percent mode: needed from the viewers watching');
  await scenario({ skipWatching: 0 });
  await waitFor(async () => (await text(page, '.v-skip-count')) === 'Votes to skip: 1', 'percent mode with nobody watching: votes only');
  await scenario({ settingsPatch: { skipVoteMode: 'off' } });
  await waitFor(async () => (await page.locator('.v-skip').count()) === 0, 'off: no card');
  await scenario({ settingsPatch: { skipVoteMode: 'signal' }, skipWatching: 4 });
  await waitFor(async () => (await page.locator('.v-skip').count()) === 1, 'the card is back');
  check(errors.length === 0, 'no console errors (phone)', errors);
  await page.close();

  // ---------- Phone, 320 px, Polish ----------
  ({ page, errors } = await open({ width: 320, height: 700 }, 'pl-PL'));
  for (const id of ['now', 'queue', 'add']) {
    await page.click(`.v-tab[data-tab=${id}]`);
    await page.waitForTimeout(150);
    check(await noScroll(page), `no horizontal scroll at 320 px (${id}, pl)`);
    await shot(page, `viewer-b-320-${id}`);
  }
  check((await text(page, '.v-tab[data-tab=queue]')).startsWith('Kolejka'), 'Polish tab names');
  check(errors.length === 0, 'no console errors (320 pl)', errors);
  await page.close();

  // ---------- Wide screen, 1280 px ----------
  ({ page, errors } = await open({ width: 1280, height: 800 }));
  check((await page.locator('.v-tabs').count()) === 0, 'wide: no tab bar');
  const cols = await page.evaluate(() => {
    const a = document.querySelector('.v-col-now').getBoundingClientRect();
    const b = document.querySelector('.v-col-queue').getBoundingClientRect();
    return { aLeft: a.left, bLeft: b.left, aTop: a.top, bTop: b.top };
  });
  check(cols.bLeft > cols.aLeft + 300 && Math.abs(cols.aTop - cols.bTop) < 40, 'wide: Now on the left, the queue on the right', cols);
  check((await page.locator('.v-col-now .v-now').count()) === 1 && (await page.locator('.v-col-queue .v-item').count()) >= 4, 'wide: now playing and the queue at once');
  check((await page.locator('.v-add').count()) === 0, 'wide: the form is not on the page until asked');
  await shot(page, 'viewer-b-wide');
  await page.click('[data-action=add-film]');
  await page.waitForSelector('.v-drawer[role=dialog]');
  await page.waitForFunction(() => document.activeElement?.closest('.v-drawer') !== null, null, { timeout: 2000 }).catch(() => {});
  check(await page.evaluate(() => document.activeElement?.closest('.v-drawer') !== null), 'the drawer takes the focus');
  await shot(page, 'viewer-b-wide-drawer');
  // Tab stays inside the drawer.
  for (let i = 0; i < 30; i += 1) await page.keyboard.press('Tab');
  check(await page.evaluate(() => document.activeElement?.closest('.v-drawer') !== null), 'Tab stays inside the drawer');
  await page.keyboard.press('Escape');
  await waitFor(async () => (await page.locator('.v-drawer').count()) === 0, 'Esc closes the drawer');
  check(await page.evaluate(() => document.activeElement?.dataset?.action === 'add-film'), 'the focus goes back to the button');
  await page.click('[data-action=add-film]');
  await page.waitForSelector('.v-drawer');
  await page.click('[data-action=drawer-close]');
  await waitFor(async () => (await page.locator('.v-drawer').count()) === 0, 'the close button closes it');
  await page.click('[data-action=add-film]');
  await page.waitForSelector('.v-drawer');
  await page.mouse.click(60, 400);
  await waitFor(async () => (await page.locator('.v-drawer').count()) === 0, 'a click on the backdrop closes it');
  // A request from the drawer closes it.
  await page.click('[data-action=add-film]');
  await page.fill('.v-drawer input[name=url]', 'https://www.youtube.com/watch?v=wide');
  await page.click('.v-drawer .v-submit');
  await waitFor(async () => (await page.locator('.v-drawer').count()) === 0, 'a sent request closes the drawer');
  check(await noScroll(page), 'no horizontal scroll at 1280 px');
  check(errors.length === 0, 'no console errors (wide)', errors);
  await page.close();

  // ---------- The window: the counter next to the film on air and the vote settings ----------
  {
    const profile = mkdtempSync(join(tmpdir(), 'brave-viewer-b-'));
    profiles.push(profile);
    const ctx = await chromium.launchPersistentContext(profile, { executablePath: BRAVE, headless: true, locale: 'en-US', viewport: { width: 1200, height: 800 }, args: ['--disable-extensions', '--no-first-run'] });
    contexts.push(ctx);
    const app = await ctx.newPage();
    const appErrors = [];
    app.on('pageerror', (e) => appErrors.push(String(e)));
    await app.addInitScript(() => {
      window.__TAURI_INTERNALS__ = { invoke: (cmd) => Promise.resolve(cmd === 'plugin:window|is_maximized' ? false : null) };
    });
    await scenario({ settingsPatch: { skipVoteMode: 'count', skipVoteCount: 4 }, skipOthers: 2, skipMine: false });
    await app.goto(`${base}/admin`);
    await waitFor(async () => (await app.locator('.nowp-sub').first().innerText().catch(() => '')).includes('Skip votes: 2/4'), 'the player bar shows "Skip votes: 2/4"');
    await scenario({ settingsPatch: { skipVoteMode: 'signal' } });
    await waitFor(async () => (await app.locator('.nowp-sub').first().innerText().catch(() => '')).includes('Skip votes: 2') && !(await app.locator('.nowp-sub').first().innerText()).includes('/'), 'signal mode: "Skip votes: 2"', await app.locator('.nowp-sub').first().innerText());
    await app.goto(`${base}/admin#/settings/channel`);
    await app.waitForSelector('select[name=skipVoteMode]');
    check((await app.locator('select[name=skipVoteMode] option').count()) === 4, 'Settings: four modes in the select');
    check((await app.locator('input[name=skipVotePercent]').isDisabled()) && (await app.locator('input[name=skipVoteCount]').isDisabled()), 'Settings: limits wait for a mode that uses them');
    await app.selectOption('select[name=skipVoteMode]', 'percent');
    await waitFor(async () => !(await app.locator('input[name=skipVotePercent]').isDisabled()), 'percent field enabled');
    await app.fill('input[name=skipVotePercent]', '60');
    await app.click('form[data-form=skip-vote] button[type=submit]');
    await waitFor(async () => (await call('/__test/requests')).some((r) => r.method === 'PUT' && r.body?.skipVotePercent === 60), 'the percent is saved');
    check((await call('/__test/requests')).some((r) => r.method === 'PUT' && r.body?.skipVoteMode === 'percent'), 'the mode is saved');
    await app.fill('input[name=skipVotePercent]', '0');
    await app.click('form[data-form=skip-vote] button[type=submit]');
    await app.waitForSelector('.set-note.err, [role=alert]');
    check(appErrors.length === 0, 'no errors in the window', appErrors);
  }

  console.log(`\n${passed} checks passed`);
} finally {
  for (const c of contexts) await c.close().catch(() => {});
  await engine.close();
  for (const p of profiles) rmSync(p, { recursive: true, force: true });
}
process.exit(0);
