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

  $('channel-hint').textContent = 'Ten link wklejasz do odtwarzacza w VRChat. Adres publiczny pojawia się sam, gdy kanał połączy się z internetem (Ustawienia, sekcja Internet).';
  wireTabs();
  wireNet();
  wireGeneral();
  wireInternet();
  wireRemote();
  wireTools();
  wireAdvanced();
  wireLog();
  wireAbout();
  wireDrop();
  renderBanners();
  setInterval(refreshInfo, 3000);
  if (location.hash === '#internet') {
    showTab('settings');
    $('section-internet').scrollIntoView();
  }
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
  renderNet();
  renderInternet();
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
  renderNet();
  renderInternet();
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
  renderInternet();
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

// ---------- Znacznik połączenia (górna belka panelu) ----------

const NET_LOST = 'Brak internetu — kanał wstrzymany';

/**
 * Stan połączenia z internetem z odpowiedzi GET /api/desktop/info: rodzaj (kolor kropki), napis, opis
 * i link kanału do skopiowania (albo null, gdy kanał nie ma adresu publicznego).
 */
export function netBadge(i) {
  const t = i.tunnel ?? { state: 'off' };
  const reasons = i.pauseReasons ?? [];
  const lostPause = Boolean(i.paused) && reasons.includes('tunnel');
  const viewersPause = Boolean(i.paused) && reasons.includes('no_viewers');
  const link = (base) => `${base}/live/main.m3u8`;
  if (t.state === 'online') {
    return { kind: 'online', text: 'W sieci', detail: viewersPause ? 'Kanał wstrzymany — nikt nie ogląda. Wznowi się, gdy ktoś zacznie oglądać.' : '', link: link(t.url) };
  }
  if (t.state !== 'off' && lostPause) {
    return { kind: 'offline', text: NET_LOST, detail: t.message || 'Film wznowi się od tego samego miejsca, gdy wróci połączenie.', link: null };
  }
  switch (t.state) {
    case 'connecting':
      return { kind: 'connecting', text: 'Łączenie', detail: 'Łączę kanał z internetem…', link: null };
    case 'reconnecting':
      return { kind: 'connecting', text: 'Łączenie', detail: 'Połączenie z internetem przerwane — łączę ponownie…', link: null };
    case 'error':
      return { kind: 'offline', text: 'Brak internetu', detail: t.message || 'Nie udało się połączyć z internetem.', link: null };
    default:
      if (i.publicUrl) return { kind: 'online', text: 'W sieci', detail: 'Adres wpisany ręcznie (tunel jest wyłączony).', link: link(i.publicUrl) };
      return { kind: 'off', text: 'Tylko lokalnie', detail: 'Tunel jest wyłączony — kanał widać tylko na tym komputerze. Włączysz go w Ustawieniach, w sekcji Internet.', link: null };
  }
}

let netLink = null;

function renderNet() {
  const b = netBadge(info);
  $('net-badge').dataset.kind = b.kind;
  $('net-text').textContent = b.text;
  $('net-detail').textContent = b.detail;
  netLink = b.link;
  $('net-copy').classList.toggle('hidden', !b.link);
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const field = el('textarea', { style: 'position:fixed;opacity:0' });
    field.value = text;
    document.body.append(field);
    field.select();
    document.execCommand('copy');
    field.remove();
  }
}

function wireNet() {
  $('net-copy').addEventListener('click', async (e) => {
    if (!netLink) return;
    const button = e.currentTarget;
    await copyText(netLink);
    button.textContent = 'Skopiowano';
    setTimeout(() => (button.textContent = 'Kopiuj link'), 1500);
  });
  renderNet();
}

// ---------- Internet (tunel, stały link, wstrzymywanie) ----------

const TUNNEL_STATE_TEXT = {
  off: 'Tunel jest wyłączony.',
  connecting: 'Łączenie z internetem…',
  online: 'W sieci.',
  reconnecting: 'Połączenie przerwane — łączę ponownie…',
  error: 'Brak internetu.',
};
const NAMED_HELP = 'Stały link pozwala mieć ten sam adres po każdym uruchomieniu programu. Potrzebujesz darmowego konta Cloudflare z dodaną własną domeną.';

