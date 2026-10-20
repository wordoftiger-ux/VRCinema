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
`errors.too_many_active`, `errors.too_many_requests`) addresses players **informally**. Errors
the panel shows too stay formal.

| | Address: program / request page | Register and form | Typography |
|---|---|---|---|
| **en** | "you" / "you" | Plain, short sentences; buttons are verbs ("Copy link"). | “ ” for quotes in the interface; `"` in log and console texts. |
| **pl** | "ty" / "ty" (informal, as today) | Impersonal forms about viewers: "dodane przez", "zgłoszone przez". | „ ” for quotes. |
| **es** | "usted" or neutral / "tú" | Avoid gendered forms about viewers: "añadido por {name}" refers to the video. Infinitive on buttons ("Copiar enlace"). | « » or “ ” consistently (pick “ ” to match the interface); ¿…? ¡…! |
| **de** | "Sie" / "du" | Impersonal forms about viewers ("hinzugefügt von {name}", "gewünscht von {name}"); infinitive on buttons ("Link kopieren"); progress and waiting as passive or noun forms, never first person ("Wird geladen…", "Wird verbunden…", "Internetverbindung wird hergestellt…", "Warten auf die Anmeldung…", "{url} wird heruntergeladen"); results as "… ist fehlgeschlagen", not "hat nicht geklappt". Generic masculine for roles (Zuschauer, Betreiber). Hints that name a control quote its label exactly („Nur erlaubte Websites“, „Intervall“, Ansicht „Kanal“). | „ “ for quotes; spaced en dash " – " (never the em dash "—"); "z. B."; space before units and "%" ("CPU 12 %"). |
| **fr** | "vous" / "tu" | Impersonal forms about viewers ("ajouté par {name}"); infinitive on buttons ("Copier le lien"). | « » with a narrow no-break space inside; no-break space before ? ! : ; |
| **ja** | Polite (both) | です/ます for sentences; short nouns on buttons and labels (「コピー」「スキップ」); no personal pronouns; consistent katakana for loanwords (the table); impersonal "追加: {name}" style about viewers. | 「 」 for quotes; full-width punctuation 。、 in sentences, none at the end of buttons. |

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
| Slate — still screen the stream shows between videos (idle, buffering, pause) | slate | plansza | pantalla | Tafel (Wartetafel, Pausentafel) | écran | 画面（待機画面） |
| Slate language — language of slates and ticker | language of the slates | język plansz | idioma de las pantallas | Sprache der Tafeln | langue des écrans | 画面の言語 |
| Info ticker — text band burned into the stream | info ticker | pasek informacyjny | barra informativa | Laufband | bandeau d’information | テロップ |
| Message (owner's text in the ticker) | message | wiadomość | mensaje | Nachricht | message | メッセージ |
| Interval (ticker mode "from time to time") | interval | odstęp | intervalo | Intervall | intervalle | 間隔 |
| Progress line (ticker) | progress line | pasek postępu | línea de progreso | Fortschrittslinie | ligne de progression | 進行バー |
| Tunnel — built-in Cloudflare connection | tunnel | tunel | túnel | Tunnel | tunnel | トンネル |
| Quick tunnel / quick link — random trycloudflare.com address | quick tunnel / quick link | szybki tunel / szybki link | túnel rápido / enlace rápido | Schnelltunnel / schneller Link | tunnel rapide / lien rapide | クイックトンネル / クイックリンク |
| Permanent link — own domain via a named tunnel | permanent link | stały link | enlace permanente | dauerhafter Link | lien permanent | 固定リンク |
| Public address | public address | adres publiczny | dirección pública | öffentliche Adresse | adresse publique | 公開アドレス |
| Manual address — typed by hand with the tunnel off | manual address | adres ręczny | dirección manual | eigene Adresse | adresse manuelle | 手動アドレス |
| Request page — page where viewers add videos | request page | strona zgłoszeń | página de peticiones | Wunschseite | page de demandes | リクエストページ |
| Request — a viewer's video sent from the request page | request | zgłoszenie | petición | Wunsch | demande | リクエスト |
| Requested by / added by (impersonal) | requested by / added by | zgłoszone przez / dodane przez | pedido por / añadido por | gewünscht von / hinzugefügt von | demandé par / ajouté par | リクエスト: {name} / 追加: {name} |
| Viewer | viewer | widz | espectador | Zuschauer | spectateur | 視聴者 |
| Owner — the person running the program | owner | właściciel | propietario | Betreiber | propriétaire | オーナー |
| Host — the owner's badge and reserved nick (must match `engine/src/nicks.rs`) | Host | Gospodarz | Anfitrión | Gastgeber | Hôte | ホスト |
| Added by the host (request page, `viewer.added_by_host`; the program's own queue says "added here", `queue.by_host`) | added by the host | dodane przez gospodarza | añadido por el anfitrión | vom Gastgeber hinzugefügt | ajouté par l’hôte | ホストが追加 |
| Nick — viewer's chosen name | nick | nick | apodo | Nick | pseudo | ニックネーム |
| Device — a viewer's browser (device key) | device | urządzenie | dispositivo | Gerät | appareil | 端末 |
| Block / unblock (a device) | block / unblock | zablokuj / odblokuj | bloquear / desbloquear | sperren / entsperren | bloquer / débloquer | ブロック / ブロック解除 |
| Release (a nick) | release | zwolnij | liberar | freigeben | libérer | 解放 |
| Anonymous | anonymous | anonimowo | anónimo | anonym | anonyme | 匿名 |
| Approval / approve / reject (moderation) | approval / approve / reject | zatwierdzanie / zatwierdź / odrzuć | aprobación / aprobar / rechazar | Freigabe / freigeben / ablehnen | validation / valider / refuser | 承認 / 承認 / 却下 |
| Waiting for approval | Waiting for approval | Do zatwierdzenia | Pendiente de aprobación | Wartet auf Freigabe | En attente de validation | 承認待ち |
| Allowed sites (allowlist) | allowed sites | dozwolone serwisy | sitios permitidos | erlaubte Websites | sites autorisés | 許可サイト |
| Blocked sites (blocklist) | blocked sites | zablokowane serwisy | sitios bloqueados | gesperrte Websites | sites bloqués | ブロックしたサイト |
| Site (a website a link comes from; not a page of the program) | site | serwis | sitio | Website | site | サイト |
| Limits (for viewers) | limits | limity | límites | Limits | limites | 制限 |
| Pause / resume | pause / resume | wstrzymaj / wznów | pausar / reanudar | pausieren / fortsetzen | mettre en pause / reprendre | 一時停止 / 再開 |
| Paused | paused | wstrzymane | en pausa | pausiert | en pause | 一時停止中 |
| Seek (jump within the video), back / forward 10 s | seek, back / forward | skok, wstecz / naprzód | saltar, atrás / adelante | springen (spulen), zurück / vor | se déplacer, reculer / avancer | シーク、戻す / 進める |
| Skip / next video | skip / next video | pomiń / następny film | saltar / siguiente vídeo | überspringen / nächstes Video | passer / vidéo suivante | スキップ / 次の動画 |
| From the beginning | From the beginning | Od początku | Desde el principio | Von vorn | Depuis le début | 最初から |
| Play now | Play now | Odtwórz teraz | Reproducir ahora | Jetzt abspielen | Lire maintenant | 今すぐ再生 |
| On air | on air | na antenie | en emisión | auf Sendung (laufendes Video) | à l’antenne | 放送中 |
| Live stream | live stream | transmisja na żywo | directo | Livestream | direct | ライブ配信 |
| History | History | Historia | Historial | Verlauf | Historique | 履歴 |
| Preview — the stream played in the window | preview | podgląd | vista previa | Vorschau | aperçu | プレビュー |
| Stats for nerds — technical overlay (as on YouTube) | Stats for nerds | Statystyki dla nerdów | Estadísticas para nerds | Statistiken für Nerds | Statistiques pour les nerds | 詳細統計情報 |
| Share (view) | Share | Udostępnij | Compartir | Teilen | Partager | 共有 |
| Settings | Settings | Ustawienia | Ajustes | Einstellungen | Paramètres | 設定 |
| Tools — yt-dlp, ffmpeg, Deno, cloudflared | tools | narzędzia | herramientas | Werkzeuge | outils | ツール |
| Encoder / conversion (re-encoding the picture) | encoder / conversion | koder / konwersja | codificador / conversión | Encoder / Konvertierung | encodeur / conversion | エンコーダー / 変換 |
| Graphics card | graphics card | karta graficzna | tarjeta gráfica | Grafikkarte | carte graphique | グラフィックボード |
| Processor | processor | procesor | procesador | Prozessor | processeur | CPU |
| Panel — the program's interface (window or browser) | panel | panel | panel | Panel | panneau | パネル |
| Remote panel — signing in from another device with a password | remote panel | panel zdalny | panel remoto | Fernzugriff | panneau à distance | リモートパネル |
| Wizard — first-run setup and the permanent link setup | wizard | kreator | asistente | Assistent | assistant | ウィザード |
| Log | log | dziennik | registro | Protokoll | journal | ログ |
| Tray / notification area | notification area (tray) | obszar powiadomień (zasobnik) | área de notificación | Infobereich | zone de notification | 通知領域 |
| Home network | home network | sieć domowa | red doméstica | Heimnetz | réseau domestique | ホームネットワーク |
| Update | update | aktualizacja | actualización | Update / aktualisieren | mise à jour | アップデート |
| Sign in / sign out | sign in / sign out | zaloguj / wyloguj | iniciar sesión / cerrar sesión | anmelden / abmelden | se connecter / se déconnecter | ログイン / ログアウト |
| Video | video | film | vídeo | Video | vidéo | 動画 |
| Link | link | link | enlace | Link | lien | リンク |
| QR code | QR code | kod QR | código QR | QR-Code | code QR | QRコード |
| Buffering (slate title) | Buffering… | Buforowanie… | Cargando… | Wird geladen… | Chargement… | 読み込み中… |
| Online / no internet (status pill) | Online / No internet | W sieci / Brak internetu | En línea / Sin internet | Online / Kein Internet | En ligne / Pas d’internet | オンライン / インターネットなし |
| Keyboard keys | Ctrl, Shift, Home, Space | Ctrl, Shift, Home, Spacja | Ctrl, Mayús, Inicio, Espacio | Strg, Umschalt, Pos1, Leertaste | Ctrl, Maj, Origine, Espace | Ctrl, Shift, Home, スペース |
| Window buttons (as Windows names them) | Minimize, Maximize, Restore Down, Close | Minimalizuj, Maksymalizuj, Przywróć w dół, Zamknij | Minimizar, Maximizar, Restaurar, Cerrar | Minimieren, Maximieren, Verkleinern, Schließen | Réduire, Agrandir, Niveau inférieur, Fermer | 最小化, 最大化, 元に戻す (縮小), 閉じる |

## Notes per language

- **de:** "freigeben" is used both for approving a request (Freigabe) and for releasing a nick;
  the two buttons never appear together. "Fernzugriff" names the remote panel section and the
  feature ("Passwort für den Fernzugriff"); the interface itself is "Panel". "Website" is a
  site a link comes from (erlaubte/gesperrte Websites, "Download von Websites"); "Seite" only
  for pages of the program and Cloudflare's login page; "Wunschseite" is the request page.
  "Übergänge" for discontinuities (stats), "Stream-Kopie" for copying without conversion.
  Request page in du-form: "Dein Nick", "Füge einen Link … ein", "So schaust du in VRChat zu".
- **ja:** loanwords in katakana as in the table (キュー, トンネル, テロップ, ニックネーム); the
  host's word ホスト must stay as in `engine/src/nicks.rs`.
