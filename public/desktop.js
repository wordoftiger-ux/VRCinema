// Panel w oknie aplikacji VRCinema.exe: zakładka „Ustawienia”, paski (aktualizacja, naprawa,
// restart), warstwa przeciągania plików. Ładowany tylko wtedy, gdy przeglądarka ma znacznik okna
// aplikacji; zwykła przeglądarka (tryb serwera) nigdy tego pliku nie pobiera.

import { api, apiRetry, el, notice, sleep, encoderLabel, toolProgressList, TOOL_ORDER, TOOL_TEXT, watchToolsJob } from '/static/common.js';

const $ = (id) => document.getElementById(id);

let info = null; // GET /api/desktop/info
let config = null; // GET /api/desktop/config
let updateOffer = null; // { version, notes } albo null
let toolsJobRunning = false;
let restartInProgress = false; // „Uruchom ponownie kanał” czeka na odpowiedź
let hooks = {};

const plural = (n, one, few, many) => (n === 1 ? one : n % 10 >= 2 && n % 10 <= 4 && (n % 100 < 10 || n % 100 >= 20) ? few : many);

/**
 * Włącza interfejs aplikacji, jeśli to naprawdę okno aplikacji. Zwraca false w zwykłej przeglądarce
 * (znacznik został po wygaśnięciu sesji). Pierwsze uruchomienie przenosi do kreatora.
 */
export async function start({ refreshQueue }) {
  try {
    info = await api('GET', '/api/desktop/info');
  } catch {
    return false;
  }
  if (!info.firstRunCompleted) {
    location.replace('/wizard');
    await new Promise(() => {}); // strona się zmienia
  }
  hooks = { refreshQueue };
  document.querySelectorAll('.app-only').forEach((n) => n.classList.remove('app-only'));
  document.querySelectorAll('.headless-only').forEach((n) => n.classList.add('hidden'));
  updateOffer = info.update;

  // „Ustawienia kanału” z panelu przenoszą się do zakładki.
  const block = $('channel-settings-block');
  block.classList.add('settings-block');
  block.querySelector('h2').textContent = 'Kanał';
  $('channel-slot').append(block);

  $('channel-hint').textContent = 'Ten link wklejasz do odtwarzacza w VRChat. Musi być adresem publicznym, np. z Cloudflare Tunnel. Adres publiczny ustawisz w zakładce Ustawienia, w sekcji Zaawansowane.';
  wireTabs();
  wireGeneral();
  wireRemote();
  wireTools();
  wireAdvanced();
  wireLog();
  wireAbout();
  wireDrop();
  renderBanners();
  setInterval(refreshInfo, 5000);
  return true;
}

// ---------- Zakładki ----------
function wireTabs() {
  const tabs = { channel: $('tab-channel'), settings: $('tab-settings') };
  const buttons = { channel: $('tab-btn-channel'), settings: $('tab-btn-settings') };
  for (const name of Object.keys(tabs)) {
    buttons[name].addEventListener('click', () => showTab(name));
  }
}

export function showTab(name) {
  $('tab-channel').classList.toggle('hidden', name !== 'channel');
  $('tab-settings').classList.toggle('hidden', name !== 'settings');
  $('tab-btn-channel').setAttribute('aria-selected', String(name === 'channel'));
  $('tab-btn-settings').setAttribute('aria-selected', String(name === 'settings'));
  if (name === 'settings') loadSettings();
}

async function loadSettings() {
  await Promise.all([loadConfig(), loadTools(), loadLog()]);
}

// ---------- Paski ----------
let bannerKey = '';

async function refreshInfo() {
  try {
    info = await api('GET', '/api/desktop/info');
  } catch {
    return; // serwer się uruchamia ponownie: następne odpytanie
  }
  updateOffer = info.update;
  renderBanners();
}

function banner(kind, text, ...buttons) {
  return el('div', { class: `banner ${kind}`, role: 'status' }, el('p', {}, text), el('div', { class: 'banner-actions' }, ...buttons));
}