let internetKey = '';
let confirmDelete = false;
let namedRun = 0; // numer kreatora: zmiana przerywa pętle odpytywania poprzedniego

function renderInternet() {
  if (!config) return;
  const t = info.tunnel ?? { state: 'off' };
  const on = config.tunnelEnabled;
  $('set-tunnel').checked = on;
  $('set-pause-loss').checked = config.pauseOnTunnelLoss;
  $('set-pause-viewers').checked = config.pauseWhenNoViewers;
  $('tunnel-on').classList.toggle('hidden', !on);
  notice($('tunnel-warning'), on && t.warning ? t.warning : '', 'err');
  // Ręczny adres i zaufanie do Cloudflare mają sens tylko bez tunelu.
  $('public-url-form').classList.toggle('hidden', on);
  $('trust-row').classList.toggle('hidden', on);
  $('manual-hint').classList.toggle('hidden', !on);

  const named = config.tunnelMode === 'named';
  const host = config.namedTunnelHostname;
  const key = JSON.stringify([on, t.state, t.url, t.message, named, host, confirmDelete]);
  if (key === internetKey) return;
  internetKey = key;

  $('tunnel-state').textContent = TUNNEL_STATE_TEXT[t.state] ?? t.state;
  $('tunnel-address').textContent = t.url ? `Adres kanału: ${t.url}` : t.message ? `Powód: ${t.message}` : '';
  $('tunnel-mode-text').textContent = named && host
    ? `Stały link: https://${host}. Adres się nie zmienia.`
    : 'Szybki link: adres jest losowy (trycloudflare.com) i zmienia się przy każdym uruchomieniu programu.';

  const actions = [];
  if (confirmDelete) {
    actions.push(
      el('p', { class: 'small', style: 'flex:1 1 100%;margin:0' }, 'Usunąć stały link? Tunel zostanie usunięty z konta Cloudflare, a certyfikat i poświadczenia z tego komputera. Jeśli rekord DNS tej nazwy zostanie na koncie, usuń go ręcznie w panelu Cloudflare.'),
      el('button', { type: 'button', class: 'danger fit', onclick: () => namedAction('delete') }, 'Usuń stały link'),
      el('button', { type: 'button', class: 'quiet fit', onclick: () => { confirmDelete = false; renderInternet(); } }, 'Anuluj'));
  } else if (named) {
    actions.push(el('button', { type: 'button', class: 'quiet fit', onclick: () => namedAction('quick') }, 'Wróć do szybkiego tunelu'));
  } else if (host) {
    actions.push(el('button', { type: 'button', class: 'quiet fit', onclick: backToNamed }, `Wróć do stałego linku (${host})`));
  } else {
    actions.push(el('button', { type: 'button', class: 'fit', onclick: openNamedWizard }, 'Ustaw stały link'));
  }
  if (!confirmDelete && host) {
    actions.push(el('button', { type: 'button', class: 'danger fit', onclick: () => { confirmDelete = true; renderInternet(); } }, 'Usuń stały link'));
  }
  $('tunnel-mode-actions').replaceChildren(...actions);
}

function wireInternet() {
  const n = $('internet-notice');
  $('set-tunnel').addEventListener('change', (e) =>
    saveConfig({ tunnelEnabled: e.target.checked }, n, e.target.checked ? 'Tunel jest włączony — łączę z internetem.' : 'Tunel jest wyłączony. Kanał działa tylko lokalnie albo pod adresem z sekcji Zaawansowane.'));
  $('set-pause-loss').addEventListener('change', (e) =>
    saveConfig({ pauseOnTunnelLoss: e.target.checked }, n, e.target.checked ? 'Kanał będzie wstrzymany, gdy zniknie internet.' : 'Kanał nie będzie wstrzymywany po utracie internetu.'));
  $('set-pause-viewers').addEventListener('change', (e) =>
    saveConfig({ pauseWhenNoViewers: e.target.checked }, n, e.target.checked ? 'Kanał będzie wstrzymany, gdy nikt nie ogląda.' : 'Kanał nie będzie wstrzymywany, gdy nikt nie ogląda.'));
  $('tunnel-restart').addEventListener('click', async (e) => {
    const button = e.currentTarget;
    button.disabled = true;
    notice(n, 'Łączę ponownie…');
    try {
      await api('POST', '/api/desktop/tunnel/restart');
      notice(n, 'Tunel uruchomiony ponownie. Szybki link dostaje wtedy nowy adres.', 'ok');
      await settle();
    } catch (err) {
      notice(n, err.message, 'err');
    } finally {
      button.disabled = false;
    }
  });
}

