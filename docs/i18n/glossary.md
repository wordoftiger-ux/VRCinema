# VRCinema translation glossary

One fixed translation per concept and language. Every dictionary in `public/locales/<lang>.json`
uses these words; a translator who needs another word for a concept changes it here first and
then everywhere. The meaning of each term comes from the English pattern (`en.json`) and the
Polish texts (`pl.json`); `context.json` says where each key appears and how long it may be.

Languages: `en` (pattern), `pl`, `es`, `de`, `fr`, `ja` (beta).

## Style rules

Two audiences, two registers: the **owner's program** (window, remote panel, tray, log, wizard —
every key outside `viewer.*`) addresses the owner **formally**; the **request page** (`viewer.*`
keys, and the errors only that page shows: `errors.nick_reserved`, `errors.nick_taken`,
`errors.too_many_active`, `errors.too_many_requests`, `errors.viewer_lists_off`, `errors.list_full`,
`errors.duplicate_link`) addresses players **informally**. Errors
the panel shows too stay formal.

| | Address: program / request page | Register and form | Typography |
|---|---|---|---|
| **en** | "you" / "you" | Plain, short sentences; buttons are verbs ("Copy link"). | “ ” for quotes in the interface; `"` in log and console texts. |
| **pl** | "ty" / "ty" (informal, as today) | Impersonal forms about viewers: "dodane przez", "zgłoszone przez". | „ ” for quotes. |
| **es** | "usted" / "tú" (also the slates and the ticker, which players read: "Pide videos…") | Neutral Spanish for Latin America and Spain (no regional slang): "video", "agregar", "equipo" (this computer), "en vivo". Avoid gendered forms about viewers: "agregado por {name}", "solicitado por {name}" refer to the video. Infinitive on buttons ("Copiar enlace", "Reintentar"); progress as a gerund or an impersonal "se" form, never first person ("Conectando…", "Se están descargando los programas…", "Se abrirá la página de Cloudflare…", "el asistente le indicará cada paso"; the one accepted exception is the stream slate "En pausa — volvemos enseguida"); results as "No se pudo …" / "Falló …". "Le damos la bienvenida" instead of the gendered "Bienvenido". Hints that name a control quote its label exactly (“Solo sitios permitidos”, “Intervalo”, la vista “Canal”). | “ ” for quotes in the interface, `"` in log and console texts; ¿…? ¡…!; spaced em dash " — " only in short status lines (player bar, tray tooltip, slate title), otherwise a colon, semicolon or full stop; "p. ej.,"; space before "%" ("CPU 12 %"). |
| **de** | "Sie" / "du" | Impersonal forms about viewers ("hinzugefügt von {name}", "gewünscht von {name}"); infinitive on buttons ("Link kopieren"); progress and waiting as passive or noun forms, never first person ("Wird geladen…", "Wird verbunden…", "Internetverbindung wird hergestellt…", "Warten auf die Anmeldung…", "{url} wird heruntergeladen"); results as "… ist fehlgeschlagen", not "hat nicht geklappt". Generic masculine for roles (Zuschauer, Betreiber). Hints that name a control quote its label exactly („Nur erlaubte Websites“, „Intervall“, Ansicht „Kanal“). | „ “ for quotes; spaced en dash " – " (never the em dash "—"); "z. B."; space before units and "%" ("CPU 12 %"). |
| **fr** | "vous" / "tu" (also the slates and the ticker, which players read: "Ajoute une vidéo sur la page de demandes") | Impersonal forms about viewers; the participle agrees with the (feminine) video: "ajoutée par {name}", "demandée par {name}", "ajoutée par l’hôte". Infinitive on buttons ("Copier le lien", "Réessayer"); progress as a noun, never first person ("Connexion…", "Connexion à internet…", "Téléchargement de {url}", "Création du tunnel…", "En attente de la connexion…", "Le programme vérifie ce qui peut être utilisé", "l’assistant vous guide pas à pas"; the one accepted exception is the stream slate "En pause – à tout de suite"); results as "… a échoué" / "Impossible de …". "Bienvenue dans VRCinema" (no gendered form). Hints that name a control quote its label exactly (« Sites autorisés uniquement », « Intervalle », la vue « Chaîne »). | « » for quotes in the interface, `"` in log and console texts; a no-break space U+00A0 (not the narrow U+202F, which the stream font and older fonts may lack) before : ; ? ! » and % and after «, and between a number and its unit ("10 s", "2 h 5 min", "84 Mo"); ’ for the apostrophe; spaced en dash " – " (never the em dash) only in short status lines (player bar, tray tooltip, slate title) and the log line "Tunnel : en ligne – {url}", otherwise a colon, semicolon or full stop; "p. ex."; Ko/Mo for kB/MB, "i/s" for fps. |
| **ja** (beta) | Polite です/ます (both); the request page, slates and ticker in a softer polite tone ("リクエストページから動画を追加できます", "最初の動画を追加してみましょう"), never the plain だ/である form | です/ます for sentences; noun phrases on buttons, labels and headings (「コピー」「スキップ」「次へ」), switches end in 〜する ("テロップを表示する"); no personal pronouns ("Your nick" → "ニックネーム"); English first-person progress becomes a subject-less statement ("確認しています", "Cloudflareのページが開きます", "ウィザードが1ステップずつ案内します"); impersonal "追加：{name}" / "リクエスト：{name}" about viewers; katakana for loanwords in Microsoft style with the long vowel mark (サーバー, ブラウザー, プレーヤー, フォルダー, エンコーダー, ユーザー, バッファー; but エントリ, メモリ). Hints that name a control quote its label exactly (「許可したサイトのみ」, 「一定間隔」, 「チャンネル」ビューの「承認待ち」). | Full-width 、。：？（）in Japanese text, 「」 for quoted labels and video titles (also in the log); no punctuation at the end of buttons; no space between Japanese and Latin letters or numbers ("Cloudflareに接続", "10秒"), a space between a number and a Latin unit ("84 MB", "30 fps"); ～ for ranges; no dashes (the English " — " becomes 。, ： or parentheses); "…" for progress ("接続中…"); 秒/分/時間, counters 本 (videos), 件 (items, messages, files), 台, 回. Settings paths in parentheses: （設定 → ツール）. Where the interface joins two sentences, Japanese gets no space after 。 (`sentences()` in `public/app/i18n.js`). The ticker starts with ▶ (Yu Gothic has no ►; the engine also writes ▶ for any language whose ticker is drawn with the Japanese font). Half-width parentheses only where Windows writes them ("元に戻す (縮小)") and in the endonym "日本語 (beta)". |