function renderBanners() {
  const restarting = info.restarting || restartInProgress;
  const key = JSON.stringify([info.toolsOk, info.channelRunning, info.channelError, restarting, updateOffer?.version, toolsJobRunning]);
  if (key === bannerKey) return;
  bannerKey = key;
  const list = [];
  const repairButton = () => el('button', { type: 'button', disabled: toolsJobRunning, onclick: () => runToolsJob('repair') }, 'Napraw narzędzia');
  // W trakcie ponownego uruchamiania kanał chwilowo nie działa: to nie jest awaria.
  if (!restarting && !info.toolsOk) {
    list.push(banner('err', 'Brakuje narzędzi, więc kanał nie nadaje.', repairButton()));
  } else if (!restarting && !info.channelRunning) {
    const why = info.channelError ? ` Powód: ${info.channelError}` : '';
    list.push(banner('err', `Kanał nie wystartował, choć narzędzia są pobrane — mogą być uszkodzone.${why}`,
      el('button', { type: 'button', disabled: toolsJobRunning, onclick: restartChannel }, 'Uruchom ponownie kanał'),
      repairButton()));
  }
  if (updateOffer) {
    list.push(banner('', `Dostępna jest nowa wersja programu: ${updateOffer.version}.`,
      el('button', { type: 'button', onclick: applyUpdate }, 'Zaktualizuj'),
      el('button', { type: 'button', class: 'quiet', onclick: skipUpdate }, 'Pomiń tę wersję')));
  }
  $('app-banners').replaceChildren(...list);
}

async function restartChannel(e) {
  const button = e.currentTarget;
  button.disabled = true;
  button.textContent = 'Uruchamiam ponownie…';
  restartInProgress = true;
  try {
    await api('POST', '/api/desktop/restart');
  } catch (err) {
    if (err.status !== undefined) alert(err.message);
  } finally {
    restartInProgress = false;
  }
  await settle();
}

/** Czeka, aż serwer znów odpowie, i odświeża paski. */
async function settle() {
  try {
    info = await apiRetry('GET', '/api/desktop/info');
  } catch {
    return;
  }
  updateOffer = info.update;
  bannerKey = '';
  renderBanners();
  hooks.refreshQueue?.();
}

async function applyUpdate() {
  notice($('about-notice'), 'Instaluję nową wersję…');
  $('app-banners').replaceChildren(banner('', 'Instaluję nową wersję. Program uruchomi się ponownie.'));
  bannerKey = '';
  try {
    const r = await api('POST', '/api/desktop/update/apply');
    notice($('about-notice'), `Zainstalowano wersję ${r.version}. Program uruchamia się ponownie…`, 'ok');
  } catch (err) {
    notice($('about-notice'), err.message, 'err');
    $('app-banners').replaceChildren(banner('err', `Aktualizacja się nie udała: ${err.message}`));
    await sleep(6000);
    bannerKey = '';
    renderBanners();
  }
}

async function skipUpdate() {
  if (!updateOffer) return;
  try {
    await api('PUT', '/api/desktop/config', { skippedUpdateVersion: updateOffer.version });
  } catch (err) {
    return alert(err.message);
  }
  updateOffer = null;
  bannerKey = '';
  renderBanners();
  renderAbout();
}

// ---------- Ogólne ----------
async function loadConfig() {
  try {
    config = await apiRetry('GET', '/api/desktop/config', undefined, { tries: 5 });
  } catch (err) {
    return notice($('general-notice'), err.message, 'err');
  }
  fillConfig();
}

function fillConfig() {
  const c = config;
  $('set-autostart').checked = c.autostart;
  $('set-lan').checked = c.lanAccess;
  $('set-port').value = c.port;
  $('set-remote').checked = c.remoteAdmin;
  $('password-state').textContent = c.hasPassword ? 'Hasło jest ustawione.' : 'Hasło nie jest ustawione.';
  $('password-form').querySelector('button').textContent = c.hasPassword ? 'Zmień hasło' : 'Ustaw hasło';
  $('set-encoder').value = c.videoEncoder;
  $('encoder-state').textContent = c.detectedEncoder
    ? `Wynik ostatniego testu: ${encoderLabel(c.detectedEncoder)}.`
    : 'Test karty jeszcze się nie odbył.';
  $('set-public-url').value = c.publicUrl ?? '';
  $('cookies-path').textContent = c.ytdlpCookies ?? 'Nie wybrano pliku.';
  $('cookies-clear').classList.toggle('hidden', !c.ytdlpCookies);
  $('set-pixeldrain').value = c.pixeldrainApiKey ?? '';
  $('set-trust').checked = c.trustCloudflare;
}

