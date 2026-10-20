// Browser check of the files on disk (MEDIA_DIRS) in the Channel view of a server without a
// window (final wave I3): root folders, the "you are in" line, folders that open, one folder
// up, files with their size and "Add to queue", "Add the whole folder", errors; nothing of it in
// the window or without a library. Against the fake engine, in headless Brave with a temporary
// profile.
//
//   PLAYWRIGHT_CORE=<path to playwright-core/index.mjs> node tools/ui-check/library.mjs <shots dir> [port]
//
// Exits 1 on the first failed check; screenshots go to <shots dir>.

import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFakeEngine } from './fake-engine.mjs';

const BRAVE = 'C:/Program Files/BraveSoftware/Brave-Browser/Application/brave.exe';
const shots = process.argv[2];
const port = Number(process.argv[3] ?? 8199);
if (!shots) throw new Error('usage: library.mjs <shots dir> [port]');
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

async function waitFor(fn, what, timeout = 5000) {
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
const toast = (page, prefix) =>
  page.waitForFunction((p) => document.querySelector('.toast span')?.textContent.startsWith(p), prefix, { timeout: 5000 }).then(() => text(page, '.toast span'));
const rows = (page) =>
  page.$$eval('.lib-list > li', (items) =>
    items.map((li) =>
      [...li.querySelectorAll('.lib-dir, .lib-name, .lib-size, .lib-add')].map((e) => e.textContent.trim()).join(' '),
    ),
  );

const LIBRARY = [
  {
    name: 'Filmy',
    tree: {
      'Seriale & bajki': { 'Odcinek 1.mp4': 1.5 * 1024 ** 3, 'Odcinek 2.mkv': 2.25 * 1024 ** 3 },
      Pusty: {},
      'Film.mp4': 734003200,
    },
  },
  { name: 'Muzyka', tree: { 'Koncert.mp4': 1024 ** 3 } },
];

async function openApp({ window = false } = {}) {
  const page = await ctx.newPage();
  if (window) await page.addInitScript(FAKE_TAURI);
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && !/status of 4\d\d/.test(m.text()) && errors.push(m.text()));
  await page.goto(`${base}/admin`);
  await page.waitForSelector('.qi');
  page.__errors = errors;
  return page;
}