Common to all languages:

- Keep every `{field}` exactly as in English; plural entries carry every CLDR category the
  language needs (`en`/`de`: one, other; `es`/`fr`: one, many, other; `pl`: one, few, many,
  other; `ja`: other).
- Never translate program and product names: VRCinema, VRChat, ProTV, Cloudflare, cloudflared,
  yt-dlp, ffmpeg, ffprobe, Deno, pixeldrain, GitHub, Homebrew; setting names (ADMIN_PASSWORD,
  MEDIA_DIRS, FONT_FILE, LANGUAGE, …), API fields and values (action, itemId, toS, byS, quick,
  named, true/false), file names (.env, YuGothM.ttc …) and technical tags (EXT-X-DISCONTINUITY,
  HLS, PTS, PID, DNS, QR).
- Names in the VRChat menu (Allow Untrusted URLs, Settings, Comfort & Safety) stay in English.
- Texts marked "the same English text in every language (API compatibility)" stay English.
- The language names (`settings.language_name.*`) are endonyms, the same in every dictionary.
- Log entries keep the leading spaces of the start-up banner and align the addresses (column 26).
- Respect the length limits of `context.json` (rail labels ~12–13, status pills ~16, slate
  titles ~30, slate subtitles ~40 characters); the engine test checks them for every complete
  language.

## Terms