/** Zapisuje część ustawień. Zwraca odpowiedź albo null (błąd pokazany w `target`, widok wraca do zapisanych wartości). */
async function saveConfig(patch, target, okText) {
  notice(target, '');
  try {
    const r = await api('PUT', '/api/desktop/config', patch);
    config = r;
    fillConfig();
    if (r.restarted) {
      notice(target, `Serwer uruchomił się ponownie na porcie ${r.port}. Okno przechodzi na nowy adres, a link kanału ma teraz ten port.`, 'ok');
    } else {
      notice(target, okText, 'ok');
    }
    settle();
    return r;
  } catch (err) {
    if (err.status === undefined) {
      notice(target, 'Serwer uruchamia się ponownie. Za chwilę sprawdzę, czy zmiana została zapisana.');
      await sleep(1500);
      await loadConfig();
    } else {
      notice(target, err.message, 'err');
      await loadConfig();
    }
    return null;
  }
}

function wireGeneral() {
  const n = $('general-notice');
  $('set-autostart').addEventListener('change', (e) =>
    saveConfig({ autostart: e.target.checked }, n, e.target.checked ? 'Program będzie uruchamiał się razem z Windows.' : 'Program nie będzie już uruchamiał się razem z Windows.'));
  $('set-lan').addEventListener('change', (e) =>
    saveConfig({ lanAccess: e.target.checked }, n, e.target.checked ? 'Inne urządzenia w sieci domowej mogą teraz otworzyć stronę.' : 'Strona jest dostępna tylko na tym komputerze.'));
  $('port-form').addEventListener('submit', (e) => {
    e.preventDefault();
    const port = Number($('set-port').value);
    if (port === config?.port) return notice(n, 'To już jest bieżący port.');
    saveConfig({ port }, n, `Zapisano port ${port}.`);
  });
}

// ---------- Panel zdalny ----------
let wantRemote = false; // włączenie zaczęte bez hasła: dokończy je ustawienie hasła

function wireRemote() {
  const n = $('remote-notice');
  const field = $('set-password');
  $('set-remote').addEventListener('change', async (e) => {
    const on = e.target.checked;
    if (on && !config.hasPassword) {
      e.target.checked = false;
      wantRemote = true;
      notice(n, 'Najpierw ustaw hasło (co najmniej 8 znaków). Panel zdalny włączy się razem z nim.');
      field.focus();
      return;
    }
    wantRemote = false;
    await saveConfig({ remoteAdmin: on }, n, on ? 'Panel zdalny jest włączony.' : 'Panel zdalny jest wyłączony.');
  });
  $('password-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const patch = { password: field.value };
    if (wantRemote) patch.remoteAdmin = true;
    const r = await saveConfig(patch, n, wantRemote ? 'Hasło ustawione, panel zdalny jest włączony.' : 'Hasło zostało zmienione. Zdalne sesje zalogowane wcześniej wygasły.');
    if (r) {
      field.value = '';
      wantRemote = false;
    }
  });
}

// ---------- Narzędzia ----------
async function loadTools() {
  let t;
  try {
    t = await apiRetry('GET', '/api/desktop/tools', undefined, { tries: 5 });
  } catch (err) {
    return notice($('tools-notice'), err.message, 'err');
  }
  toolsJobRunning = t.running;
  $('tools-list').replaceChildren(
    ...t.tools.map((tool) =>
      el('div', { class: 'kv' },
        el('span', { class: 'name' }, TOOL_TEXT[tool.name] ?? tool.name),
        el('span', { class: 'state' }, tool.installed ? `wersja ${tool.version ?? 'nieznana'}` : 'nie zainstalowane'),
        el('button', { type: 'button', class: 'quiet', disabled: t.running, onclick: () => runToolsJob('update', tool.name) }, tool.installed ? 'Aktualizuj' : 'Pobierz'))));
}