try {
  // ---------- Not in the window, not without a library ----------
  await reset();
  await scenario({ library: LIBRARY });
  let page = await openApp({ window: true });
  await sleep(800);
  check((await page.$('.lib')) === null, 'the window shows no file browser (it adds files by dropping them)');
  check(!(await requests()).some((r) => r.path === '/api/admin/library'), 'and does not ask for the library');
  await page.close();

  await reset();
  await scenario({ headless: true });
  page = await openApp();
  await waitFor(async () => (await requests()).some((r) => r.path === '/api/admin/library'), 'the headless panel asks for the library');
  await sleep(300);
  check((await page.$('.lib')) === null, 'a server without MEDIA_DIRS shows no file browser');
  await page.close();

  // ---------- A server without a window, with two root folders ----------
  await reset();
  await scenario({ headless: true, library: LIBRARY });
  page = await openApp();
  await page.waitForSelector('.lib');
  check((await text(page, '.lib h3')) === 'Files on disk', 'the block is "Files on disk"', await text(page, '.lib h3'));
  check(
    JSON.stringify(await page.$$eval('.lib select option', (o) => o.map((x) => [x.value, x.textContent]))) === '[["0","Filmy"],["1","Muzyka"]]',
    'the root folders are offered in a list',
  );
  check((await page.$eval('.lib select', (e) => e.getAttribute('aria-label'))) === 'Folder', 'the list is labelled "Folder"');
  check((await text(page, '.lib-where')) === 'Root folder', 'at the top: "Root folder"', await text(page, '.lib-where'));
  check((await page.$('.lib-up')) === null, 'no "Up one folder" at the top');
  check(
    JSON.stringify(await rows(page)) === JSON.stringify(['Pusty', 'Seriale & bajki', 'Film.mp4 0.68 GB Add to queue']),
    'folders first, then files with their size and "Add to queue"',
    await rows(page),
  );
  check(await page.$eval('.lib-add-folder', (b) => !b.disabled && b.textContent.trim() === 'Add the whole folder'), '"Add the whole folder" is there with files');
  await page.screenshot({ path: join(shots, 'library-root-en.png') });

  // A folder opens; its name goes encoded into the request.
  let m = await mark();
  await page.click('.lib-list button.lib-dir:text("Seriale & bajki")');
  await page.waitForFunction(() => document.querySelector('.lib-where')?.textContent.includes('Seriale'));
  let get = (await since(m)).find((r) => r.path === '/api/admin/library');
  check(Boolean(get), 'opening a folder asks for its listing');
  const listed = (await requests()).filter((r) => r.path === '/api/admin/library').length;
  check(listed >= 1, 'listing requests are recorded');
  const urls = await page.evaluate(() => performance.getEntriesByType('resource').map((e) => e.name).filter((n) => n.includes('/api/admin/library')));
  check(urls.some((u) => u.endsWith('?root=0&path=Seriale%20%26%20bajki')), 'the path is sent with encodeURIComponent', urls);
  check((await text(page, '.lib-where')) === 'You are in: Seriale & bajki', '"You are in: …"', await text(page, '.lib-where'));
  check((await text(page, '.lib-up')) === 'Up one folder', '"Up one folder" inside a folder');
  check(
    JSON.stringify(await rows(page)) === JSON.stringify(['Odcinek 1.mp4 1.50 GB Add to queue', 'Odcinek 2.mkv 2.25 GB Add to queue']),
    'the folder\'s files',
    await rows(page),
  );

  // Add one file.
  m = await mark();
  await page.click('.lib-list li:nth-child(1) .lib-add');
  check((await toast(page, 'Added')) === 'Added: Odcinek 1', 'adding a file says what was added', await text(page, '.toast span'));
  let post = (await since(m)).find((r) => r.method === 'POST');
  check(
    post?.path === '/api/admin/channels/main/local' && JSON.stringify(post.body) === '{"root":0,"path":"Seriale & bajki/Odcinek 1.mp4"}',
    '"Add to queue" → POST …/local {root, path}',
    post,
  );
  await waitFor(async () => (await page.$$('.qi-queued')).length === 5, 'the file appears in the queue');
  check(true, 'the queue shows it at once');

  // The whole folder.
  m = await mark();
  await page.click('.lib-add-folder');
  check((await toast(page, 'Added 2')) === 'Added 2 videos.', 'adding the folder says how many', await text(page, '.toast span'));
  post = (await since(m)).find((r) => r.method === 'POST');
  check(JSON.stringify(post?.body) === '{"root":0,"path":"Seriale & bajki"}', '"Add the whole folder" → POST …/local {root, path of the folder}', post);

  // Up again, and an empty folder.
  await page.click('.lib-up');
  await page.waitForFunction(() => document.querySelector('.lib-where')?.textContent === 'Root folder');
  check(true, '"Up one folder" goes back');
  await page.click('.lib-list button.lib-dir:text("Pusty")');
  await page.waitForSelector('.lib-empty');
  check((await text(page, '.lib-empty')) === 'No videos or folders here.', 'an empty folder says so', await text(page, '.lib-empty'));
  check(await page.$eval('.lib-add-folder', (b) => b.disabled), '"Add the whole folder" is disabled without files');

  // The other root folder.
  await page.selectOption('.lib select', '1');
  await page.waitForFunction(() => document.querySelector('.lib-list')?.textContent.includes('Koncert'));
  check((await text(page, '.lib-where')) === 'Root folder', 'another root folder opens at its top');
  check(JSON.stringify(await rows(page)) === JSON.stringify(['Koncert.mp4 1.00 GB Add to queue']), 'with its own files', await rows(page));

  // A refusal comes back as the engine's message in the interface language.
  await scenario({ libraryAddError: 'folder_unreadable' });
  await page.click('.lib-list li:nth-child(1) .lib-add');
  check((await toast(page, 'Cannot')) === 'Cannot read the folder.', 'an error is shown with its dictionary text', await text(page, '.toast span'));
  await scenario({ libraryAddError: null });
  await page.screenshot({ path: join(shots, 'library-muzyka-en.png') });

  // Polish.
  await page.evaluate(async () => (await import('/static/app/i18n.js')).loadLang('pl'));
  await page.waitForFunction(() => document.querySelector('.lib h3')?.textContent === 'Pliki z dysku');
  check((await text(page, '.lib-list .lib-add')) === 'Dodaj do kolejki', 'pl: "Dodaj do kolejki"');
  check((await text(page, '.lib-where')) === 'Folder główny', 'pl: "Folder główny"');
  check((await text(page, '.lib-list li .lib-size')) === '1,00 GB', 'pl: the size with a decimal comma', await text(page, '.lib-list li .lib-size'));
  await page.screenshot({ path: join(shots, 'library-pl.png') });
  // The longest language: no button cut off in the narrow panel.
  await page.evaluate(async () => (await import('/static/app/i18n.js')).loadLang('de'));
  await page.waitForFunction(() => document.querySelector('.lib h3')?.textContent === 'Dateien vom Datenträger');
  const cut = await page.$$eval('.lib button, .lib select', (els) =>
    els.filter((e) => e.scrollWidth > e.clientWidth + 1 || e.getBoundingClientRect().right > e.closest('.q').getBoundingClientRect().right + 1).map((e) => e.textContent.trim()),
  );
  check(cut.length === 0, 'de: no control of the block is cut off', cut);
  await page.screenshot({ path: join(shots, 'library-de.png') });
  check(page.__errors.length === 0, 'no console errors', page.__errors);
  await page.close();

  console.log(`\n${passed} checks passed`);
} finally {
  await ctx.close();
  rmSync(profile, { recursive: true, force: true });
  await engine.close();
}