| Concept (meaning) | en | pl | es | de | fr | ja |
|---|---|---|---|---|---|---|
| Channel — the one live stream the program broadcasts | channel | kanał | canal | Kanal | chaîne | チャンネル |
| Channel link — the .m3u8 address pasted into the VRChat player | channel link | link kanału | enlace del canal | Kanal-Link | lien de la chaîne | チャンネルのリンク |
| Queue — videos waiting to be played | queue | kolejka | cola | Warteschlange | file d’attente | キュー |
| Up next — heading of the queue panel | Up next | Następne | A continuación | Als Nächstes | À suivre | 次に再生 |
| Slate — still screen the stream shows between videos (idle, buffering, pause) | slate | plansza | pantalla | Tafel (Wartetafel, Pausentafel) | écran d’attente | 待機画面 |
| Slate language — language of slates and ticker | language of the slates | język plansz | idioma de las pantallas | Sprache der Tafeln | langue des écrans d’attente | 待機画面の言語 |
| Info ticker — text band burned into the stream | info ticker | pasek informacyjny | barra informativa | Laufband | bandeau d’information | テロップ |
| Message (owner's text in the ticker) | message | wiadomość | mensaje | Nachricht | message | メッセージ |
| Interval (ticker mode "from time to time") | interval | odstęp | intervalo | Intervall | intervalle | 間隔 |
| Progress line (ticker) | progress line | pasek postępu | línea de progreso | Fortschrittslinie | barre de progression | 進行状況バー |
| Tunnel — built-in Cloudflare connection | tunnel | tunel | túnel | Tunnel | tunnel | トンネル |
| Quick tunnel / quick link — random trycloudflare.com address | quick tunnel / quick link | szybki tunel / szybki link | túnel rápido / enlace rápido | Schnelltunnel / schneller Link | tunnel rapide / lien rapide | クイックトンネル / クイックリンク |
| Permanent link — own domain via a named tunnel | permanent link | stały link | enlace permanente | dauerhafter Link | lien permanent | 固定リンク |
| Public address | public address | adres publiczny | dirección pública | öffentliche Adresse | adresse publique | 公開アドレス |
| Manual address — typed by hand with the tunnel off | manual address | adres ręczny | dirección manual | eigene Adresse | adresse manuelle | 手動アドレス |
| Request page — page where viewers add videos | request page | strona zgłoszeń | página de solicitudes | Wunschseite | page de demandes | リクエストページ |
| Request — a viewer's video sent from the request page | request | zgłoszenie | solicitud | Wunsch | demande | リクエスト |
| Requested by / added by (impersonal) | requested by / added by | zgłoszone przez / dodane przez | solicitado por / agregado por | gewünscht von / hinzugefügt von | demandée par / ajoutée par | リクエスト：{name} / 追加：{name} |
| Viewer | viewer | widz | espectador | Zuschauer | spectateur | 視聴者 |
| Owner — the person running the program | owner | właściciel | propietario | Betreiber | propriétaire | オーナー |
| Host — the owner's badge and reserved nick (must match `engine/src/nicks.rs`) | Host | Gospodarz | Anfitrión | Gastgeber | Hôte | ホスト |
| Added by the host (request page, `viewer.added_by_host`; the program's own queue says "added here", `queue.by_host`) | added by the host | dodane przez gospodarza | agregado por el anfitrión | vom Gastgeber hinzugefügt | ajoutée par l’hôte | 追加：ホスト (queue.by_host: パネルから追加) |
| Nick — viewer's chosen name | nick | nick | apodo | Nick | pseudo | ニックネーム |
| Device — a viewer's browser (device key) | device | urządzenie | dispositivo | Gerät | appareil | デバイス |
| Block / unblock (a device) | block / unblock | zablokuj / odblokuj | bloquear / desbloquear | sperren / entsperren | bloquer / débloquer | ブロック / ブロック解除 |
| Release (a nick) | release | zwolnij | liberar | freigeben | libérer | 解放 |
| Anonymous | anonymous | anonimowo | anónimo | anonym | anonyme | 匿名 |
| Approval / approve / reject (moderation) | approval / approve / reject | zatwierdzanie / zatwierdź / odrzuć | aprobación / aprobar / rechazar | Freigabe / freigeben / ablehnen | validation / valider / refuser | 承認 / 承認 / 却下 |
| Waiting for approval | Waiting for approval | Do zatwierdzenia | Pendiente de aprobación | Wartet auf Freigabe | En attente de validation | 承認待ち |
| Allowed sites (allowlist) | allowed sites | dozwolone serwisy | sitios permitidos | erlaubte Websites | sites autorisés | 許可したサイト |
| Blocked sites (blocklist) | blocked sites | zablokowane serwisy | sitios bloqueados | gesperrte Websites | sites bloqués | ブロックしたサイト |
| Site (a website a link comes from; not a page of the program) | site | serwis | sitio | Website | site | サイト |
| Limits (for viewers) | limits | limity | límites | Limits | limites | 制限 |
| Pause / resume | pause / resume | wstrzymaj / wznów | pausar / reanudar | pausieren / fortsetzen | mettre en pause / reprendre | 一時停止 / 再開 |
| Paused | paused | wstrzymane | en pausa | pausiert | en pause | 一時停止中 |
| Pause pending — the owner paused at a frame of the Preview the viewers have not got yet; On air plays to the end of that segment, then pauses | Pause pending | Wstrzymywanie | Pausa pendiente | Pause angefordert | Pause demandée | 一時停止待ち |
| Seek (jump within the video), back / forward 10 s | seek, back / forward | skok, wstecz / naprzód | salto, retroceder / adelantar | springen (spulen), zurück / vor | se déplacer, reculer / avancer | シーク、戻る / 進む (10秒戻る / 10秒進む) |
| Skip / next video | skip / next video | pomiń / następny film | omitir / siguiente video | überspringen / nächstes Video | passer / vidéo suivante (state: interrompue) | スキップ / 次の動画 (state: スキップ済み) |
| From the beginning | From the beginning | Od początku | Desde el principio | Von vorn | Depuis le début | 最初から |
| Play now | Play now | Odtwórz teraz | Reproducir ahora | Jetzt abspielen | Lire maintenant | 今すぐ再生 |
| Play next — queue a video right after the one on air | Play next | Odtwórz jako następny | Reproducir a continuación | Als Nächstes abspielen | Lire ensuite | 次に再生 |
| Clear the queue — remove every waiting video | Clear queue | Wyczyść kolejkę | Vaciar la cola | Warteschlange leeren | Vider la file | キューを空にする |
| Undo (a removal, for a few seconds) | Undo | Cofnij | Deshacer | Rückgängig | Annuler | 元に戻す |
| On air — also the caption of the right screen of the Channel view (the stream as the viewers get it) | on air / On air | na antenie / Na antenie | en emisión / En emisión | auf Sendung (laufendes Video) / Auf Sendung | à l’antenne / À l’antenne | 放送中 (what the channel broadcasts now) |
| Live stream | live stream | transmisja na żywo | transmisión en vivo (tag: EN VIVO) | Livestream | direct (tag: EN DIRECT) | ライブ配信 (only a live source; tag: ライブ) |
| History | History | Historia | Historial | Verlauf | Historique | 履歴 |
| Preview — the left screen of the Channel view: the newest material the server has, before it airs (the producer's monitor; key S syncs it with on-air) | Preview | Podgląd | Vista previa | Vorschau | Aperçu | プレビュー |
| Stats for nerds — technical overlay (as on YouTube) | Stats for nerds | Statystyki dla nerdów | Estadísticas para nerds | Statistiken für Nerds | Statistiques avancées | 詳細統計情報 |
| Share (view) | Share | Udostępnij | Compartir | Teilen | Partager | 共有 |
| Settings | Settings | Ustawienia | Configuración | Einstellungen | Paramètres | 設定 |
| Tools — yt-dlp, ffmpeg, Deno, cloudflared | tools | narzędzia | herramientas | Werkzeuge | outils | ツール |
| Encoder / conversion (re-encoding the picture) | encoder / conversion | koder / konwersja | codificador / conversión | Encoder / Konvertierung | encodeur / conversion | エンコーダー / 変換 (stream copy: ストリームコピー) |
| Graphics card | graphics card | karta graficzna | tarjeta gráfica | Grafikkarte | carte graphique | グラフィックボード (Intel: Intelのグラフィックス) |
| Processor | processor | procesor | procesador | Prozessor | processeur | CPU |
| Panel — the program's interface (window or browser) | panel | panel | panel | Panel | panneau | パネル |
| Remote panel — signing in from another device with a password | remote panel | panel zdalny | panel remoto | Fernzugriff | panneau à distance | リモートパネル |
| Wizard — first-run setup and the permanent link setup | wizard | kreator | asistente | Assistent | assistant | ウィザード |
| Log | log | dziennik | registro | Protokoll | journal | ログ |
| Tray / notification area | notification area (tray) | obszar powiadomień (zasobnik) | área de notificación | Infobereich | zone de notification | 通知領域 |
| Home network | home network | sieć domowa | red doméstica | Heimnetz | réseau domestique | ホームネットワーク |
| Update | update | aktualizacja | actualización | Update / aktualisieren | mise à jour | アップデート (VRCinema) / 更新 (a tool) |
| Sign in / sign out | sign in / sign out | zaloguj / wyloguj | iniciar sesión / cerrar sesión | anmelden / abmelden | se connecter / se déconnecter | ログイン / ログアウト |
| Video | video | film | video | Video | vidéo | 動画 |
| Link | link | link | enlace | Link | lien | リンク |
| Playlist — a site's list of videos (YouTube, pixeldrain) | playlist | playlista | lista de reproducción | Playlist | playlist | 再生リスト |
| List — several links a viewer sends at once (one per line, or a playlist), approved as one group | list | lista | lista | Liste | liste | リスト |
| QR code | QR code | kod QR | código QR | QR-Code | code QR | QRコード |
| Buffering (slate title) | Buffering… | Buforowanie… | Cargando… | Wird geladen… | Chargement… | 読み込み中… |
| Online / no internet (status pill) | Online / No internet | W sieci / Brak internetu | En línea / Sin internet | Online / Kein Internet | En ligne / Pas d’internet (connecting: Connexion…) | オンライン / インターネットなし (connecting: 接続中) |
| Keyboard keys | Ctrl, Shift, Home, Space | Ctrl, Shift, Home, Spacja | Ctrl, Mayús, Inicio, Espacio | Strg, Umschalt, Pos1, Leertaste | Ctrl, Maj, Origine, Espace | Ctrl, Shift, Home, スペース |
| Window buttons (as Windows names them) | Minimize, Maximize, Restore Down, Close | Minimalizuj, Maksymalizuj, Przywróć w dół, Zamknij | Minimizar, Maximizar, Restaurar hacia abajo, Cerrar | Minimieren, Maximieren, Verkleinern, Schließen | Réduire, Agrandir, Niveau inférieur, Fermer | 最小化, 最大化, 元に戻す (縮小), 閉じる |

## Notes per language

- **de:** "freigeben" is used both for approving a request (Freigabe) and for releasing a nick;
  the two buttons never appear together. "Fernzugriff" names the remote panel section and the
  feature ("Passwort für den Fernzugriff"); the interface itself is "Panel". "Website" is a
  site a link comes from (erlaubte/gesperrte Websites, "Download von Websites"); "Seite" only
  for pages of the program and Cloudflare's login page; "Wunschseite" is the request page.
  "Übergänge" for discontinuities (stats), "Stream-Kopie" for copying without conversion.
  Request page in du-form: "Dein Nick", "Füge einen Link … ein", "So schaust du in VRChat zu".
- **es:** neutral Spanish: "video" (not "vídeo"), "agregar" (not "añadir"), "equipo" for the
  computer ("este equipo", as Windows says), "en vivo" (not "en directo"), "Configuración" (as
  the Windows Settings app; the rail breaks it as "Configu-ración" with a soft hyphen).
  "Solicitud" is a viewer's request and "página de solicitudes" the request page; a technical
  request of the API is also "solicitud" ("Solicitud no válida"). "Omitir" is skipping a video
  (Omitido, Omitir video); "salto" / "retroceder" / "adelantar" is seeking within one. "Sitio"
  is a site a link comes from ("sitios permitidos", "descarga desde sitios web"); "página" a
  page of the program or Cloudflare's login page. "Pantalla" is a slate ("pantalla de espera"),
  "barra informativa" the ticker, "copia directa" copying without conversion (stats and log),
  "codificador" the encoder, "propietario" the owner, "anfitrión" the Host (lower case in running
  text; `engine/src/nicks.rs` reserves "Anfitrión"). The slates and the ticker speak to players
  in tú like the request page ("Pide videos en la página de solicitudes", "pide un video: …").
  Request page in tú-form: "Tu apodo", "Pega el enlace de un video", "Agrega el primer video".
  "Comando" for a command of the channel or the console ("Falló un comando del canal",
  "Comando desconocido"), not "orden". "Tipo de letra" for a font (the slate and ticker font
  in the log); "fuente" only for the source of a video ("La fuente no tiene pista de video",
  stats row "Fuente"). Numbers are formatted by the interface (`Intl`) for the es locale, so
  they come out Spain-style ("1.482,9", "1,9×"); informational only, nothing to translate.
- **fr:** "chaîne" is the channel, "file d’attente" the queue ("Ajouter à la file" on short
  buttons), "écran d’attente" a slate (in the interface and the log; never a bare "écran"),
  "bandeau d’information" the ticker ("bandeau" for short), "barre de progression" its progress line, "page de demandes" the request page and "demande" a viewer's request; a technical
  request of the API is "requête" ("Requête non valide"). "Site" (or "site web") is a site a link
  comes from ("sites autorisés", "téléchargement depuis les sites web"); "page" a page of the
  program or Cloudflare's login page. "Passer" is the action of skipping a video ("Passer la vidéo"); the history state of a
  skipped video is "Interrompue" ("Arrêtée par le propriétaire"; log "passage à la vidéo suivante"); "se déplacer" / "reculer" / "avancer" is seeking within
  one. "Hôte" is the Host (lower case in running text, "ajoutée par l’hôte"; `engine/src/nicks.rs`
  reserves "Hôte"), "propriétaire" the owner, "pseudo" the nick, "valider" / "refuser" approving
  and rejecting a request. "Commande" for a command of the channel or the console; "police" for
  a font (slate and ticker font in the log), "source" only for the source of a video. "Lire" / "lecture" only for playback; fetching or
  reading a link (yt-dlp, pixeldrain, ffprobe) is "récupérer" / "analyser" ("Impossible de récupérer
  la vidéo", "impossible d’analyser la source"). Before a {nick} field, no article to elide:
  "l’appareil utilisé par {nick}" (not "de {nick}"). Stats overlay: "Statistiques avancées".
  Intel graphics like AMD/NVIDIA: "carte graphique Intel (Quick Sync)". "p. ex." instead of "par
  exemple" in parentheses; "…" (not "...") in log texts too.
  "Copie directe" is copying without conversion (stats and log), "encodeur" the encoder,
  "panneau" the panel and "panneau à distance" the remote panel, "journal" the log, "zone de
  notification" the tray. "Internet" in lower case in running text, capitalised only as the name
  of the Settings section and the Share card. Program register "vous", request page, slates and
  ticker "tu" ("Ton pseudo", "Colle le lien d’une vidéo", "Ajoute une vidéo"). Numbers are
  formatted by the interface (`Intl`) for the fr locale ("1 482,9" with a narrow no-break space,
  "1,9×"); informational only, nothing to translate. The log banner labels fit the column 26
  ("  Chaîne (lien local) :").
- **ja (beta):** loanwords in katakana as in the table (キュー, トンネル, テロップ, ニックネーム); the
  host's word ホスト must stay as in `engine/src/nicks.rs`. "放送中" is what the channel broadcasts (on air), "ライブ配信" only a live source (a live stream in
  the queue), "ストリーム" the HLS output of the channel; "映像" the picture. "待機画面" is every slate (idle,
  buffering, pause; never a bare "画面"), "テロップ" the ticker, "進行状況バー" its progress line.
  "リクエスト" is a viewer's request and "リクエストページ" the request page; a technical request of
  the API is "要求" ("無効な要求です。"). "サイト" is a site a link comes from; "ページ" a page of the
  program or Cloudflare's login page. Reading a link (yt-dlp, pixeldrain) is "取得"
  ("動画の情報を取得できませんでした"), analysing a source "解析"; "再生" only for playback. "スキップ" is skipping a
  video (history state "スキップ済み"); "シーク" / "戻る" / "進む" is moving within one. The queue
  headings are "次に再生" (panel), "承認待ち" and "再生待ち". "PC" for this computer (as Windows),
  "CPU" for the processor, "グラフィックボード" for AMD/NVIDIA and "Intelのグラフィックス" for Intel.
  "再生リスト" for a playlist (YouTube Japan). "アップデート" is VRCinema's own new version (About),
  "更新" updating a tool or refreshing the log. Steps are "ステップ{n}/{total}". Settings sections as
  in the rail and the list: 全般, インターネット, チャンネル, ツール, リモートパネル, 詳細設定, ログ, バージョン情報.
  The log banner labels end before display column 26 counting full-width characters as two
  columns ("  ローカルリンク：" for the channel's local link). The interface names Yu Gothic UI /
  Meiryo UI for Japanese (`:lang(ja)` in `public/app/theme.css`).