function wireTools() {
  $('tools-repair').addEventListener('click', () => runToolsJob('repair'));
  $('encoder-detect').addEventListener('click', async (e) => {
    const n = $('encoder-notice');
    const button = e.currentTarget;
    button.disabled = true;
    notice(n, 'Sprawdzam kartę graficzną. To potrwa kilka sekund…');
    try {
      const r = await apiRetry('POST', '/api/desktop/encoder/detect');
      notice(n, `Wynik testu: ${encoderLabel(r.detectedEncoder)}.`, 'ok');
      await loadConfig();
      settle();
    } catch (err) {
      notice(n, err.message, 'err');
    } finally {
      button.disabled = false;
    }
  });
  $('set-encoder').addEventListener('change', (e) =>
    saveConfig({ videoEncoder: e.target.value }, $('encoder-notice'), e.target.value === 'auto' ? 'Enkoder wybierany automatycznie.' : 'Zapisano enkoder.'));
}

/** Pobieranie, aktualizacja albo naprawa narzędzi z paskami postępu w sekcji „Narzędzia”. */
export async function runToolsJob(kind, tool) {
  showTab('settings');
  const n = $('tools-notice');
  const box = $('tools-progress');
  notice(n, '');
  const names = kind === 'update' ? [tool] : TOOL_ORDER;
  const list = toolProgressList(names);
  box.replaceChildren(list.root);
  box.scrollIntoView({ block: 'center' });
  toolsJobRunning = true;
  bannerKey = '';
  renderBanners();
  try {
    try {
      await apiRetry('POST', kind === 'update' ? '/api/desktop/tools/update' : '/api/desktop/tools/repair', kind === 'update' ? { tool } : undefined);
    } catch (err) {
      if (err.status !== 409) throw err;
    }
    const p = await watchToolsJob((progress) => list.update(names, progress));
    if (p.error) notice(n, `Nie udało się: ${p.error}`, 'err');
    else notice(n, kind === 'update' ? 'Narzędzie jest zaktualizowane.' : 'Narzędzia są naprawione.', 'ok');
  } catch (err) {
    notice(n, err.message, 'err');
  } finally {
    toolsJobRunning = false;
    await Promise.all([loadTools(), settle()]);
    if ($('tools-notice').className.includes('ok')) box.replaceChildren();
  }
}

// ---------- Zaawansowane ----------
function wireAdvanced() {
  const n = $('advanced-notice');
  $('public-url-form').addEventListener('submit', (e) => {
    e.preventDefault();
    saveConfig({ publicUrl: $('set-public-url').value }, n, 'Zapisano adres publiczny. Pojawi się w linku kanału po ponownym uruchomieniu kanału.');
  });
  $('cookies-pick').addEventListener('click', async () => {
    notice(n, 'Wybierz plik w oknie, które się otworzyło…');
    try {
      const r = await api('POST', '/api/desktop/pick-cookies');
      notice(n, r.picked ? 'Zapisano plik cookies.' : 'Nie wybrano pliku.', r.picked ? 'ok' : '');
      await loadConfig();
    } catch (err) {
      notice(n, err.message, 'err');
    }
  });
  $('cookies-clear').addEventListener('click', () => saveConfig({ ytdlpCookies: null }, n, 'Plik cookies nie jest już używany.'));
  $('pixeldrain-form').addEventListener('submit', (e) => {
    e.preventDefault();
    saveConfig({ pixeldrainApiKey: $('set-pixeldrain').value }, n, $('set-pixeldrain').value.trim() ? 'Zapisano klucz pixeldrain.' : 'Usunięto klucz pixeldrain.');
  });
  $('set-trust').addEventListener('change', (e) =>
    saveConfig({ trustCloudflare: e.target.checked }, n, e.target.checked ? 'Adresy widzów będą brane z nagłówków Cloudflare.' : 'Nagłówki Cloudflare są ignorowane.'));
}

