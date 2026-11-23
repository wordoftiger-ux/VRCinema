// Browser check of the addons in the window: the Search tab (results, a series with its seasons and
// episodes, the sources of every addon asked on its own, Add to queue / Play next), "Choose source" on a
// viewer's title request in the approvals, Settings -> Addons (list, switch, add, order, remove) and the
// viewer-catalog switch in Settings -> Channel; the rail on a phone with five tabs. Against the fake
// engine, in headless Brave with a temporary profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/addons.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';
import { showAdd } from './viewer-nav.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: addons.mjs <shots dir> [port]');
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
  viewport: { width: 1100, height: 760 },
  args: ['--disable-extensions', '--no-first-run'],
});

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
const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const texts = (page, sel) => page.$$eval(sel, (els) => els.map((e) => e.textContent.trim()));
const count = (page, sel) => page.$$eval(sel, (els) => els.length);
const hash = (page) => page.evaluate(() => location.hash);
const toast = (page, prefix) =>
  page.waitForFunction((p) => document.querySelector('.toast span')?.textContent.startsWith(p), prefix, { timeout: 5000 }).then(() => text(page, '.toast span'));

async function openApp() {
  const page = await ctx.newPage();
  await page.addInitScript(FAKE_TAURI);
  // The addons' pictures are unreachable (a made-up host): they fail at once, like a dead link.
  await page.route('https://addons.example/**', (r) => r.abort());
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 4\d\d|status of 5\d\d|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  await page.goto(`${base}/admin`);
  await page.waitForSelector('.qi');
  page.__errors = errors;
  return page;
}