async function backToNamed() {
  await saveConfig({ tunnelMode: 'named' }, $('internet-notice'), 'Działa stały link.');
}

/** „Wróć do szybkiego tunelu” (`quick`) i „Usuń stały link” (`delete`). */
async function namedAction(kind) {
  const n = $('internet-notice');
  notice(n, kind === 'delete' ? 'Usuwam stały link…' : 'Przełączam na szybki tunel…');
  try {
    const r = await api('POST', `/api/desktop/tunnel/named/${kind}`);
    notice(n, r.message, 'ok');
  } catch (err) {
    notice(n, err.message, 'err');
  }
  confirmDelete = false;
  closeNamedWizard();
  await Promise.all([loadConfig(), settle()]);
}

// ----- Kreator stałego linku -----

const wizardStep = () => $('named-step');
const namedNotice = (text, kind) => notice($('named-notice'), text, kind);

function closeNamedWizard() {
  namedRun++;
  $('named-wizard').classList.add('hidden');
  wizardStep().replaceChildren();
  namedNotice('');
}

async function openNamedWizard() {
  const run = ++namedRun;
  $('named-wizard').classList.remove('hidden');
  $('named-wizard').scrollIntoView({ block: 'nearest' });
  namedNotice('');
  let p;
  try {
    p = await apiRetry('GET', '/api/desktop/tunnel/named/progress');
  } catch (err) {
    return namedNotice(err.message, 'err');
  }
  if (run !== namedRun) return;
  if (p.running && p.operation === 'login') return watchLogin(run);
  if (p.loggedIn) return stepHost(run);
  stepLogin(run);
}

function stepLogin(run, retry = false) {
  wizardStep().replaceChildren(
    el('p', {}, NAMED_HELP),
    el('p', {}, 'Krok 1 z 2. Otworzę stronę Cloudflare, na której zalogujesz się i wybierzesz domenę dla kanału.'),
    el('div', { class: 'row' },
      el('button', { type: 'button', class: 'fit', onclick: () => startLogin(run) }, retry ? 'Spróbuj ponownie' : 'Połącz z Cloudflare'),
      el('button', { type: 'button', class: 'quiet fit', onclick: closeNamedWizard }, 'Anuluj')));
}

async function startLogin(run) {
  namedNotice('');
  try {
    await api('POST', '/api/desktop/tunnel/named/login');
  } catch (err) {
    // „busy”: logowanie już trwa — tylko je obserwujemy.
    if (err.data?.code !== 'busy') return failedLogin(run, err.message);
  }
  watchLogin(run);
}

function failedLogin(run, message, helpUrl) {
  if (run !== namedRun) return;
  stepLogin(run, true);
  namedNotice(message, 'err');
  if (helpUrl) $('named-notice').append(' ', el('a', { href: helpUrl, target: '_blank', rel: 'noopener' }, 'Jak to zrobić'));
}

async function watchLogin(run) {
  const msg = el('p', { class: 'notice', role: 'status' }, 'Uruchamiam logowanie…');
  const link = el('p', { class: 'hidden small' });
  wizardStep().replaceChildren(
    el('p', {}, 'Krok 1 z 2. Zaloguj się w Cloudflare i wybierz domenę w otwartym oknie przeglądarki. Masz na to 5 minut.'),
    msg, link,
    el('div', { class: 'row' },
      el('button', { type: 'button', class: 'quiet fit', onclick: cancelLogin }, 'Anuluj')));
  let opened = false;
  let bad = 0;
  for (;;) {
    await sleep(1000);
    if (run !== namedRun) return;
    let p;
    try {
      p = await api('GET', '/api/desktop/tunnel/named/progress');
      bad = 0;
    } catch (err) {
      if (++bad > 20) return failedLogin(run, err.message);
      continue;
    }
    if (run !== namedRun) return;
    msg.textContent = p.message || 'Czekam na logowanie…';
    if (p.loginUrl) {
      link.classList.remove('hidden');
      link.replaceChildren('Jeśli strona się nie otworzyła: ', el('a', { href: p.loginUrl, target: '_blank', rel: 'noopener' }, 'otwórz stronę logowania'));
      // Gdy cloudflared sam otworzył przeglądarkę, okno tego nie powtarza.
      if (p.browserOpened === false && !opened) {
        opened = true;
        window.open(p.loginUrl, '_blank', 'noopener');
      }
    }
    if (p.running) continue;
    if (p.state === 'done' || p.loggedIn) return stepHost(run);
    const text = p.state === 'cancelled' ? 'Logowanie przerwane.' : p.message || 'Logowanie się nie udało.';
    return failedLogin(run, text, p.helpUrl);
  }
}