// ---------- Dziennik ----------
async function loadLog() {
  try {
    const r = await apiRetry('GET', '/api/desktop/log', undefined, { tries: 5 });
    const view = $('log-view');
    view.textContent = r.lines.slice(-200).join('\n') || 'Dziennik jest pusty.';
    view.scrollTop = view.scrollHeight;
  } catch (err) {
    notice($('log-notice'), err.message, 'err');
  }
}

function wireLog() {
  $('log-refresh').addEventListener('click', async () => {
    notice($('log-notice'), '');
    await loadLog();
  });
  $('log-copy').addEventListener('click', async (e) => {
    const button = e.currentTarget;
    const view = $('log-view');
    try {
      await navigator.clipboard.writeText(view.textContent);
    } catch {
      const range = document.createRange();
      range.selectNodeContents(view);
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      document.execCommand('copy');
    }
    button.textContent = 'Skopiowano';
    setTimeout(() => (button.textContent = 'Kopiuj'), 1500);
  });
}

// ---------- O programie ----------
function renderAbout() {
  $('about-version').textContent = `VRCinema, wersja ${info.version}`;
  const box = $('about-update');
  if (!updateOffer) return box.replaceChildren();
  box.replaceChildren(
    el('p', {}, `Dostępna jest wersja ${updateOffer.version}.`),
    updateOffer.notes ? el('p', { class: 'muted small' }, updateOffer.notes) : null,
    el('div', { class: 'row' },
      el('button', { type: 'button', class: 'fit', onclick: applyUpdate }, 'Zaktualizuj'),
      el('button', { type: 'button', class: 'quiet fit', onclick: skipUpdate }, 'Pomiń tę wersję')));
}

function wireAbout() {
  renderAbout();
  $('about-check').addEventListener('click', async (e) => {
    const button = e.currentTarget;
    const n = $('about-notice');
    button.disabled = true;
    notice(n, 'Sprawdzam…');
    try {
      const r = await api('GET', '/api/desktop/update');
      updateOffer = r.available;
      notice(n, r.available ? '' : `Masz najnowszą wersję (${r.current}). Jeśli nie ma połączenia z internetem albo repozytorium jest prywatne, program nic nie pokaże: szczegóły są w dzienniku.`, r.available ? '' : 'ok');
      info.version = r.current;
      bannerKey = '';
      renderBanners();
      renderAbout();
    } catch (err) {
      notice(n, err.message, 'err');
    } finally {
      button.disabled = false;
    }
  });
}

// ---------- Przeciąganie plików ----------
function wireDrop() {
  const overlay = el('div', { class: 'drop-overlay hidden', 'aria-hidden': 'true' }, el('div', {}, 'Upuść, aby dodać do kolejki'));
  document.body.append(overlay);
  const note = $('drop-notice');
  let timer = null;
  window.addEventListener('vrcinema:drag-enter', () => overlay.classList.remove('hidden'));
  window.addEventListener('vrcinema:drag-leave', () => overlay.classList.add('hidden'));
  window.addEventListener('vrcinema:files-added', (e) => {
    overlay.classList.add('hidden');
    const { added = 0, skipped = 0, error = null } = e.detail ?? {};
    const parts = [];
    if (error) {
      notice(note, error, 'err');
    } else {
      if (added > 0) parts.push(`Dodano ${added} ${plural(added, 'film', 'filmy', 'filmów')}.`);
      if (skipped > 0) parts.push(`Pominięto ${skipped} ${plural(skipped, 'plik', 'pliki', 'plików')} (to nie filmy albo nie da się ich odczytać).`);
      if (!parts.length) parts.push('Nie dodano żadnego filmu.');
      notice(note, parts.join(' '), added > 0 ? 'ok' : '');
    }
    clearTimeout(timer);
    timer = setTimeout(() => notice(note, ''), 10000);
    hooks.refreshQueue?.();
  });
}