try {
  await reset();
  await scenario({ pending: 2, titleRequest: true });
  const page = await openApp();

  // ---------- The rail ----------
  check(
    JSON.stringify((await texts(page, '.rail-label'))) === JSON.stringify(['Channel', 'Search', 'History', 'Share', 'Settings']),
    'the rail shows Channel, Search, History, Share, Settings',
    await texts(page, '.rail-label'),
  );

  // ---------- A viewer's title request in the approvals (before anything else changes) ----------
  await page.waitForSelector('.qi-request');
  check((await text(page, '.qi-request .choose-source')) === 'Choose source', 'a title request shows "Choose source"');
  check((await count(page, '.qi-request .act-button.yes')) === 0, 'and no Approve button');
  check((await count(page, '.qi-request .act-button.no')) === 1, 'Reject stays');
  check((await count(page, '.qi-pending:not(.qi-request) .act-button.yes')) >= 1, 'ordinary waiting videos keep Approve');
  await waitFor(async () => (await count(page, '.qi-request .poster-none')) === 1, 'the poster of the request is a neutral stand-in (its picture does not load)');
  check(true, 'the poster that does not load is a neutral stand-in');
  const itemId = await page.$eval('.qi-request', (e) => e.dataset.id);
  await page.screenshot({ path: join(shots, 'addons-approvals-en.png') });

  // ---------- The link field: quick title search ----------
  let m = await mark();
  await page.click('.addf-input');
  await page.keyboard.type('exa');
  await page.waitForSelector('.addf-opt');
  await waitFor(async () => (await count(page, '.addf-opt')) === 2, 'the dropdown lists the titles');
  check((await count(page, '.addf-opt')) <= 6, 'the dropdown shows at most 6 rows');
  check((await since(m)).filter((r) => r.path === '/api/admin/addons/search').length === 1, 'one search request after the pause, not one per letter');
  check((await page.getAttribute('.addf-input', 'role')) === 'combobox' && (await count(page, '[role="listbox"] [role="option"]')) === 2, 'combobox + listbox + options');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  check((await page.getAttribute('.addf-input', 'aria-activedescendant')) === 'addf-opt-1', 'aria-activedescendant follows the arrows');
  await page.screenshot({ path: join(shots, 'addons-linkfield-en.png') });
  await page.keyboard.press('Escape');
  check((await count(page, '.addf-opt')) === 0, 'Esc closes the dropdown');
  await page.fill('.addf-input', '');
  await page.keyboard.type('zzz');
  await page.waitForSelector('.addf-none');
  check((await text(page, '.addf-none')) === 'No titles found.', 'text with no match says so');
  await page.fill('.addf-input', '');
  await page.keyboard.type('exa');
  await page.waitForSelector('.addf-opt');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await waitFor(async () => (await hash(page)) === '#/search/series/tt2', 'Enter on a highlighted row opens the title');
  await page.evaluate(() => (location.hash = '#/channel'));
  await page.waitForSelector('.addf-input');
  check((await page.inputValue('.addf-input')) === '', 'the field was cleared');
  await page.click('.addf-input');
  m = await mark();
  await page.keyboard.type('https://x');
  await sleep(700);
  check((await count(page, '.addf-list')) === 0, 'a link shows no dropdown');
  check((await since(m)).filter((r) => r.path === '/api/admin/addons/search').length === 0, 'and asks no addon');
  await page.keyboard.press('Enter');
  await waitFor(async () => (await since(m)).some((r) => r.method === 'POST' && r.path.endsWith('/items')), 'Enter on a link still adds it');

  // ---------- Search ----------
  await page.click('.rail-search');
  await page.waitForSelector('.sr-input');
  check(await page.evaluate(() => document.activeElement?.classList.contains('sr-input')), 'the search field has the focus');
  check((await text(page, '.sr-hint')) === 'Type at least two letters.', 'a hint asks for two letters');
  m = await mark();
  await page.keyboard.type('exa');
  await page.waitForSelector('.sr-card');
  await waitFor(async () => (await count(page, '.sr-card')) === 2, 'two results');
  check(true, 'searching "exa" shows 2 posters');
  const get = (await since(m)).filter((r) => r.path === '/api/admin/addons/search');
  check(get.length === 1 && get[0].search === '?q=exa', 'one search request, after the pause, for "exa"', get);
  check(JSON.stringify(await texts(page, '.sr-name')) === '["Example Film","Example Show"]', 'the titles', await texts(page, '.sr-name'));
  check(
    (await page.$$eval('.sr-card img', (imgs) => imgs.every((i) => i.getAttribute('referrerpolicy') === 'no-referrer' && i.getAttribute('loading') === 'lazy'))),
    'posters are lazy and sent without a referrer',
  );
  await waitFor(async () => (await count(page, '.sr-card .poster-none')) === 2, 'the poster that fails is replaced by a placeholder');
  check(true, 'a poster that fails (or is missing) shows the placeholder');
  check(JSON.stringify(await texts(page, '.sr-type')) === '["Film","Series"]', 'type badges', await texts(page, '.sr-type'));
  await page.screenshot({ path: join(shots, 'addons-search-en.png') });

  // ---------- A series: seasons, episodes, the sources of each addon ----------
  await page.click('.sr-card[data-type="series"]');
  await page.waitForSelector('.ep-season');
  check((await hash(page)) === '#/search/series/tt2', 'the title page has its own address', await hash(page));
  check((await text(page, '.tp-name')) === 'Example Show', 'the title page shows the name');
  check(JSON.stringify(await texts(page, '.ep-season option')) === '["Season 1","Season 2"]', 'the season select', await texts(page, '.ep-season option'));
  check((await count(page, '.ep-row')) === 2, 'season 1 lists its 2 episodes');
  check((await count(page, '.src-addon')) === 0 && (await count(page, '.tp-pick')) === 1, 'no sources until an episode is picked');
  m = await mark();
  await page.click('.ep-row[data-episode="tt2:1:2"]');
  await waitFor(async () => (await count(page, '.src-addon')) === 3, 'three addons are listed');
  check(true, 'sources are listed per addon');
  check(await page.$eval('.src-addon[data-addon="3"]', (e) => e.dataset.state === 'loading'), 'a slow addon shows its spinner while the others have answered or fail');
  await waitFor(async () => (await count(page, '.src-addon[data-state="loading"]')) === 0, 'all addons answered');
  check((await count(page, '.src-row')) === 2, 'two addons answered with a source each');
  check((await texts(page, '.src-fail')).join('|') === 'Example Streams Broken did not answer.', 'one muted failure line names the addon', await texts(page, '.src-fail'));
  check((await count(page, '.src-hidden')) === 1 && (await page.$eval('.src-hidden a', (a) => a.getAttribute('href'))) === '#/settings/addons', 'torrents off: one line says how many torrent sources are hidden, with a link to the setting', await texts(page, '.src-hidden'));
  const asked = (await since(m)).filter((r) => r.path === '/api/admin/addons/streams/series/tt2%3A1%3A2' || r.path.startsWith('/api/admin/addons/streams/series/tt2'));
  check(asked.filter((r) => r.search).length === 3 || asked.length >= 4, 'each addon was asked on its own request', asked.map((r) => r.path + r.search));
  check((await page.$eval('.src-addon[data-addon="2"] .src-name', (e) => getComputedStyle(e).whiteSpace)) === 'pre-line', 'multi-line names are kept as they are');
  await page.screenshot({ path: join(shots, 'addons-title-en.png') });

  // Add to queue / Play next.
  m = await mark();
  await page.click('.src-addon[data-addon="2"] .src-add');
  await toast(page, 'Added');
  let post = (await since(m)).find((r) => r.method === 'POST' && r.path === '/api/admin/channels/main/items');
  check(post?.body?.url === 'https://addons.example/s/2/tt2-1-2.m3u8', '"Add to queue" -> POST …/items with the stream URL', post);
  m = await mark();
  await page.click('.src-addon[data-addon="3"] .src-next');
  await toast(page, 'Up next');
  const sent = await since(m);
  const next = sent.find((r) => r.path === '/api/admin/channels/main/play-next');
  check(
    sent.some((r) => r.method === 'POST' && r.path === '/api/admin/channels/main/items' && r.body?.url === 'https://addons.example/s/3/tt2-1-2.m3u8') && Number.isInteger(next?.body?.itemId) && next.body.itemId >= 100,
    '"Play next" adds the link, then puts that new item next',
    sent.map((r) => `${r.method} ${r.path} ${JSON.stringify(r.body)}`),
  );

  // Back keeps the results; the season select changes the episodes.
  await page.selectOption('.ep-season', '2');
  await waitFor(async () => (await texts(page, '.ep-title')).join() === 'Return', 'season 2 lists its episode');
  check(true, 'picking season 2 lists its episode');
  await page.click('.tp-back');
  await page.waitForSelector('.sr-card');
  check((await hash(page)) === '#/search' && (await count(page, '.sr-card')) === 2, 'Back returns to the same results');
  await page.waitForTimeout(100);

  // An episode address opens with that episode picked.
  await page.evaluate(() => (location.hash = '#/search/series/tt2%3A1%3A2'));
  await page.waitForSelector('.ep-row.on');
  check((await page.$eval('.ep-row.on', (e) => e.dataset.episode)) === 'tt2:1:2', 'an episode address opens with the episode picked');
  await waitFor(async () => (await count(page, '.src-addon')) === 3, 'and its sources');
  check(true, 'and its sources are asked at once');

  // ---------- Choose source for a viewer's request ----------
  await page.click('.rail-channel');
  await page.waitForSelector('.qi-request');
  await page.click('.qi-request .choose-source');
  await page.waitForSelector('.tp-for');
  check((await hash(page)) === `#/search/movie/tt1?for=${itemId}`, '"Choose source" opens the title page for the request', [await hash(page), itemId]);
  check((await text(page, '.tp-for')) === 'Choosing a source for the request “Example Film (2020)”.', 'a banner names the request', await text(page, '.tp-for'));
  await waitFor(async () => (await count(page, '.src-row')) === 2, 'sources of the movie');
  check((await count(page, '.src-use')) === 2 && (await count(page, '.src-add')) === 0, 'each source has one button: "Use this source"');
  check((await text(page, '.src-use')) === 'Use this source', 'its text');
  await page.screenshot({ path: join(shots, 'addons-choose-source-en.png') });
  m = await mark();
  await page.click('.src-addon[data-addon="2"] .src-use');
  await page.waitForFunction(() => location.hash === '#/channel');
  post = (await since(m)).find((r) => r.path === `/api/admin/channels/main/items/${itemId}/source`);
  check(post?.body?.url === 'https://addons.example/s/2/tt1.m3u8', '"Use this source" -> POST …/items/<id>/source with the URL', post);
  check((await toast(page, 'Source chosen')).startsWith('Source chosen'), 'the flow ends on the channel with a toast');
  await waitFor(async () => (await count(page, '.qi-request')) === 0, 'the request left the waiting list');
  check(true, 'the request is no longer waiting');
  // A request that is gone says so (409 not_title_request).
  await page.evaluate((id) => (location.hash = `#/search/movie/tt1?for=${id}`), itemId);
  await page.waitForSelector('.tp-for');
  check((await text(page, '.tp-for')) === 'This request is no longer waiting for approval.', 'a request that is gone says so', await text(page, '.tp-for'));
  await waitFor(async () => (await count(page, '.src-use')) === 2, 'sources again');
  await page.click('.src-addon[data-addon="2"] .src-use');
  check((await toast(page, 'This item')).startsWith('This item is not a title request'), 'a refused choice shows the engine\'s reason', await text(page, '.toast span'));

  // ---------- Settings -> Addons ----------
  await page.click('.rail-settings');
  await page.click('.snav a[data-section="addons"]');
  await page.waitForSelector('.ad-row');
  check((await text(page, '[data-note="responsibility"]')) === 'VRCinema does not provide any addons. You are responsible for what the addons you add offer.', 'the note says the program provides no addons');
  check((await text(page, '[data-note="expiry"]')).includes('expire after a few hours'), 'the hint about expiring links');
  check((await count(page, '.ad-row')) === 4, 'the addons are listed');
  check((await text(page, '.ad-row:nth-child(1) .ad-name')).startsWith('Example Catalog'), 'with their names', await text(page, '.ad-row:nth-child(1) .ad-name'));
  check(JSON.stringify(await texts(page, '.ad-row:nth-child(1) .chip-tag')) === '["Catalog","Details","Film","Series"]', 'what each provides and its types', await texts(page, '.ad-row:nth-child(1) .chip-tag'));
  await page.screenshot({ path: join(shots, 'addons-settings-en.png') });

  m = await mark();
  await page.click('.ad-row[data-addon="2"] [role="switch"]');
  await page.waitForSelector('.ad-row[data-addon="2"].off');
  post = (await since(m)).find((r) => r.method === 'POST' && r.path === '/api/admin/addons/2');
  check(JSON.stringify(post?.body) === '{"enabled":false}', 'the switch -> POST …/addons/2 {enabled: false}', post);
  check((await text(page, '.set-note.ok')) === 'Example Streams is off.', 'and says so', await text(page, '.set-note.ok'));

  m = await mark();
  await page.click('.ad-row[data-addon="1"] .ad-down');
  await waitFor(async () => (await texts(page, '.ad-name')).at(0)?.startsWith('Example Streams'), 'the order changes');
  post = (await since(m)).find((r) => r.method === 'POST' && r.path === '/api/admin/addons/1');
  check(JSON.stringify(post?.body) === '{"move":"down"}', 'down -> {move: "down"}', post);
  check(await page.$eval('.ad-row:first-child .ad-up', (b) => b.disabled), 'the first row cannot go up');

  await page.fill('input[name="addonUrl"]', 'not an address');
  await page.click('.ad-add button[type="submit"]');
  await page.waitForSelector('.set-note.err');
  check((await text(page, '.set-note.err')).includes('not a valid addon'), 'a bad address is refused with the engine\'s reason', await text(page, '.set-note.err'));
  await page.fill('input[name="addonUrl"]', 'https://addons.example/new/manifest.json');
  m = await mark();
  await page.click('.ad-add button[type="submit"]');
  await waitFor(async () => (await count(page, '.ad-row')) === 5, 'the addon joins the list');
  post = (await since(m)).find((r) => r.method === 'POST' && r.path === '/api/admin/addons');
  check(post?.body?.url === 'https://addons.example/new/manifest.json', 'Add -> POST …/addons {url}', post);
  check((await text(page, '.set-note.ok')) === 'Added: Example Added', 'and says what was added', await text(page, '.set-note.ok'));
  check((await page.$eval('input[name="addonUrl"]', (e) => e.value)) === '', 'the field is emptied');

  // Removing asks first.
  await page.click('.ad-row[data-addon="5"] .ad-remove');
  await page.waitForSelector('.dialog');
  check((await text(page, '.dialog h2')) === 'Remove this addon?', 'removing asks first');
  await page.click('.dialog-foot .btn:has-text("Cancel")');
  check((await count(page, '.ad-row')) === 5, 'Cancel keeps it');
  await page.click('.ad-row[data-addon="5"] .ad-remove');
  await page.waitForSelector('.dialog');
  m = await mark();
  await page.click('.dialog-foot .btn.danger');
  await waitFor(async () => (await count(page, '.ad-row')) === 4, 'the addon leaves the list');
  check((await since(m)).some((r) => r.method === 'DELETE' && r.path === '/api/admin/addons/5'), 'Remove -> DELETE …/addons/5');

  // ---------- Settings -> Channel: the switch for viewers ----------
  await page.click('.snav a[data-section="channel"]');
  await page.waitForSelector('[role="switch"][aria-label="Viewers can search for a film"]');
  m = await mark();
  await page.click('[role="switch"][aria-label="Viewers can search for a film"]');
  await page.waitForSelector('.set-note.ok');
  put: {
    const put = (await since(m)).find((r) => r.method === 'PUT' && r.path === '/api/admin/channels/main/settings');
    check(JSON.stringify(put?.body) === '{"viewerCatalog":false}', 'the viewer switch -> PUT settings {viewerCatalog: false}', put);
  }

  // ---------- The request page: "Find a film" ----------
  const viewer = await ctx.newPage();
  const verrors = [];
  viewer.on('pageerror', (e) => verrors.push(String(e)));
  await viewer.setViewportSize({ width: 390, height: 780 });
  await viewer.route('https://addons.example/**', (r) => r.abort());
  await viewer.goto(`${base}/`);
  await showAdd(viewer);
  await viewer.waitForSelector('.v-add form');
  await viewer.evaluate(async () => (await import('/static/app/i18n.js')).loadLang('en'));
  check((await count(viewer, '.v-mode')) === 0, 'viewer: with the setting off there is no switch');
  await scenario({ settingsPatch: { viewerCatalog: true } });
  await viewer.waitForSelector('.v-mode', { timeout: 8000 });
  check(JSON.stringify(await texts(viewer, '.v-mode .btn')) === '["Link","Find a film"]', 'viewer: with it on the switch shows', await texts(viewer, '.v-mode .btn'));
  await viewer.click('.v-mode [data-mode="film"]');
  check(await viewer.$eval('.v-submit', (b) => b.disabled), 'viewer: Send waits for a title');
  await viewer.fill('input[name="title"]', 'exa');
  await waitFor(async () => (await count(viewer, '.v-film-row')) === 2, 'viewer: results');
  check(await viewer.evaluate(() => document.documentElement.scrollWidth <= 390), 'viewer: no sideways scroll');
  await viewer.screenshot({ path: join(shots, 'addons-viewer-search-phone.png') });
  await viewer.click('.v-film-row[data-type="series"]');
  await viewer.waitForSelector('.v-film-row[data-episode="tt2:1:2"]');
  check(await viewer.$eval('.v-submit', (b) => b.disabled), 'viewer: a series needs an episode');
  await viewer.click('.v-film-row[data-episode="tt2:1:2"]');
  await viewer.screenshot({ path: join(shots, 'addons-viewer-episodes-phone.png') });
  m = await mark();
  await viewer.click('.v-submit');
  const vsent = await viewer.waitForFunction(() => document.querySelector('.v-toast')?.textContent === 'Sent. The host will pick a source.', null, { timeout: 5000 }).then(() => true, () => false);
  const post2 = (await since(m)).find((r) => r.method === 'POST' && r.path === '/api/channels/main/requests');
  check(JSON.stringify(post2?.body?.title) === '{"type":"series","id":"tt2:1:2"}', 'viewer: Send -> POST …/requests {title: {series, tt2:1:2}}', post2);
  check(vsent, 'viewer: shows the sent message', await viewer.evaluate(() => document.body.innerText.slice(0, 300)));
  check(verrors.length === 0, 'viewer: no page errors', verrors);
  await viewer.close();

  // ---------- Torrent sources: the settings block ----------
  await page.evaluate(() => (location.hash = '#/settings/addons'));
  await page.waitForSelector('.torrent-settings');
  check((await page.getAttribute('.torrent-settings', 'data-torrents')) === 'off', 'torrents: off by default');
  check((await text(page, '.torrent-settings .row2-text small')).includes('public IP address'), 'torrents: the warning mentions the public IP');
  check((await page.inputValue('.torrent-settings [name="torrentUploadKbps"]')) === '200' && (await page.inputValue('.torrent-settings [name="torrentDiskCapGb"]')) === '50', 'torrents: defaults 200 KB/s and 50 GB');
  m = await mark();
  await page.click('.torrent-settings [role="switch"]');
  await waitFor(async () => (await page.getAttribute('.torrent-settings', 'data-torrents')) === 'on', 'torrents: the switch turns on');
  const put1 = (await since(m)).find((r) => r.method === 'PUT' && r.path === '/api/admin/channels/main/settings');
  check(JSON.stringify(put1?.body) === '{"torrentsEnabled":true}', 'torrents: switch -> PUT {torrentsEnabled: true}', put1);
  await page.fill('.torrent-settings [name="torrentUploadKbps"]', '0');
  await page.fill('.torrent-settings [name="torrentDiskCapGb"]', '20');
  m = await mark();
  await page.click('.tr-limits button[type="submit"]');
  await page.waitForSelector('.torrent-settings .set-note.ok');
  const put2 = (await since(m)).find((r) => r.method === 'PUT' && r.path === '/api/admin/channels/main/settings');
  check(JSON.stringify(put2?.body) === '{"torrentUploadKbps":0,"torrentDiskCapGb":20}', 'torrents: limits -> PUT {upload 0, cap 20}', put2);
  await page.fill('.torrent-settings [name="torrentDiskCapGb"]', '0');
  await page.click('.tr-limits button[type="submit"]');
  await page.waitForSelector('.torrent-settings .set-note.err');
  check(true, 'torrents: a cap of 0 is refused with a message');
  // The download folder: empty shows the default as its placeholder, with the drive's free space.
  check((await page.inputValue('.torrent-settings [name="torrentDir"]')) === '', 'torrents: the download folder is empty by default');
  check((await page.getAttribute('.torrent-settings [name="torrentDir"]', 'placeholder')).endsWith('VRCinema\\data\\torrents'), 'torrents: the folder placeholder shows the default folder');
  check((await text(page, '.tr-drive-free')) === 'Free space on its drive: 15.6 GB', 'torrents: the drive\u2019s free space is shown', await text(page, '.tr-drive-free'));
  check((await text(page, '.tr-dir-hint')).includes('VRCinema-torrents'), 'torrents: the hint names the VRCinema-torrents subfolder');
  await page.fill('.torrent-settings [name="torrentDir"]', 'relative\\dir');
  await page.click('.tr-dir button[type="submit"]');
  await waitFor(async () => (await texts(page, '.torrent-settings .set-note.err')).some((s) => s.includes('full path')), 'torrents: a relative folder is refused with a message');
  check(true, 'torrents: a relative folder is refused with a message');
  await page.fill('.torrent-settings [name="torrentDir"]', ' D:\\Films ');
  m = await mark();
  await page.click('.tr-dir button[type="submit"]');
  await page.waitForSelector('.torrent-settings .set-note.ok');
  const put3 = (await since(m)).find((r) => r.method === 'PUT' && r.path === '/api/admin/channels/main/settings');
  check(JSON.stringify(put3?.body) === '{"torrentDir":"D:\\\\Films"}', 'torrents: Save -> PUT {torrentDir: "D:\\\\Films"} (trimmed)', put3);
  check((await text(page, '.torrent-settings .set-note.ok')) === 'Download folder saved.', 'torrents: the folder save is confirmed');
  check((await page.inputValue('.torrent-settings [name="torrentDir"]')) === 'D:\\Films', 'torrents: the saved folder stays in the field');
  await page.screenshot({ path: join(shots, 'torrents-settings-en.png') });

  // ---------- The torrent chip on a source ----------
  await page.evaluate(() => (location.hash = '#/search'));
  await page.fill('.sr-input', 'exa');
  await waitFor(async () => (await count(page, '.sr-card')) === 2, 'torrents: results');
  await page.click('.sr-card[data-type="movie"]');
  await waitFor(async () => (await count(page, '.src-addon[data-state="loading"]')) === 0 && (await count(page, '.src-addon')) > 0, 'torrents: sources');
  check((await count(page, '.src-torrent')) === 1 && (await text(page, '.src-torrent')) === 'torrent', 'torrents: a torrent source has the "torrent" chip, the others do not');

  // ---------- The queue item ----------
  await scenario({ torrentItem: { state: 'downloading', progress: 0.42, rateBps: 3100000, etaS: 240, ready: false } });
  await page.evaluate(() => (location.hash = '#/channel'));
  await page.waitForSelector('.qi-torrent');
  await waitFor(async () => (await text(page, '.qi-torrent')) === 'Downloading 42 % · 3.1 MB/s · ready in ~4 min', 'torrents: the progress line', await texts(page, '.qi-torrent'));
  check(true, 'torrents: the queue item shows "Downloading 42 % · 3.1 MB/s · ready in ~4 min"');
  await page.$eval('.qi-torrent', (e) => e.scrollIntoView({ block: 'center' }));
  await page.screenshot({ path: join(shots, 'torrents-queue-en.png') });
  await scenario({ torrentItem: { state: 'waitingForDisk', progress: 0, rateBps: 0, etaS: null, ready: false, diskFreeBytes: 15 * 1024 ** 3, needBytes: 20.5 * 1024 ** 3 } });
  await waitFor(async () => (await texts(page, '.qi-torrent')).includes('Not enough free space on the drive (15.0 GB free, needs 20.5 GB)'), 'torrents: the drive-space line');
  check(true, 'torrents: a download waiting for the drive says how much is free and needed');

  // ---------- A magnet link: no title search; refused while torrents are off ----------
  await scenario({ settingsPatch: { torrentsEnabled: false } });
  m = await mark();
  await page.fill('.addf-input', '');
  await page.click('.addf-input');
  await page.keyboard.type('magnet:?xt=urn:btih:' + 'c'.repeat(40));
  await sleep(700);
  check((await since(m)).filter((r) => r.path === '/api/admin/addons/search').length === 0 && (await count(page, '.addf-list')) === 0, 'torrents: a magnet link does not open the title dropdown');
  await page.keyboard.press('Enter');
  await page.waitForSelector('.addf-error');
  check((await text(page, '.addf-error')).includes('Turn them on in Settings'), 'torrents: a magnet link with torrents off shows the message', await text(page, '.addf-error'));
  await page.fill('.addf-input', '');

  // ---------- Polish ----------
  await page.evaluate(async () => (await import('/static/app/i18n.js')).loadLang('pl'));
  await page.waitForFunction(() => document.querySelector('.rail-search .rail-label')?.textContent === 'Szukaj');
  check(true, 'pl: the rail says "Szukaj"');
  await page.evaluate(() => (location.hash = '#/settings/addons'));
  await page.waitForSelector('.ad-row');
  check((await text(page, '[data-note="responsibility"]')).startsWith('VRCinema nie dostarcza żadnych dodatków.'), 'pl: the note');
  await page.screenshot({ path: join(shots, 'addons-settings-pl.png') });
  await page.evaluate(async () => (await import('/static/app/i18n.js')).loadLang('en'));

  // ---------- A phone: five tabs fit; the search page and a title page work ----------
  await page.setViewportSize({ width: 390, height: 780 });
  await page.evaluate(() => (location.hash = '#/search'));
  await page.waitForSelector('.sr-input');
  const tabs = await page.$$eval('.rail-item', (els) => els.map((e) => ({ l: Math.round(e.getBoundingClientRect().left), r: Math.round(e.getBoundingClientRect().right), w: e.scrollWidth - e.clientWidth })));
  check(tabs.length === 5 && tabs.every((x) => x.l >= 0 && x.r <= 390 && x.w <= 1), 'phone: five tabs fit the width', tabs);
  await page.fill('.sr-input', 'exa');
  await waitFor(async () => (await count(page, '.sr-card')) === 2, 'phone: results');
  check(await page.evaluate(() => document.documentElement.scrollWidth <= 390 && document.querySelector('.view').scrollWidth <= 390), 'phone: no sideways scroll on the search page');
  await page.screenshot({ path: join(shots, 'addons-search-phone.png') });
  await page.click('.sr-card[data-type="movie"]');
  await waitFor(async () => (await count(page, '.src-addon')) === 2 && (await count(page, '.src-addon[data-state="loading"]')) === 0, 'phone: sources (addon 2 is off now)');
  check(await page.evaluate(() => document.querySelector('.view').scrollWidth <= 390), 'phone: no sideways scroll on a title page');
  await page.screenshot({ path: join(shots, 'addons-title-phone.png') });
  await page.evaluate(() => (location.hash = '#/settings/addons'));
  await page.waitForSelector('.ad-row');
  check(await page.evaluate(() => document.querySelector('.view').scrollWidth <= 390), 'phone: no sideways scroll in Settings -> Addons');
  await page.screenshot({ path: join(shots, 'addons-settings-phone.png') });

  check(page.__errors.length === 0, 'no console errors', page.__errors);

  // ---------- A remote panel: the download folder stays in the window ----------
  await scenario({ remote: true });
  const remote = await openApp();
  await remote.evaluate(() => (location.hash = '#/settings/addons'));
  await remote.waitForSelector('.tr-dir-remote');
  check((await count(remote, '[name="torrentDir"]')) === 0 && (await count(remote, '.tr-drive-free')) === 0, 'remote: no download folder field and no free space');
  check((await text(remote, '.tr-dir-remote')).includes('Download folder'), 'remote: the folder is named as window-only', await text(remote, '.tr-dir-remote'));
  const rs = await remote.evaluate(async () => (await fetch('/api/admin/channels/main/settings')).json());
  check(!('torrentDir' in rs) && !('torrentDirDefault' in rs) && !('torrentDriveFreeBytes' in rs), 'remote: the settings carry no folder path or free space', rs);
  await remote.close();
  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
}