async function cancelLogin() {
  try {
    await api('POST', '/api/desktop/tunnel/named/login', { cancel: true });
  } catch {
    // postęp pokaże, co się stało
  }
}

function stepHost(run, previous = '') {
  const host = el('input', { type: 'text', id: 'named-host', placeholder: 'kanal.twojadomena.pl', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', required: true, value: previous });
  const form = el('form', { class: 'stack' },
    el('p', {}, 'Krok 2 z 2. Wpisz pełny adres, pod którym ma działać kanał. Domena musi być na Twoim koncie Cloudflare.'),
    el('div', { class: 'row' },
      el('label', { class: 'grow' }, 'Adres stałego linku (np. kanal.twojadomena.pl)', host),
      el('button', { type: 'submit', class: 'fit' }, 'Utwórz stały link')),
    el('div', { class: 'row' }, el('button', { type: 'button', class: 'quiet fit', onclick: closeNamedWizard }, 'Anuluj')));
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    createNamed(run, host.value.trim().replace(/^https?:\/\//i, '').replace(/\/.*$/, ''), false, form);
  });
  wizardStep().replaceChildren(form);
  host.focus({ preventScroll: true });
}

async function createNamed(run, hostname, overwrite, form) {
  form.querySelectorAll('button, input').forEach((c) => (c.disabled = true));
  namedNotice('Tworzę stały link…');
  let working = true;
  (async () => {
    while (working && run === namedRun) {
      await sleep(1000);
      try {
        const p = await api('GET', '/api/desktop/tunnel/named/progress');
        if (working && p.running && p.message) namedNotice(p.message);
      } catch {
        // chwilowo bez odpowiedzi
      }
    }
  })();
  try {
    const r = await api('POST', '/api/desktop/tunnel/named/create', { hostname, overwrite });
    working = false;
    if (run !== namedRun) return;
    wizardStep().replaceChildren(el('p', {}, `Gotowe. Kanał ma teraz stały adres: https://${r.tunnel?.hostname ?? hostname}`));
    namedNotice(r.message, 'ok');
    await Promise.all([loadConfig(), settle()]);
  } catch (err) {
    working = false;
    if (run !== namedRun) return;
    const code = err.data?.code;
    if (code === 'not_logged_in') {
      stepLogin(run, true);
      return namedNotice(err.message, 'err');
    }
    if (code === 'dns_taken') {
      namedNotice('');
      return wizardStep().replaceChildren(
        el('p', { class: 'notice err', role: 'alert' }, err.message),
        el('p', {}, `Nazwa ${hostname} ma już w Cloudflare inny rekord DNS. Nadpisać go, żeby wskazywał na ten kanał?`),
        el('div', { class: 'row' },
          el('button', { type: 'button', class: 'fit', onclick: (e) => createNamed(run, hostname, true, e.currentTarget.closest('#named-step')) }, 'Nadpisz rekord'),
          el('button', { type: 'button', class: 'quiet fit', onclick: () => { namedNotice(''); stepHost(run, hostname); } }, 'Wybierz inną nazwę')));
    }
    stepHost(run, hostname);
    namedNotice(err.message, 'err');
    if (code === 'no_domain' && err.data?.helpUrl) {
      $('named-notice').append(' ', el('a', { href: err.data.helpUrl, target: '_blank', rel: 'noopener' }, 'Jak dodać domenę'));
    }
  }
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
    saveConfig({ publicUrl: $('set-public-url').value }, n, 'Zapisano adres publiczny.');
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
