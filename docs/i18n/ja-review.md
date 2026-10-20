# Japanese (beta): texts for review

Every Japanese text of VRCinema (`public/locales/ja.json`, 1063 keys) with the English
pattern, where it appears (from `public/locales/context.json`) and the translator's notes. Japanese
is marked "日本語 (beta)" in the language lists until a native speaker has reviewed this list.

## How to review

- Read the Japanese column in its context: the same English word can need different Japanese in
  a button, a heading, an error or a log line. Screenshots of every view in Japanese are made by
  `tools/ui-check/languages.mjs` (`… <shots dir> <port> ja`).
- Please mark wrong or unnatural texts and give the better wording; the notes column lists the
  alternatives considered where the translator was unsure.
- Changes go into `public/locales/ja.json` (same `{fields}`; Japanese plural entries have only
  `other`) and, for terms, into the Japanese column of `docs/i18n/glossary.md`.

## Style used

- **Register:** polite です/ます for every sentence. The owner's program (window, remote panel,
  tray, log, wizard) uses a neutral polite style (〜してください, 〜できませんでした). The request page
  (`viewer.*`, the viewer-only errors `errors.nick_reserved`, `nick_taken`, `too_many_active`,
  `too_many_requests`) and the texts on the stream (slates, ticker) are softer but still polite
  (〜できます, 〜してみましょう); never the plain だ/である form.
- **No personal pronouns** (no あなた, 私): "Your nick" → ニックネーム, "Your messages" → メッセージ;
  English first-person progress texts ("I am checking", "I will open") become subject-less
  statements (確認しています, ページが開きます).
- **Labels and buttons** are noun phrases or verb stems without 。 (設定, 一時停止, 次へ, リンクをコピー);
  switches are verb phrases ending in 〜する (テロップを表示する).
- **Impersonal about viewers:** 追加：{name}, リクエスト：{name}, ホストが追加.
- **Terms:** チャンネル, キュー, 次に再生, 待機画面 (slate, never a bare 画面), テロップ (ticker), トンネル,
  クイックリンク, 固定リンク, リクエストページ, リクエスト (a viewer's request; a technical API request
  is 要求), 視聴者, オーナー, ホスト (reserved nick, `engine/src/nicks.rs`), ニックネーム, デバイス,
  承認/却下, スキップ, シーク, 再生中, 放送中 (on air: what the channel broadcasts now), ライブ配信 (only a live
  source, e.g. a live stream added to the queue), ストリーム (the HLS output of the channel), 履歴, 詳細統計情報 (YouTube Japan), 共有, 設定, ツール,
  グラフィックボード, CPU, PC (this computer), 再生リスト (playlist, as YouTube Japan). Full list:
  `docs/i18n/glossary.md`.
- **Katakana:** Microsoft style with the long vowel mark: サーバー, ブラウザー, プレーヤー, フォルダー,
  エンコーダー, ユーザー, バッファー, ヘッダー; but エントリ, メモリ (Microsoft spelling).
- **Typography:** full-width 、。：？（）and 「」 in Japanese text (「」 also quote UI labels exactly, and
  video titles in the log); no space between Japanese and Latin letters or numbers (Cloudflareに接続,
  10秒, ポート{port}); a space between a number and a Latin unit (84 MB, 30 fps, 1.4 MB/s); ～ for
  ranges; no dashes (the English " — " became 。, ：, or parentheses); "…" for progress (接続中…);
  the interface's own separator " · " is kept where the code joins parts.
- **Units:** 秒, 分, 時間; counters 本 (videos), 件 (items, messages, files), 台 (devices), 回 (times).
- **Settings paths** are always written in parentheses: （設定 → インターネット）.
- **Sentence joining:** where the page puts two sentences together, Japanese gets no space after 。
  (`sentences()` in `public/app/i18n.js`).
- **Kept in English:** VRCinema and program names, VRChat menu names (Allow Untrusted URLs,
  Settings, Comfort & Safety), setting names (ADMIN_PASSWORD, MEDIA_DIRS …), file names, API
  fields and values, keyboard keys Ctrl / Shift / Home, the four API-compatibility errors, the
  language names (endonyms).
- **Stream font:** slates and ticker are drawn with Yu Gothic (`YuGothM.ttc`). It has no ►, so the
  Japanese ticker starts with ▶; the engine also replaces ► with ▶ for any language whose ticker is
  drawn with the Japanese font (a Japanese title). All slate texts and a ticker line were rendered with ffmpeg and
  checked for missing glyphs.

## Texts

### app

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `app.home_unwritable` | VRCinema cannot write its data:<br>{error}<br><br>Check the permissions of this folder or set another one in the VRCINEMA_HOME variable. | VRCinemaはデータを書き込めません：<br>{error}<br><br>このフォルダーのアクセス許可を確認するか、環境変数VRCINEMA_HOMEで別のフォルダーを指定してください。 | Message box of the desktop app. VRCINEMA_HOME is the name of an environment variable, keep it. | アクセス許可 as in Windows Explorer. |
| `app.start_failed` | VRCinema could not start:<br>{error}<br><br>Log: {path} | VRCinemaを起動できませんでした：<br>{error}<br><br>ログ：{path} | Message box of the desktop app. Shown when the program cannot start; {path} is the log folder. |  |
| `app.stop_too_slow` | VRCinema: stopping took too long — quitting without waiting. | VRCinema：終了に時間がかかりすぎているため、待たずに終了します。 | Printed to the console when the program quits, not to the log. |  |
| `app.webview2_missing` | VRCinema needs the Microsoft Edge WebView2 Runtime component, which this computer does not have.<br><br>Download it (the "Evergreen" version) from:<br>{url}<br><br>Open the download page now? | VRCinemaを使うには、Microsoft Edge WebView2 Runtimeコンポーネントが必要ですが、このPCにはインストールされていません。<br><br>次の場所からダウンロードしてください（「Evergreen」版）：<br>{url}<br><br>今すぐダウンロードページを開きますか？ | Message box of the desktop app. A yes/no question; "Evergreen" is the name of the download version, keep it. | Message box; 「Evergreen」版 keeps the product's version name. |
| `app.window_failed` | VRCinema could not open its window:<br>{error} | VRCinemaのウィンドウを開けませんでした：<br>{error} | Message box of the desktop app. Shown when the window cannot be created. |  |

### banner

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `banner.channel_down` | The channel is not running. | チャンネルが停止しています。 | Notice at the top of the interface when the channel stopped. |  |
| `banner.channel_down_reason` | The channel is not running: {reason} | チャンネルが停止しています：{reason} | Notice at the top of the interface; {reason} is an error message of the program (already translated). |  |
| `banner.dismiss` | Hide this notice | この通知を閉じる | Close button of a notice (screen-reader label and tooltip). |  |
| `banner.go_internet` | Internet | インターネット | Link in a notice to Settings → Internet. |  |
| `banner.go_ticker` | Ticker settings | テロップの設定 | Link in a notice to the info ticker settings (Settings → Channel). |  |
| `banner.go_tools` | Tools | ツール | Link in a notice to Settings → Tools. |  |
| `banner.go_update` | See the update | アップデートを表示 | Link in a notice to Settings → About, where the update is installed. |  |
| `banner.label` | Needs attention | 確認が必要な通知 | Screen-reader name of the strip of notices at the top of the interface. | Screen-reader only. Alt.: 要確認, 注意が必要な項目. |
| `banner.ticker_slow` | The computer cannot keep up with the info ticker — the stream may stutter. | PCの処理がテロップに追いついていません。映像がカクつく可能性があります。 | Notice at the top of the interface. The info ticker is the text strip burned into the stream. | 映像 (the picture) rather than 配信; カクつく is casual-technical. Alt.: 途切れる, コマ落ちする. |
| `banner.tools_missing` | Some tools are missing ({tools}) — the channel needs them. | 一部のツールがありません（{tools}）。チャンネルの動作に必要です。 | Notice at the top of the interface. {tools} is a list of program names like yt-dlp, ffmpeg. |  |
| `banner.tunnel_error` | The internet connection (tunnel) failed. | インターネット接続（トンネル）に失敗しました。 | Notice at the top of the interface: the built-in Cloudflare tunnel is in an error state. |  |
| `banner.tunnel_error_reason` | The internet connection (tunnel) failed: {reason} | インターネット接続（トンネル）に失敗しました：{reason} | Like banner.tunnel_error, with the reason (an already translated message). |  |
| `banner.update` | VRCinema {version} is available. | VRCinema {version}が利用可能です。 | Notice at the top of the interface; {version} is a version number like 0.3.0. |  |

### drop

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `drop.hint` | Drop to add to the queue | ドロップしてキューに追加 | New interface: shown over the whole window while files are dragged onto it. |  |
| `drop.none` | No video was added. | 動画は追加されませんでした。 | New interface: message after files were dropped on the window and none could be added. |  |
| `drop.skipped` | one: Skipped {count} file (it is not a video or cannot be read).<br>other: Skipped {count} files (they are not videos or cannot be read). | other: {count}件のファイルは追加されませんでした（動画ではないか、読み込めないファイルです）。 | New interface: message after files were dropped on the window; some of them were not added. | en "Skipped"; rendered as 追加されませんでした to avoid confusion with skipping a video (スキップ). Files are counted with 件 everywhere. |

### errors

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `errors.action_or_position` | Give action (approve/reject) or position. | action（approve/reject）またはpositionを指定してください。 | API error (panel, request page or app window notice). Moving/approving a queue item with a malformed request; action, approve, reject, position are API field names — keep them. |  |
| `errors.app_closing` | The app is closing. | アプリを終了しています。 | API error (panel, request page or app window notice). The app window asked for something while the app shuts down. |  |
| `errors.archive_lacks_file` | The downloaded archive has no {file}. | ダウンロードしたアーカイブに{file}がありません。 | Error text (shown in the app window and written to the log). Tools install; {file} is a file name inside the archive. |  |
| `errors.autostart_failed` | Could not change starting with Windows: {detail} | 自動起動の設定を変更できませんでした：{detail} | API error (panel, request page or app window notice). Settings: switching "start with Windows" failed; {detail} is the system error. | 自動起動 (review fix 1; also log.autostart_refresh_failed and the General switch). |
| `errors.backup_dir_failed` | Cannot create a backup folder in {path}. | {path}にバックアップフォルダーを作成できません。 | Error text (shown in the app window and written to the log). Tools install; {path} is a folder. |  |
| `errors.bad_address` | The address contains characters that are not allowed. | アドレスに使用できない文字が含まれています。 | API error (panel, request page or app window notice). The address of the video found for a viewer link has a backslash, a space or a control character (also an item error). |  |
| `errors.bad_request` | Invalid request. | 無効な要求です。 | API error (panel player controls): generic text of the code bad_request; answers usually carry a more specific text. | 要求 (not リクエスト) for a technical API request; リクエスト is reserved for a viewer's video request. |
| `errors.bad_scheme` | Only http and https links are allowed. | 使用できるのはhttpとhttpsのリンクだけです。 | API error (panel, request page or app window notice). Request page: the link is not an http or https address (for example file: or ftp:). |  |
| `errors.block_not_found` | This device is not blocked (already unblocked). | このデバイスはブロックされていません（すでに解除されています）。 | API error (panel). The block to lift does not exist (already lifted). |  |
| `errors.body_too_large` | Request body is too large | Request body is too large | API error (panel, request page or app window notice). Technical (request over 16 KB); the same English text in every language (API compatibility). |  |
| `errors.busy` | Another permanent link operation is running — wait until it ends. | 固定リンクの別の操作を実行中です。終了するまでお待ちください。 | Permanent-link wizard (app window, Settings → Internet): a second operation was started while one runs. |  |
| `errors.cancelled` | The Cloudflare login was cancelled. | Cloudflareへのログインはキャンセルされました。 | Permanent-link wizard (app window, Settings → Internet): the owner cancelled the Cloudflare login. |  |
| `errors.cannot_create_data_dir` | Cannot create the data folder {path}: {detail} | データフォルダー{path}を作成できません：{detail} | Error text (shown in the app window and written to the log). Server start; {detail} is the system error. |  |
| `errors.cannot_create_dir` | Cannot create the folder {path} | フォルダー{path}を作成できません | Error text (shown in the app window and written to the log). Tools install; a cause (system error) may follow after a colon. |  |
| `errors.cannot_listen` | Cannot listen on {address}: {detail} | {address}で待ち受けできません：{detail} | Error text (shown in the app window and written to the log). Server start; {address} is host:port, {detail} the system error (port taken). | 待ち受け = listen on a port (network term). Alt.: {address}を使用できません. |
| `errors.cannot_lock_file` | Cannot lock the file {path}: {detail} | ファイル{path}をロックできません：{detail} | Error text (shown in the app window and written to the log). Server start; the lock keeps two servers from using one data folder. |  |
| `errors.cannot_open_file` | Cannot open the file {path}: {detail} | ファイル{path}を開けません：{detail} | Error text (shown in the app window and written to the log). Server start; {detail} is the system error. |  |
| `errors.cannot_replace_file` | Cannot replace the file {path} (is it in use?) | ファイル{path}を置き換えられません（使用中の可能性があります） | Error text (shown in the app window and written to the log). Tools install; a cause (system error) may follow after a colon. |  |
| `errors.cannot_write_file` | Cannot write {path} | {path}に書き込めません | Error text (shown in the app window and written to the log). Tools install; a cause (system error) may follow after a colon. |  |
| `errors.certificate_save_failed` | Cannot save the Cloudflare certificate in the program folder: {reason} | Cloudflareの証明書をプログラムのフォルダーに保存できません：{reason} | Permanent-link wizard (app window, Settings → Internet): saving the login certificate failed; {reason} is the system error. |  |
| `errors.checksum_entry_missing` | The checksum file has no entry for {file}. | チェックサムファイルに{file}のエントリがありません。 | Error text (shown in the app window and written to the log). Tools download; {file} is a file name. | エントリ (Microsoft style without long vowel). |
| `errors.checksum_mismatch` | The checksum of {file} does not match (expected {expected}, got {actual}). | {file}のチェックサムが一致しません（期待値：{expected}、実際の値：{actual}）。 | Error text (shown in the app window and written to the log). Tools download; {expected} and {actual} are SHA-256 digests, keep them as they are. |  |
| `errors.cloudflared_start` | Cannot start cloudflared: {detail} | cloudflaredを起動できません：{detail} | Tunnel error (app window, tunnel status and wizard): the cloudflared program would not start; {detail} is the system error. |  |
| `errors.config_admin_password_missing` | ADMIN_PASSWORD is missing in the .env file — set the password of the admin panel. | .envファイルにADMIN_PASSWORDがありません。管理パネルのパスワードを設定してください。 | Error text printed to the console by the headless server or the setup command (also shown in the log). ADMIN_PASSWORD and .env are names, keep them. |  |
| `errors.config_admin_password_weak` | ADMIN_PASSWORD must be at least 8 characters and not the example "zmien-mnie". | ADMIN_PASSWORDは8文字以上にし、例の値「zmien-mnie」以外にしてください。 | Error text printed to the console by the headless server or the setup command (also shown in the log). ADMIN_PASSWORD is a setting name; "zmien-mnie" is the example value in .env.example, keep it as it is. |  |
| `errors.config_invalid_port` | Invalid PORT: {value} | PORTが無効です：{value} | Error text printed to the console by the headless server or the setup command (also shown in the log). PORT is a setting name. |  |
| `errors.config_session_secret` | Cannot generate SESSION_SECRET: {detail} | SESSION_SECRETを生成できません：{detail} | Error text printed to the console by the headless server or the setup command (also shown in the log). SESSION_SECRET is a setting name in the .env file, keep it. |  |
| `errors.config_unknown_encoder` | Unknown VIDEO_ENCODER "{value}". Allowed: {allowed} | 不明なVIDEO_ENCODER「{value}」です。使用できる値：{allowed} | Error text printed to the console by the headless server or the setup command (also shown in the log). VIDEO_ENCODER is a setting name; {allowed} is a list of technical names. |  |
| `errors.config_unknown_language` | Unknown LANGUAGE "{value}". Allowed: {allowed} | 不明なLANGUAGE「{value}」です。使用できる値：{allowed} | Error text printed to the console by the headless server or the setup command (also shown in the log). LANGUAGE is a setting name; {allowed} is a list of language codes. |  |
| `errors.cookies_by_picker` | Choose the cookies file in the file picker. | Cookieファイルはファイル選択ダイアログで選んでください。 | API error (panel, request page or app window notice). Settings: the cookies file can only be chosen with the file picker. |  |
| `errors.credentials_fetch_failed` | Could not fetch the credentials of the tunnel {tunnel}: {reason} | トンネル{tunnel}の資格情報を取得できませんでした：{reason} | Permanent-link wizard (app window, Settings → Internet): {tunnel} is the tunnel name (vrcinema), {reason} what cloudflared said. |  |
| `errors.credentials_invalid` | cloudflared did not save valid tunnel credentials. Try again. | cloudflaredが有効なトンネルの資格情報を保存しませんでした。もう一度お試しください。 | Permanent-link wizard (app window, Settings → Internet): the tunnel credentials file is unusable. |  |
| `errors.credentials_save_failed` | Cannot save the tunnel credentials in the program folder: {reason} | トンネルの資格情報をプログラムのフォルダーに保存できません：{reason} | Permanent-link wizard (app window, Settings → Internet): {reason} is the system error. |  |
| `errors.data_dir_in_use` | Another channel server is already using the data folder: {path} | 別のチャンネルサーバーがすでにこのデータフォルダーを使用しています：{path} | Error text (shown in the app window and written to the log). Server start; two servers on one data folder would wipe each other's files. |  |
| `errors.deno_unpack_failed` | Unpacking Deno failed | Denoの展開に失敗しました | Error text (shown in the app window and written to the log). Tools install; a cause (system error) may follow after a colon. Deno is a program name. |  |
| `errors.device_blocked` | You cannot add videos from this device. | このデバイスからは動画を追加できません。 | API error (request page). The owner blocked the viewer's device; its requests are refused. |  |
| `errors.dns_route_failed` | Could not set the address {hostname} in Cloudflare DNS: {reason} | Cloudflare DNSにアドレス{hostname}を設定できませんでした：{reason} | Permanent-link wizard (app window, Settings → Internet): {hostname} is the chosen address, {reason} what cloudflared said. |  |
| `errors.dns_taken` | The address {hostname} is already taken in Cloudflare DNS (another record has it). Choose another name or replace the existing record. | アドレス{hostname}はCloudflare DNSですでに使われています（別のレコードがあります）。別の名前を選ぶか、既存のレコードを置き換えてください。 | Permanent-link wizard (app window, Settings → Internet): the chosen address already has a DNS record; the window then offers to replace it. |  |
| `errors.domain_blocked` | This site is blocked on this channel. | このサイトはこのチャンネルでブロックされています。 | API error (panel, request page or app window notice). Request page: the link is from a site the owner has blocked. |  |
| `errors.domain_not_allowed` | This site is not allowed. Allowed: {domains}. | このサイトは許可されていません。許可されているサイト：{domains}。 | API error (panel, request page or app window notice). Request page: the link is from a site not on the list; {domains} is a comma-separated list of domains. |  |
| `errors.download_failed` | Download failed: {url} | ダウンロードに失敗しました：{url} | Error text (shown in the app window and written to the log). Tools download and update; a cause (network error) may follow after a colon. |  |
| `errors.download_failed_status` | Download failed: {status} {url} | ダウンロードに失敗しました：{status} {url} | Error text (shown in the app window and written to the log). Tools download; {status} is the HTTP status number. |  |
| `errors.download_incomplete` | Download interrupted ({downloaded} of {total} bytes): {url} | ダウンロードが中断されました（{total}バイト中{downloaded}バイト）：{url} | Error text (shown in the app window and written to the log). Tools download; the byte counts are numbers. |  |
| `errors.download_interrupted` | Download interrupted: {url} | ダウンロードが中断されました：{url} | Error text (shown in the app window and written to the log). Tools download; a cause (network error) may follow after a colon. |  |
| `errors.download_unexpected_range` | The server returned an unexpected range of data: {url} | サーバーから予期しない範囲のデータが返されました：{url} | Error text (shown in the app window and written to the log). Tools download; the server answered a resumed download with the wrong part of the file. |  |
| `errors.empty_json_body` | Body cannot be empty when content-type is set to 'application/json' | Body cannot be empty when content-type is set to 'application/json' | API error (panel, request page or app window notice). Technical; the same English text in every language (API compatibility). |  |
| `errors.empty_playlist` | The playlist is empty. | 再生リストが空です。 | API error (panel, request page or app window notice). Panel: an added playlist has no playable entries. |  |
| `errors.engine_unavailable` | The channel is not taking commands right now. Try again in a moment. | チャンネルは現在コマンドを受け付けていません。しばらくしてからもう一度お試しください。 | API error (panel, request page or app window notice). Panel player controls (pause, seek, restart, play now) while the channel loop is busy or restarting. |  |
| `errors.env_file_unreadable` | Cannot read the file {path}: {detail} | ファイル{path}を読み込めません：{detail} | Error text printed to the console by the headless server or the setup command (also shown in the log). {detail} is the system error. |  |
| `errors.failed` | The permanent link operation was interrupted: {detail} | 固定リンクの操作が中断されました：{detail} | Permanent-link wizard (app window, Settings → Internet): an operation ended unexpectedly; {detail} is a technical cause. Also the generic message of the error code "failed". |  |
| `errors.fetch_failed` | Could not read the video: {reason} | 動画の情報を取得できませんでした：{reason} | API error (panel, request page or app window notice). Request page: the link could not be read; {reason} is a message from yt-dlp or the site (often English) or another errors.* message. | en "read" = fetch/look up the link (yt-dlp), not play: 取得. Shown on the request page too. |
| `errors.ffmpeg_missing` | ffmpeg is missing — download the tools first. | ffmpegがありません。先にツールをダウンロードしてください。 | API error (panel, request page or app window notice). Settings: encoder test without ffmpeg installed. |  |
| `errors.ffmpeg_unpack_failed` | Unpacking ffmpeg failed | ffmpegの展開に失敗しました | Error text (shown in the app window and written to the log). Tools install; a cause (system error) may follow after a colon. ffmpeg is a program name. |  |
| `errors.field_not_boolean` | The field {field} must be true or false. | フィールド{field}はtrueまたはfalseである必要があります。 | API error (panel, request page or app window notice). Malformed settings request; {field} is an API field name. |  |
| `errors.field_not_text` | The field {field} must be text. | フィールド{field}はテキストである必要があります。 | API error (panel, request page or app window notice). Malformed settings request; {field} is an API field name. |  |
| `errors.file_missing` | The file does not exist: {name} | ファイルが存在しません：{name} | API error (panel, request page or app window notice). Files from disk: the file is gone; {name} is a file name. |  |
| `errors.file_paths_unavailable` | Files by full path are not available. | フルパスによるファイルの指定は利用できません。 | API error (panel, request page or app window notice). Files dropped on the window are not supported in this mode. |  |
| `errors.folder_unreadable` | Cannot read the folder. | フォルダーを読み込めません。 | API error (panel, request page or app window notice). Panel, files from disk: a media folder cannot be read. |  |
| `errors.folder_unreadable_detail` | Cannot read the folder: {detail} | フォルダーを読み込めません：{detail} | API error (panel, request page or app window notice). Panel, files from disk; {detail} is the system error. |  |
| `errors.github_bad_answer` | Invalid answer from GitHub | GitHubからの応答が無効です | Error text (shown in the app window and written to the log). Update check; a cause may follow after a colon. |  |
| `errors.github_status` | GitHub answered with code {status} | GitHubがコード{status}を返しました | Error text (shown in the app window and written to the log). Update check; {status} is the HTTP status number. |  |
| `errors.github_unreachable` | Cannot connect to GitHub | GitHubに接続できません | Error text (shown in the app window and written to the log). Update check; a cause (network error) may follow after a colon. |  |
| `errors.home_unknown` | Cannot determine the app data folder (neither VRCINEMA_HOME nor LOCALAPPDATA is set). | アプリのデータフォルダーを特定できません（VRCINEMA_HOMEもLOCALAPPDATAも設定されていません）。 | Error text (shown in a message box at start-up). VRCINEMA_HOME and LOCALAPPDATA are names of environment variables, keep them. |  |
| `errors.hostname_invalid` | Invalid host name for the permanent link (e.g. channel.example.com). | 固定リンクのホスト名が無効です（例：channel.example.com）。 | Permanent-link wizard (app window, Settings → Internet): the typed address is not a valid host name; the example may be adapted. |  |
| `errors.hostname_required` | Enter the permanent link address (e.g. channel.example.com). | 固定リンクのアドレスを入力してください（例：channel.example.com）。 | Permanent-link wizard (app window, Settings → Internet): no address was typed; the example may be adapted. |  |
| `errors.internal` | Server error | サーバーエラー | API error (panel, request page or app window notice). Unexpected server failure (details only in the log). |  |
| `errors.invalid` | Invalid request. | 無効な要求です。 | Permanent-link wizard (app window): a malformed request; also the generic text of the code invalid. | Same wording as errors.bad_request (要求 = technical request). |
| `errors.invalid_device_key` | The device key must be 32 hexadecimal characters. | デバイスキーは16進数32文字である必要があります。 | API error (request page). The device key sent with a request is not 32 hexadecimal characters (a bug of the page, not something the viewer typed). |  |
| `errors.invalid_disk_file_url` | Invalid address of a file on disk. | ディスク上のファイルのアドレスが無効です。 | API error (panel, request page or app window notice). Files from disk: malformed internal file address. |  |
| `errors.invalid_json` | Body is not valid JSON but content-type is set to 'application/json' | Body is not valid JSON but content-type is set to 'application/json' | API error (panel, request page or app window notice). Technical; the same English text in every language (API compatibility). |  |
| `errors.invalid_language` | Unknown language. | 不明な言語です。 | Error text (shown in the app window and written to the log). Settings; the language setting named a language the program does not have. |  |
| `errors.invalid_link` | That does not look like a valid link (http/https). | 有効なリンク（http/https）ではないようです。 | API error (panel, request page or app window notice). Request page and panel: the text entered is not an http(s) link. |  |
| `errors.invalid_path` | Invalid path. | 無効なパスです。 | API error (panel, request page or app window notice). Panel, files from disk: a malformed path. |  |
| `errors.invalid_port` | The port must be a number from 1024 to 65535. | ポートは1024～65535の数値で指定してください。 | API error (panel, request page or app window notice). Settings: the port field. | Range with full-width tilde ～ (Japanese convention). |
| `errors.invalid_public_url` | The public address must start with http:// or https://. | 公開アドレスはhttp://またはhttps://で始まる必要があります。 | API error (panel, request page or app window notice). Settings: the public address field. |  |
| `errors.invalid_settings` | Invalid settings. | 設定が無効です。 | API error (panel, request page or app window notice). Settings: the request is not an object of settings. |  |
| `errors.invalid_tunnel_mode` | The tunnel mode must be quick or named. | トンネルモードはquickまたはnamedである必要があります。 | API error (panel, request page or app window notice). Settings; quick and named are API values — keep them. |  |
| `errors.invalid_url` | Invalid URL: {url} | 無効なURLです：{url} | API error (panel, request page or app window notice). {url} is the link that could not be parsed. |  |
| `errors.item_finished` | This item has already finished. | この項目はすでに終了しています。 | API error (panel, request page or app window notice). Panel: removing a queue item that already played. |  |
| `errors.item_id_required` | Give itemId. | itemIdを指定してください。 | API error (panel, request page or app window notice). Panel "play now" with a malformed request; itemId is an API field name — keep it. |  |
| `errors.item_not_playable` | Item #{id} cannot be played now (status {status}). | 項目#{id}は現在再生できません（状態：{status}）。 | Error text (written to the log). The owner asked to play a queue item that is not in a state that allows it; {status} is a technical word (playing, removed, ...), keep it. |  |
| `errors.library_disabled` | Files from disk are switched off. Set MEDIA_DIRS in the .env file. | ディスク上のファイルは無効になっています。.envファイルでMEDIA_DIRSを設定してください。 | API error (panel, request page or app window notice). Headless server panel; MEDIA_DIRS and .env are names — keep them. |  |
| `errors.live_item` | A live stream cannot be paused or seeked. | ライブ配信は一時停止やシークができません。 | API error (panel, request page or app window notice). Panel player controls while a live stream plays. |  |
| `errors.live_owner_only` | Only the channel owner can add live streams. | ライブ配信を追加できるのはチャンネルのオーナーだけです。 | API error (panel, request page or app window notice). Request page: a viewer sent a live stream link. |  |
| `errors.local_address` | Links to a local network or to this computer are not allowed. | ローカルネットワークやこのPCへのリンクは使用できません。 | API error (panel, request page or app window notice). Request page: the link points to the owner's local network or computer (also an item error when the video address found for a viewer's link is such). |  |
| `errors.local_files_disabled` | Files from disk are switched off (no MEDIA_DIRS in .env). | ディスク上のファイルは無効になっています（.envにMEDIA_DIRSがありません）。 | API error (panel, request page or app window notice). A disk file was requested without media folders; MEDIA_DIRS and .env are names — keep them. |  |
| `errors.local_not_a_video` | This is not a video file: {name} | 動画ファイルではありません：{name} | API error (panel, request page or app window notice). Panel, files from disk: the chosen file is not a video; {name} is the file name. |  |
| `errors.login_failed` | The Cloudflare login failed: {reason} | Cloudflareへのログインに失敗しました：{reason} | Permanent-link wizard (app window, Settings → Internet): {reason} is what cloudflared said (English) or named.no_reason. |  |
| `errors.login_interrupted` | The Cloudflare login was interrupted: {reason} | Cloudflareへのログインが中断されました：{reason} | Permanent-link wizard (app window, Settings → Internet): {reason} is the system error. |  |
| `errors.login_no_certificate` | The Cloudflare login failed: cloudflared did not save the certificate. Try again. | Cloudflareへのログインに失敗しました：cloudflaredが証明書を保存しませんでした。もう一度お試しください。 | Permanent-link wizard (app window, Settings → Internet): the login ended without a certificate. |  |
| `errors.login_required` | Log in. | ログインしてください。 | API error (panel, request page or app window notice). Panel request without a valid session. |  |
| `errors.login_timeout` | The Cloudflare login was not completed within {duration}. Try again. If the Cloudflare account has no domain yet, add one first — without it the login does not finish. | Cloudflareへのログインが{duration}以内に完了しませんでした。もう一度お試しください。Cloudflareアカウントにまだドメインがない場合は、先に追加してください。ドメインがないとログインは完了しません。 | Permanent-link wizard (app window, Settings → Internet): {duration} is a duration like "5 min" (units.*). |  |
| `errors.lookup_failed` | Could not look up the address of this site in time. Try again in a moment. | このサイトのアドレスを時間内に確認できませんでした。しばらくしてからもう一度お試しください。 | API error (panel, request page or app window notice). Request page: the name of the site in the link could not be looked up in time (also an item error for the video address). | "look up the address" = DNS lookup; 確認 is deliberately non-technical (request page). Alt.: 名前解決. |
| `errors.move_only_queued` | Only items in the queue can be moved. | 移動できるのはキュー内の項目だけです。 | API error (panel, request page or app window notice). Panel: moving an item that is not queued. |  |
| `errors.named_files_left` | The tunnel was deleted in Cloudflare and the quick tunnel runs, but the permanent link files could not be deleted: {detail} | Cloudflareのトンネルは削除され、クイックトンネルが動作していますが、固定リンクのファイルを削除できませんでした：{detail} | Permanent-link wizard (app window, Settings → Internet): deleting the permanent link left files behind; {detail} is the system error. |  |
| `errors.named_link_first` | Set up the permanent link first (Settings → Internet → Set up a permanent link). | 先に固定リンクを設定してください（設定 → インターネット → 固定リンクを設定）。 | API error (panel, request page or app window notice). Settings: switching to the permanent link before setting it up; the path names the window's sections — use their translated names. | Path uses the real labels: 設定 → インターネット → 固定リンクを設定 (permalink.setup). |
| `errors.named_operation_running` | A permanent link operation is running (Cloudflare login or address setup) — try again when it ends. | 固定リンクの操作（Cloudflareへのログインまたはアドレスの設定）を実行中です。終了してからもう一度お試しください。 | API error (panel, request page or app window notice). Settings → Tools: updating cloudflared while the permanent-link wizard runs. |  |
| `errors.nick_not_found` | No such nick. | そのニックネームは見つかりません。 | API error (panel). The nick to release or block is not held by anyone (already released). |  |
| `errors.nick_reserved` | This nick is reserved. Pick another one. | このニックネームは予約済みのため使用できません。別のニックネームを選んでください。 | API error (request page). The nick is reserved for the owner (the words for host in every language and look-alike spellings of them) and cannot be used by viewers. | Request page (viewer): 予約済みのため使用できません. |
| `errors.nick_taken` | This nick is already taken by someone else. Pick another one. | このニックネームはすでに他の人が使っています。別のニックネームを選んでください。 | API error (request page). The nick (or a look-alike spelling of it) belongs to another viewer's device, which used it in the last 30 days. | Request page (viewer). |
| `errors.no_cloudflared` | cloudflared is missing — download the missing tools in the settings (Tools). | cloudflaredがありません。不足しているツールをダウンロードしてください（設定 → ツール）。 | Tunnel status and wizard (app window): the cloudflared program is not installed; "Tools" is the name of the settings section. | Settings paths are always written （設定 → X）. |
| `errors.no_domain` | Add a domain to Cloudflare before you set up a permanent link. The address {hostname} must belong to the domain chosen at the Cloudflare login. | 固定リンクを設定する前に、Cloudflareにドメインを追加してください。アドレス{hostname}は、Cloudflareへのログイン時に選んだドメインに属している必要があります。 | Permanent-link wizard (app window, Settings → Internet): the account has no domain or the address is outside it; shown with a help link. |  |
| `errors.no_domain_record` | Add a domain to Cloudflare before you set up a permanent link. The address {hostname} must belong to the domain chosen at the Cloudflare login. Cloudflare set the record {record} instead — delete it in the Cloudflare dashboard (DNS). | 固定リンクを設定する前に、Cloudflareにドメインを追加してください。アドレス{hostname}は、Cloudflareへのログイン時に選んだドメインに属している必要があります。代わりにCloudflareがレコード{record}を設定しました。Cloudflareのダッシュボード（DNS）で削除してください。 | Permanent-link wizard (app window, Settings → Internet): like errors.no_domain, plus the wrong DNS record {record} Cloudflare created. |  |
| `errors.no_public_address` | The channel has no public address yet. | チャンネルにはまだ公開アドレスがありません。 | API error (panel, request page or app window notice). App window: the QR code was asked for before the channel has an address. |  |
| `errors.no_segment` | No such segment | そのセグメントはありません | API error (panel, request page or app window notice). A video player asked for a stream segment that is gone (technical). |  |
| `errors.no_such_item` | There is no such item. | その項目は存在しません。 | API error (panel, request page or app window notice). Panel: the queue item does not exist (any more). |  |
| `errors.no_such_root` | There is no such root folder. | そのルートフォルダーは存在しません。 | API error (panel, request page or app window notice). Panel, files from disk: an unknown media folder. |  |
| `errors.no_update` | No update is available. | 利用できるアップデートはありません。 | API error (panel, request page or app window notice). About: installing an update when none is offered. |  |
| `errors.no_videos_in_folder` | There are no video files in this folder. | このフォルダーには動画ファイルがありません。 | API error (panel, request page or app window notice). Panel, files from disk: adding an empty folder. |  |
| `errors.not_a_folder` | This is not a folder. | これはフォルダーではありません。 | API error (panel, request page or app window notice). Panel, files from disk: listing a file as a folder. |  |
| `errors.not_a_video_file` | This is not a video file: {name} | 動画ファイルではありません：{name} | API error (panel, request page or app window notice). Files dropped on the window: {name} is the file name. |  |
| `errors.not_logged_in` | Connect to Cloudflare first (step “Connect to Cloudflare”). | 先にCloudflareに接続してください（ステップ「Cloudflareに接続」）。 | Permanent-link wizard (app window, Settings → Internet): a step needs the Cloudflare login first; the quoted step name must match the wizard's button. | Quotes the button permalink.connect 「Cloudflareに接続」 exactly; ステップ as in wizard.step. |
| `errors.not_pending` | This item is not waiting for approval. | この項目は承認待ちではありません。 | API error (panel, request page or app window notice). Panel moderation: approving an item that is not pending. |  |
| `errors.not_playable_now` | Only a video from the queue or the history can be played now. | 今すぐ再生できるのは、キューまたは履歴にある動画だけです。 | API error (panel, request page or app window notice). Panel "play now" on an item that is neither queued nor in the history. |  |
| `errors.nothing_playing` | Nothing is playing right now. | 現在、再生中の動画はありません。 | API error (panel, request page or app window notice). Panel player controls when no video plays. |  |
| `errors.page_not_found` | Page not found | ページが見つかりません | API error (panel, request page or app window notice). A page of the program is missing (technical). |  |
| `errors.password_first` | Set the remote panel password first. | 先にリモートパネルのパスワードを設定してください。 | API error (panel, request page or app window notice). Settings: switching on the remote panel without a password. |  |
| `errors.password_save_failed` | Could not save the password: {detail} | パスワードを保存できませんでした：{detail} | API error (panel, request page or app window notice). Settings; {detail} is a technical cause. |  |
| `errors.password_too_short` | The password must have at least {min} characters. | パスワードは{min}文字以上にしてください。 | API error (panel, request page or app window notice). Settings: remote panel password; {min} is a number (8). |  |
| `errors.path_outside_library` | The path leads outside the video folder. | このパスは動画フォルダーの外を指しています。 | API error (panel, request page or app window notice). Panel, files from disk: a path escaping the media folder. |  |
| `errors.pixeldrain_bad_answer` | pixeldrain: invalid server answer. | pixeldrain：サーバーの応答が無効です。 | API error (panel, request page or app window notice). Reading a pixeldrain link; "pixeldrain" is a site name. |  |
| `errors.pixeldrain_bad_link` | pixeldrain: unrecognised address. Use a /u/… or /l/… link. | pixeldrain：認識できないアドレスです。/u/…または/l/…のリンクを使用してください。 | API error (panel, request page or app window notice). Reading a pixeldrain link; /u/ and /l/ are URL parts — keep them. |  |
| `errors.pixeldrain_connection` | pixeldrain: connection error ({detail}). | pixeldrain：接続エラーです（{detail}）。 | API error (panel, request page or app window notice). Reading a pixeldrain link; {detail} is the network error. |  |
| `errors.pixeldrain_list` | pixeldrain: a list must first be expanded into single files. | pixeldrain：リストは先に個別のファイルに展開する必要があります。 | API error (panel, request page or app window notice). A pixeldrain list was played directly. |  |
| `errors.pixeldrain_not_video` | pixeldrain: "{name}" is not a video file. | pixeldrain：「{name}」は動画ファイルではありません。 | API error (panel, request page or app window notice). Reading a pixeldrain link; {name} is the file name. |  |
| `errors.pixeldrain_read` | pixeldrain: error reading the answer ({detail}). | pixeldrain：応答の受信中にエラーが発生しました（{detail}）。 | API error (panel, request page or app window notice). Reading a pixeldrain link; {detail} is the network error. | Fetching, not playing: 応答の受信中. |
| `errors.pixeldrain_unavailable` | pixeldrain: file unavailable (HTTP {status}). | pixeldrain：ファイルを利用できません（HTTP {status}）。 | API error (panel, request page or app window notice). Reading a pixeldrain link; {status} is an HTTP status number. |  |
| `errors.playlist_owner_only` | Only the channel owner can add playlists. | 再生リストを追加できるのはチャンネルのオーナーだけです。 | API error (panel, request page or app window notice). Request page: a viewer sent a playlist link. |  |
| `errors.process_timeout` | Time limit exceeded ({seconds} s): {command} | 制限時間を超えました（{seconds}秒）：{command} | API error (panel, request page or app window notice). A helper program (yt-dlp) ran too long; {command} is its name. |  |
| `errors.qr_failed` | Could not draw the QR code: {detail} | QRコードを生成できませんでした：{detail} | API error (panel, request page or app window notice). App window, share view; {detail} is a technical cause. |  |
| `errors.read_failed` | Could not read: {reason} | リンクの情報を取得できませんでした：{reason} | API error (panel, request page or app window notice). Panel: an added link could not be read; {reason} is a message from yt-dlp or the site, or another errors.* message. | As errors.fetch_failed: 取得 for reading a link. |
| `errors.remote_panel_off` | The remote panel is switched off. | リモートパネルはオフになっています。 | API error (panel, request page or app window notice). Remote panel login page while remote access is off. |  |
| `errors.restart_failed` | Could not restart the server: {detail} | サーバーを再起動できませんでした：{detail} | API error (panel, request page or app window notice). App window "restart channel"; {detail} is a technical cause. |  |
| `errors.restart_interrupted` | The server restart was interrupted: {detail} | サーバーの再起動が中断されました：{detail} | Error text (shown in the app window and written to the log). Server restart; {detail} is the system error. |  |
| `errors.restart_reverted` | Could not start the server with the new settings ({detail}). The previous ones were restored. | 新しい設定でサーバーを起動できませんでした（{detail}）。以前の設定に戻しました。 | API error (panel, request page or app window notice). Settings: new port or network access failed; {detail} is a technical cause. |  |
| `errors.seek_seconds` | Give a number of seconds. | 秒数を指定してください。 | API error (panel, request page or app window notice). Panel seek with a malformed or too large number. |  |
| `errors.seek_target` | Give toS or byS (seconds). | toSまたはbySを秒単位で指定してください。 | API error (panel, request page or app window notice). Panel seek with a malformed request; toS and byS are API field names — keep them. |  |
| `errors.seek_unknown_length` | The length of this video is not known: it can only be skipped forward. | この動画は長さが不明なため、先に進めることしかできません。 | API error (panel, request page or app window notice). Panel seek backwards in a video of unknown length. |  |
| `errors.server_aborted` | The server stopped working: {detail} | サーバーが動作を停止しました：{detail} | Error text (shown in the app window and written to the log). Server start and stop; {detail} is the system error. |  |
| `errors.server_exited_early` | The server stopped before it started. | サーバーが起動する前に停止しました。 | Error text (shown in the app window and written to the log). Server start. |  |
| `errors.server_no_state` | The server did not hand over its state. | サーバーが状態を引き渡しませんでした。 | Error text (shown in the app window and written to the log). Server start; an internal problem. |  |
| `errors.server_restarting` | The server is being restarted — try again in a moment. | サーバーを再起動しています。しばらくしてからもう一度お試しください。 | Error text (shown in the app window and written to the log). Adding files by dropping them on the window while the server restarts. |  |
| `errors.server_start_failed` | Could not start the server: {detail} | サーバーを起動できませんでした：{detail} | API error (panel, request page or app window notice). Settings; {detail} is a technical cause. |  |
| `errors.server_start_timeout` | The server did not start within {seconds} s. | サーバーが{seconds}秒以内に起動しませんでした。 | Error text (shown in the app window and written to the log). Server start; {seconds} is a number. |  |
| `errors.setting_not_boolean` | {key}: expected true/false | {key}：true/falseを指定してください | API error (panel, request page or app window notice). Panel channel settings; {key} is a setting name. |  |
| `errors.setting_not_language` | {key}: expected one of: {languages} | {key}：{languages}のいずれかを指定してください | API error (panel). Panel channel settings (slate language); {key} is a setting name, {languages} the allowed language codes, comma separated (en, pl, ...) - keep the codes as they are. |  |
| `errors.setting_not_list` | {key}: expected a list | {key}：リストを指定してください | API error (panel, request page or app window notice). Panel channel settings; {key} is a setting name. |  |
| `errors.setting_not_number` | {key}: expected a number ≥ 0 | {key}：0以上の数値を指定してください | API error (panel, request page or app window notice). Panel channel settings; {key} is a setting name. |  |
| `errors.setting_not_one_of` | {key}: expected one of: {values} | {key}：{values}のいずれかを指定してください | API error (panel). Panel channel settings (info ticker); {key} is a setting name, {values} the allowed values, comma separated (interval, continuous, 60, 120, ...) - keep the values as they are. |  |
| `errors.settings_not_object` | expected an object with settings | 設定のオブジェクトを指定してください | API error (panel, request page or app window notice). Panel channel settings: malformed request. |  |
| `errors.settings_save_failed` | Cannot save the settings in {path}: {detail} | {path}に設定を保存できません：{detail} | API error (panel, request page or app window notice). App settings could not be written; {path} is the file, {detail} the system error. |  |
| `errors.setup_env_example_missing` | The file .env.example is missing | ファイル.env.exampleがありません | Error text printed to the console by the headless server or the setup command (also shown in the log). A file name; a cause may follow after a colon. |  |
| `errors.setup_macos_ffmpeg` | On macOS install ffmpeg with Homebrew and set FFMPEG_PATH/FFPROBE_PATH in .env. | macOSではHomebrewでffmpegをインストールし、.envでFFMPEG_PATH/FFPROBE_PATHを設定してください。 | Error text printed to the console by the headless server or the setup command (also shown in the log). Homebrew, FFMPEG_PATH, FFPROBE_PATH and .env are names, keep them. |  |
| `errors.setup_unknown_option` | Unknown option: {option} (available: --force, --no-env) | 不明なオプション：{option}（使用できるオプション：--force、--no-env） | Error text printed to the console by the headless server or the setup command (also shown in the log). Keep the option names --force and --no-env. | Console text. |
| `errors.site_not_supported` | This site is not supported. | このサイトには対応していません。 | Item error (queue, panel and request page). yt-dlp does not support the site of the link. |  |
| `errors.source_message` | {detail} | {detail} | A message from outside the program (yt-dlp, a website, the system), shown as it is; used as {reason} in other messages. Keep just the field. |  |
| `errors.ticker_message_too_long` | {key}: a message can have at most {max} characters | {key}：メッセージは{max}文字以内にしてください | API error (panel). Panel channel settings, info ticker messages; {key} is the setting name, {max} a number (200). |  |
| `errors.ticker_too_many_messages` | {key}: at most {max} messages | {key}：メッセージは{max}件までです | API error (panel). Panel channel settings, info ticker messages; {key} is the setting name, {max} a number (10). |  |
| `errors.timeout` | cloudflared did not answer within {seconds} s. Check the internet connection and try again. | cloudflaredが{seconds}秒以内に応答しませんでした。インターネット接続を確認して、もう一度お試しください。 | Permanent-link wizard (app window, Settings → Internet): a cloudflared command took too long. |  |
| `errors.too_long` | The video is too long (limit: {limit}). | 動画が長すぎます（上限：{limit}）。 | API error (panel, request page or app window notice). Request page; {limit} is a duration like "2 h 0 min" (units.*). |  |
| `errors.too_many_active` | one: You already have {count} video in the queue. Wait until it has played.<br>other: You already have {count} videos in the queue. Wait until they have played. | other: すでに{count}本の動画をキューに追加しています。再生が終わるまでお待ちください。 | API error (panel, request page or app window notice). Request page: the viewer reached the limit of queued videos; {count} is that limit. (Polish keeps the original wording in every form for API compatibility.) | Request page. No pronoun (あなた avoided): すでに{count}本の動画をキューに追加しています. Counter 本 for videos. |
| `errors.too_many_logins` | Too many failed login attempts. Try again in 15 minutes. | ログインの失敗が多すぎます。15分後にもう一度お試しください。 | API error (panel, request page or app window notice). Remote panel login. |  |
| `errors.too_many_requests` | Too many requests in a short time. Try again in a few minutes. | 短時間にリクエストが多すぎます。数分後にもう一度お試しください。 | API error (panel, request page or app window notice). Request page: rate limit. | Request page. Here リクエスト is the viewer's request (rate limit), so the word fits both senses. |
| `errors.tools_busy` | cloudflared is being updated or repaired — try again when it is done. | cloudflaredを更新または修復中です。完了してからもう一度お試しください。 | Permanent-link wizard (app window, Settings → Internet): refused while the tools job replaces cloudflared. |  |
| `errors.tools_running` | The tools are already being downloaded. | ツールはすでにダウンロード中です。 | API error (panel, request page or app window notice). Settings → Tools: a second download was started. |  |
| `errors.tunnel_create_failed` | Could not create the tunnel {tunnel} in Cloudflare: {reason} | Cloudflareにトンネル{tunnel}を作成できませんでした：{reason} | Permanent-link wizard (app window, Settings → Internet): {tunnel} is the tunnel name, {reason} what cloudflared said. |  |
| `errors.tunnel_delete_failed` | Could not delete the tunnel {tunnel} in Cloudflare: {reason} | Cloudflareのトンネル{tunnel}を削除できませんでした：{reason} | Permanent-link wizard (app window, Settings → Internet): {tunnel} is the tunnel name, {reason} what cloudflared said. |  |
| `errors.tunnel_folder_failed` | Cannot create the tunnel folder: {reason} | トンネルのフォルダーを作成できません：{reason} | Permanent-link wizard (app window, Settings → Internet): {reason} is the system error. |  |
| `errors.tunnel_off` | The tunnel is off — switch it on in the settings. | トンネルがオフです。設定でオンにしてください。 | API error (panel, request page or app window notice). App window: restarting the tunnel while it is off. |  |
| `errors.unknown_encoder` | Unknown encoder: {encoder}. | 不明なエンコーダーです：{encoder}。 | API error (panel, request page or app window notice). Settings; {encoder} is the value sent. |  |
| `errors.unknown_setting` | Unknown setting: {key} | 不明な設定：{key} | API error (panel, request page or app window notice). Panel channel settings; {key} is the setting name sent. |  |
| `errors.unknown_tool` | Unknown tool: {tool}. | 不明なツールです：{tool}。 | API error (panel, request page or app window notice). Settings → Tools; {tool} is the value sent. |  |
| `errors.unsupported_media_type` | Unsupported Media Type | Unsupported Media Type | API error (panel, request page or app window notice). Technical; the same English text in every language (API compatibility). |  |
| `errors.update_cannot_install` | Cannot install the new version | 新しいバージョンをインストールできません | Error text (shown in the app window and written to the log). Self-update; a cause (system error) may follow after a colon. The old version was restored. |  |
| `errors.update_cannot_install_or_restore` | Cannot install the new version or restore the old one (copy: {path}): {detail} | 新しいバージョンをインストールできず、古いバージョンも復元できません（コピー：{path}）：{detail} | Error text (shown in the app window and written to the log). Self-update; {path} is the saved copy of the old program. |  |
| `errors.update_cannot_remove_old` | Cannot remove the previous copy of the program | プログラムの以前のコピーを削除できません | Error text (shown in the app window and written to the log). Self-update; a cause (system error) may follow after a colon. |  |
| `errors.update_cannot_rename` | Cannot rename the current program | 現在のプログラムの名前を変更できません | Error text (shown in the app window and written to the log). Self-update; a cause (system error) may follow after a colon. |  |
| `errors.update_failed` | The update failed: {detail} | アップデートに失敗しました：{detail} | API error (panel, request page or app window notice). About: installing a new version failed; {detail} is a technical cause. |  |
| `errors.update_running` | An update is already running. | アップデートはすでに実行中です。 | API error (panel, request page or app window notice). About: a second install was started. |  |
| `errors.update_sum_mismatch` | The checksum of the downloaded file does not match | ダウンロードしたファイルのチェックサムが一致しません | Error text (shown in the app window and written to the log). Self-update. |  |
| `errors.update_sum_missing` | The checksum file is missing ({file}) | チェックサムファイルがありません（{file}） | Error text (shown in the app window and written to the log). Self-update; {file} is a file name. |  |
| `errors.update_sum_unreadable` | The checksum file cannot be read ({file}) | チェックサムファイルを読み込めません（{file}） | Error text (shown in the app window and written to the log). Self-update; {file} is a file name. |  |
| `errors.update_unavailable` | Updates are not available in this version of the program. | このバージョンのプログラムではアップデートを利用できません。 | API error (panel, request page or app window notice). About: a build that cannot update itself. |  |
| `errors.window_only` | This feature is only available in the app window. | この機能はアプリのウィンドウでのみ利用できます。 | API error (panel, request page or app window notice). Remote panel asked for an app-window-only feature. |  |
| `errors.wrong_password` | Wrong password. | パスワードが違います。 | API error (panel, request page or app window notice). Remote panel login. |  |
| `errors.ytdlp_bad_data` | yt-dlp returned invalid data. | yt-dlpが無効なデータを返しました。 | API error (panel, request page or app window notice). Reading a link; yt-dlp is a program name. |  |
| `errors.ytdlp_no_stream` | yt-dlp returned no stream address. | yt-dlpがストリームのアドレスを返しませんでした。 | Reason a video failed (history in the panel); yt-dlp is a program name. |  |
| `errors.ytdlp_unknown` | Unknown yt-dlp error | yt-dlpの不明なエラー | API error (panel, request page or app window notice). Reading a link failed without a reason from yt-dlp. |  |

### history

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `history.empty` | Nothing has been on the channel yet. | まだチャンネルで何も再生されていません。 | New interface, History view: shown when there are no finished videos yet. |  |
| `history.play_now` | Play now | 今すぐ再生 | New interface, History view: button on a finished video; puts it on air at once. |  |
| `history.retry` | Try again | 再試行 | New interface, History view: button on a video that failed; puts it on air again. |  |
| `history.status_failed` | Error | エラー | New interface, History view: label of a video that could not be played. |  |
| `history.status_played` | Played | 再生済み | New interface, History view: label of a video that was played to the end. |  |
| `history.status_skipped` | Skipped | スキップ済み | New interface, History view: label of a video that was skipped before its end. | スキップ済み parallels 再生済み. |
| `history.sub` | What has already been on the channel. "Play now" puts a video on air at once. | これまでにチャンネルで流れた動画です。「今すぐ再生」を押すと、その動画をすぐに放送します。 | New interface, History view: explanation under the title. "Play now" is the name of the button. | Quotes the button 「今すぐ再生」 exactly. |
| `history.title` | History | 履歴 | New interface: title of the History view (the same word as the left rail). |  |
| `history.to_queue` | Add to queue | キューに追加 | New interface, History view: button that adds the video's link to the queue again. |  |
| `history.to_queue_local` | Only videos added by a link can be added to the queue again | キューに再び追加できるのは、リンクで追加した動画だけです | New interface, History view: tooltip of the disabled "Add to queue" button on a file from the owner's disk. |  |

### item_errors

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `item_errors.ffmpeg_exit` | ffmpeg ended with code {code} | ffmpegがコード{code}で終了しました | Reason a video failed, kept in the history (panel); ffmpeg is a program name, {code} its exit code. |  |
| `item_errors.ffmpeg_stopped` | ffmpeg stopped | ffmpegが停止しました | Reason a video failed, kept with it in the history (panel). ffmpeg is a program name. Short. |  |
| `item_errors.internal` | Internal error | 内部エラー | Reason a video failed (it kept crashing the channel), kept in the history. Short. |  |
| `item_errors.no_video` | The source has no video track. | ソースに映像トラックがありません。 | Reason a video failed, kept in the history (panel). |  |
| `item_errors.probe_failed` | ffprobe: could not read the source | ffprobe：ソースを解析できませんでした | Reason a video failed, kept in the history (panel); ffprobe is a program name and printed no reason. | 解析 = analyse (ffprobe), not play. |
| `item_errors.skipped` | Skipped by the owner | オーナーがスキップしました | Kept with a skipped video in the history (panel). Impersonal, short (max ~30 characters). | Limit 30 characters (13 used). Names the owner (オーナー), impersonal about viewers. |
| `item_errors.stalled` | The source sent no data for 30 s | ソースから30秒間データが届きませんでした | Reason a video failed, kept in the history (panel). |  |
| `item_errors.start_timeout` | The source took too long to start | ソースの開始に時間がかかりすぎました | Reason a video failed, kept in the history (panel). |  |
| `item_errors.unknown` | Unknown error | 不明なエラー | Reason a video failed without a known cause, kept in the history. Short. |  |

### library

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `library.title` | Files on disk | ディスク上のファイル | New interface, Channel view of a server without a window (headless, MEDIA_DIRS set): heading of the block under the link field that lists the video files in the folders the server may play from. Short heading (shown in capitals). | Added in the final wave (headless file browser). |
| `library.folder` | Folder | フォルダー | Files on disk (headless server): accessible name of the list that picks one of the root folders (MEDIA_DIRS). One word. |  |
| `library.root` | Root folder | ルートフォルダー | Files on disk (headless server): the line that says where the list is, while it shows the top of a root folder (the same term as in errors.no_such_root). | Same term as errors.no_such_root (ルートフォルダー). |
| `library.you_are_in` | You are in: {path} | 現在のフォルダー：{path} | Files on disk (headless server): the line that says which folder the list shows; {path} is the folder's path inside the root folder, with / between the folders. | No pronoun: 現在のフォルダー instead of "You are in". |
| `library.up` | Up one folder | 1つ上のフォルダーへ | Files on disk (headless server): button above the list that goes back to the folder that contains this one. Short button text. |  |
| `library.add` | Add to queue | キューに追加 | Files on disk (headless server): small button at the end of a video file's row that adds that file to the queue. Short button text (the panel is narrow). |  |
| `library.add_folder` | Add the whole folder | フォルダー全体を追加 | Files on disk (headless server): button next to the root folder list that adds every video file of the folder shown (not its subfolders). Disabled when the folder has no video files. |  |
| `library.empty` | No videos or folders here. | このフォルダーには動画もサブフォルダーもありません。 | Files on disk (headless server): shown instead of the list when the folder has neither video files nor subfolders. |  |
| `library.size` | {size} GB | {size} GB | Files on disk (headless server): size of a video file in its row; {size} is a number with two decimals in the language's number format (e.g. 1.50 / 1,50). Keep the unit short. | Space between the number and the Latin unit. |

### log

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `log.auto_pause_no_viewers` | Automatic pause: nobody has been watching for {seconds} s — videos wait for a viewer. | 自動一時停止：{seconds}秒間誰も視聴していません。視聴者が来るまで動画は待機します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The channel paused itself. |  |
| `log.auto_pause_no_viewers_end` | The pause caused by lack of viewers ended. | 視聴者不在による一時停止が終了しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The channel went on. |  |
| `log.auto_pause_tunnel` | Automatic pause: the tunnel has been down for {seconds} s — videos wait for the internet. | 自動一時停止：トンネルが{seconds}秒間切断されています。インターネットに再接続されるまで動画は待機します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The channel paused itself. |  |
| `log.auto_pause_tunnel_end` | The pause caused by the tunnel ended. | トンネルによる一時停止が終了しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The channel went on. |  |
| `log.autostart_refresh_failed` | Could not refresh the start with Windows entry: {error} | 自動起動の登録を更新できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the system error. |  |
| `log.bad_segment_skipped` | Skipped a damaged segment {path}: {error} | 破損したセグメント{path}をスキップしました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). A piece of the stream was unusable. |  |
| `log.banner_admin` | &nbsp;&nbsp;Admin panel:&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;{url} | &nbsp;&nbsp;管理パネル：&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;{url} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line; keep the leading spaces and align the address with the other banner lines. | Start-up banner: the address starts at display column 26, counting full-width characters as two columns (as a console shows them). In the Log tab the Japanese glyphs are proportional, so the column is approximate there. |
| `log.banner_builtin_tunnel` | &nbsp;&nbsp;VRChat link: through the built-in tunnel (the address is in the "Tunnel: online" entry). | &nbsp;&nbsp;VRChat用リンク：内蔵トンネル経由（アドレスは「トンネル：オンライン」の項目にあります）。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line; keep the leading spaces. It points to the entry log.tunnel_online. | Quotes log.tunnel_online 「トンネル：オンライン」. |
| `log.banner_lan` | &nbsp;&nbsp;On the home network:&nbsp;&nbsp;&nbsp;&nbsp;{url} | &nbsp;&nbsp;ホームネットワーク：&nbsp;&nbsp;&nbsp;&nbsp;{url} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line; keep the leading spaces. |  |
| `log.banner_local_link` | &nbsp;&nbsp;Channel link (local):&nbsp;&nbsp;&nbsp;{url} | &nbsp;&nbsp;ローカルリンク：&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;{url} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line; keep the leading spaces. | Shortened to ローカルリンク (the natural チャンネルのリンク（ローカル） does not fit before column 26). |
| `log.banner_on` | &nbsp;&nbsp;The channel is on. | &nbsp;&nbsp;チャンネルが起動しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line; keep the leading spaces. |  |
| `log.banner_public_needed` | &nbsp;&nbsp;VRChat needs a public address, e.g.: | &nbsp;&nbsp;VRChatには公開アドレスが必要です。例： | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line of the headless server; an example command follows. Keep the leading spaces. |  |
| `log.banner_requests` | &nbsp;&nbsp;Request page:&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;{url} | &nbsp;&nbsp;リクエストページ：&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;{url} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line; keep the leading spaces. |  |
| `log.banner_vrchat_link` | &nbsp;&nbsp;VRChat link:&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;{url} | &nbsp;&nbsp;VRChat用リンク：&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;{url} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start-up banner line; keep the leading spaces. |  |
| `log.cannot_create_dir` | Cannot create the folder {path}: {error} | フォルダー{path}を作成できません：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the system error. |  |
| `log.channel_command_failed` | A channel command failed: {error} | チャンネルのコマンドが失敗しました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.channel_command_skipped` | The channel is not running — command {command} skipped. | チャンネルが停止しているため、コマンド{command}をスキップしました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {command} is an internal name, keep it. |  |
| `log.channel_error` | Channel error: {error} | チャンネルのエラー：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.channel_error_pause` | Channel error during a pause: {error} | 一時停止中のチャンネルのエラー：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.channel_error_seek` | Channel error during a seek: {error} | シーク中のチャンネルのエラー：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.channel_loop_failed` | The channel loop ended with an error: {error} — restarting | チャンネルのループがエラーで終了したため、再起動します：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). The channel restarts itself; {error} is the reason. |  |
| `log.channel_loop_not_running` | The channel loop is not running. | チャンネルのループが動作していません。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Slates could not be refreshed because the channel is down. |  |
| `log.channel_not_started` | The channel will not start: the tools (ffmpeg) are missing. The pages and the panel work. | ツール（ffmpeg）がないため、チャンネルは起動しません。ページとパネルは動作します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start of the server without tools. |  |
| `log.channel_start_failed` | The channel did not start: {reason}. The server runs without the channel — repair the tools in the settings. | チャンネルが起動しませんでした：{reason}。サーバーはチャンネルなしで動作しています。設定でツールを修復してください。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {reason} is why. |  |
| `log.channel_stop_error` | Error while stopping the channel: {error} | チャンネルの停止中にエラーが発生しました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.clip_action_copy` | repackaging | 変換なしでコピー | Log entry (Settings, Log tab; the log file on disk keeps the English text). The video is copied without re-encoding (a word in log.clip_start). | Inserted in log.clip_start: 動画#1「…」：変換なしでコピー（h264 1080p 30 fps）. Alt.: そのままコピー, 再パッケージ. |
| `log.clip_action_transcode` | transcoding | 変換 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The video is re-encoded (a word in log.clip_start). |  |
| `log.clip_failed` | Video #{id} "{title}": {error} | 動画#{id}「{title}」：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). A video failed to start or play; {error} is the reason. |  |
| `log.clip_no_audio` | , no audio | 、音声なし | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.clip_start; keep the leading comma. | Leading comma kept as the Japanese 、 (inside the parentheses of log.clip_start). |
| `log.clip_resume` | , resuming from {seconds} s | 、{seconds}秒から再開 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.clip_start; keep the leading comma. | Leading 、 follows the closing parenthesis of log.clip_start. |
| `log.clip_start` | Video #{id} "{title}": {action} ({codec} {height}p {fps} fps{silent}){resume} | 動画#{id}「{title}」：{action}（{codec} {height}p {fps} fps{silent}）{resume} | Log entry (Settings, Log tab; the log file on disk keeps the English text). A video starts airing. {action} is log.clip_action_copy or log.clip_action_transcode; {silent} and {resume} are log.clip_no_audio / log.clip_resume or empty. |  |
| `log.closing` | Shutting down... | 終了しています… | Log entry (Settings, Log tab; the log file on disk keeps the English text). The server is shutting down. |  |
| `log.cloudflared_missing` | Tools: the cloudflared program is missing (new in this version) — downloading it in the background. | ツール：cloudflaredがありません（このバージョンで追加されたツールです）。バックグラウンドでダウンロードしています。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). cloudflared is a program name. |  |
| `log.cloudflared_retry` | Tools: downloading the missing cloudflared program again. | ツール：不足しているcloudflaredを再度ダウンロードしています。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). cloudflared is a program name. |  |
| `log.command_not_sent` | Cannot pass the command {command} to the channel: {error} | コマンド{command}をチャンネルに渡せません：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {command} is an internal name, keep it. |  |
| `log.config_bad_kept` | The corrupt file was kept as {path}. | 破損したファイルは{path}として保存しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.config_corrupt. |  |
| `log.config_bad_not_kept` | The corrupt file could not be kept. | 破損したファイルを保存できませんでした。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.config_corrupt. |  |
| `log.config_corrupt` | The file {file} is corrupt ({error}). {kept} Default settings are used. | ファイル{file}が破損しています（{error}）。{kept}既定の設定を使用します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {kept} is log.config_bad_kept or log.config_bad_not_kept. |  |
| `log.config_unreadable` | Cannot read {path}: {error}. Default settings are used. | {path}を読み込めません：{error}。既定の設定を使用します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The settings file config.json could not be read. |  |
| `log.encoder_test` | Graphics card test: {encoder} | グラフィックボードのテスト：{encoder} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {encoder} is a technical name (libx264, h264_nvenc, ...), keep it. |  |
| `log.fake_resolver` | Warning: the fake resolver is active ({path}) — this is a test mode. | 警告：テスト用の偽リゾルバーが有効です（{path}）。これはテストモードです。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Developer test mode; {path} is a file. |  |
| `log.files_add_failed` | Could not add the files: {error} | ファイルを追加できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Files dropped on the window; {error} is the reason. |  |
| `log.files_added` | Added from disk: {added} (skipped: {skipped}) | ディスクから追加：{added}件（追加されなかったファイル：{skipped}件） | Log entry (Settings, Log tab; the log file on disk keeps the English text). Files dropped on the window; the numbers are counts. |  |
| `log.generating_slates` | Generating the slates... | 待機画面を生成しています… | Log entry (Settings, Log tab; the log file on disk keeps the English text). Start of the channel; slates are the still pictures shown between videos. |  |
| `log.headless_config_error` | <br>Configuration error: {error}<br> | <br>設定エラー：{error}<br> | Log entry (Settings, Log tab; the log file on disk keeps the English text). Printed to the console by the headless server, not to the log. |  |
| `log.headless_tools_missing` | <br>Tools missing: {tools}<br>Run first: vrcinema setup<br> | <br>ツールがありません：{tools}<br>先に次のコマンドを実行してください：vrcinema setup<br> | Log entry (Settings, Log tab; the log file on disk keeps the English text). Printed to the console by the headless server, not to the log. Keep the command "vrcinema setup". |  |
| `log.headless_unknown_command` | Unknown command: {command}<br>Usage: vrcinema [setup [--force] [--no-env]] | 不明なコマンド：{command}<br>使い方：vrcinema [setup [--force] [--no-env]] | Log entry (Settings, Log tab; the log file on disk keeps the English text). Printed to the console by the headless server, not to the log. Keep the usage line. |  |
| `log.item_fail_mark_failed` | Could not mark video #{id} as failed: {error} | 動画#{id}を失敗として記録できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.item_keeps_crashing` | Video #{id} broke the channel loop {count} times in a row — marking it as failed: {reason} | 動画#{id}がチャンネルのループを{count}回連続で停止させたため、失敗として扱います：{reason} | Log entry (Settings, Log tab; the log file on disk keeps the English text). The channel gave up on a video; {count} is a number. |  |
| `log.job_assign_failed` | cannot assign cloudflared to its Windows job object ({detail}) — it stops on cancel but may survive a sudden close of the program | cloudflaredをWindowsのジョブオブジェクトに割り当てられません（{detail}）。キャンセル時には停止しますが、プログラムが突然終了した場合は残る可能性があります | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.named_job_warning. |  |
| `log.job_containment_failed` | Warning: cannot tie the child processes to the server: {error} | 警告：子プロセスをサーバーに関連付けられません：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Headless server start (Windows job object); {error} is the system error. |  |
| `log.job_create_failed` | cannot create a separate Windows job object for cloudflared ({detail}) — it stops on cancel but may survive a sudden close of the program | cloudflared用に別のWindowsジョブオブジェクトを作成できません（{detail}）。キャンセル時には停止しますが、プログラムが突然終了した場合は残る可能性があります | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.named_job_warning. |  |
| `log.job_no_breakaway` | the program runs in a Windows job object that does not let a process be separated — the browser opened by cloudflared may be closed together with the program | プログラムがプロセスの分離を許可しないWindowsジョブオブジェクト内で動作しているため、cloudflaredが開いたブラウザーがプログラムと一緒に閉じられる可能性があります | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.named_job_warning. |  |
| `log.media_dir_missing` | Warning: a folder from MEDIA_DIRS does not exist: {path} | 警告：MEDIA_DIRSのフォルダーが存在しません：{path} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Headless server; MEDIA_DIRS is a setting name, keep it. |  |
| `log.named_already_gone` | Permanent link: the tunnel {tunnel} is already gone from Cloudflare. | 固定リンク：トンネル{tunnel}はすでにCloudflareから削除されています。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_cert_copied` | Permanent link: used the cloudflared certificate that was already in {from} (copied to {to}; the owner's file stays). | 固定リンク：{from}にあったcloudflaredの証明書を使用しました（{to}にコピーしました。元のファイルはそのまま残ります）。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_cert_moved` | Permanent link: connected to Cloudflare; the certificate was moved from {from} to {to}. | 固定リンク：Cloudflareに接続しました。証明書を{from}から{to}に移動しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_cert_save_failed` | Permanent link: cannot save the Cloudflare certificate in {path}: {error} | 固定リンク：Cloudflareの証明書を{path}に保存できません：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_create_failed` | Permanent link: creating the tunnel failed: {reason} | 固定リンク：トンネルの作成に失敗しました：{reason} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_credentials_failed` | Permanent link: fetching the credentials failed: {reason} | 固定リンク：資格情報の取得に失敗しました：{reason} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_delete_failed` | Permanent link: deleting the tunnel failed: {reason} | 固定リンク：トンネルの削除に失敗しました：{reason} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_deleted` | Permanent link: the tunnel {tunnel} was deleted in Cloudflare. | 固定リンク：Cloudflareのトンネル{tunnel}を削除しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_dns_failed` | Permanent link: setting {hostname} in DNS failed: {reason} | 固定リンク：DNSへの{hostname}の設定に失敗しました：{reason} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_dns_routed` | Permanent link: {hostname} leads to the tunnel {id}. | 固定リンク：{hostname}はトンネル{id}を指しています。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). | 指しています = points to (DNS). |
| `log.named_dns_routed_replaced` | Permanent link: {hostname} leads to the tunnel {id} (the previous DNS record was replaced). | 固定リンク：{hostname}はトンネル{id}を指しています（以前のDNSレコードは置き換えられました）。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_dns_wrong_domain` | Permanent link: {hostname} is not in the domain you logged in with — Cloudflare set the record {record}. | 固定リンク：{hostname}はログインしたドメインに含まれていません。Cloudflareはレコード{record}を設定しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_files_removed` | Permanent link: the certificate and credentials were removed from {path}. | 固定リンク：証明書と資格情報を{path}から削除しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_job_warning` | Permanent link: {warning}. | 固定リンク：{warning}。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {warning} is log.job_no_breakaway / log.job_create_failed / log.job_assign_failed. |  |
| `log.named_late_cert_failed` | Permanent link: cannot move the certificate from {from} to {to}: {error} | 固定リンク：証明書を{from}から{to}に移動できません：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_late_cert_moved` | Permanent link: the certificate written just before the login was interrupted was moved from {from} to {to}. | 固定リンク：ログインが中断される直前に書き込まれた証明書を{from}から{to}に移動しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_login_cancelled` | Permanent link: the Cloudflare login was cancelled. | 固定リンク：Cloudflareへのログインはキャンセルされました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_login_failed` | Permanent link: the Cloudflare login failed: {reason} | 固定リンク：Cloudflareへのログインに失敗しました：{reason} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_login_no_certificate` | Permanent link: cloudflared finished the login without a certificate in {path}. | 固定リンク：cloudflaredは{path}に証明書を作成しないままログインを終了しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_login_started` | Permanent link: logging in to Cloudflare (cloudflared, pid {pid}) — waiting for confirmation in the browser. | 固定リンク：Cloudflareにログインしています（cloudflared、PID {pid}）。ブラウザーでの確認を待っています。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_login_timeout` | Permanent link: the Cloudflare login did not finish within {duration}. | 固定リンク：Cloudflareへのログインが{duration}以内に完了しませんでした。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {duration} is a time such as "5 min". |  |
| `log.named_remove_files_failed` | Permanent link: could not remove the files from {path}: {error} | 固定リンク：{path}からファイルを削除できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_tunnel_exists` | Permanent link: the tunnel {tunnel} already exists in Cloudflare — using it. | 固定リンク：トンネル{tunnel}はすでにCloudflareにあるため、それを使用します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.named_tunnel_ready` | Permanent link: tunnel {tunnel} ({id}), credentials in {path}. | 固定リンク：トンネル{tunnel}（{id}）、資格情報の場所：{path}。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.panel_open_failed` | Could not open the panel: {error} | パネルを開けませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the system error. |  |
| `log.pause` | Pause ({reason}): video #{id} "{title}" held at {seconds} s | 一時停止（{reason}）：動画#{id}「{title}」を{seconds}秒の位置で停止 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {reason} is a technical word (manual, tunnel, no-viewers), keep it. | {reason} is log.pause_reason_* (手動 / 視聴者なし / トンネル). |
| `log.pause_manual` | Manual pause. | 手動で一時停止しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The owner paused the channel. |  |
| `log.pause_manual_end` | The manual pause ended. | 手動の一時停止が終了しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The owner resumed the channel. |  |
| `log.pause_reason_manual` | manual | 手動 | Word used inside log entries (Settings, Log tab). Why the channel is paused; lowercase, used in log.pause as {reason}. |  |
| `log.pause_reason_no_viewers` | no viewers | 視聴者なし | Word used inside log entries (Settings, Log tab). Why the channel is paused; lowercase, used in log.pause as {reason}. |  |
| `log.pause_reason_tunnel` | tunnel | トンネル | Word used inside log entries (Settings, Log tab). Why the channel is paused; lowercase, used in log.pause as {reason}. |  |
| `log.play_now` | Play now: "{title}" (#{id}) | 今すぐ再生：「{title}」（#{id}） | Log entry (Settings, Log tab; the log file on disk keeps the English text). The owner chose a video to play at once. |  |
| `log.program_stopped` | Program stopped. | プログラムを停止しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The program has stopped cleanly. |  |
| `log.queue_error_prefetch` | Queue error while looking ahead: {error} | 次の動画の事前準備中にキューのエラーが発生しました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). The next video is prepared in advance; {error} is the reason. |  |
| `log.request_received` | Request from viewer {viewer}: "{title}" ({status}) | 視聴者{viewer}からのリクエスト：「{title}」（{status}） | Log entry (Settings, Log tab; the log file on disk keeps the English text). A viewer asked for a video. {viewer} is an anonymous tag of 8 hex digits (never an address), keep it as it is; {status} is log.status_queued / log.status_pending. |  |
| `log.restart_failed_reverting` | The server did not start with the new settings: {error}. Restoring the previous ones. | 新しい設定でサーバーが起動しませんでした：{error}。以前の設定に戻します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.resume` | Resuming: video #{id} "{title}" from {seconds} s | 再開：動画#{id}「{title}」を{seconds}秒の位置から | Log entry (Settings, Log tab; the log file on disk keeps the English text). The paused video goes on. |  |
| `log.seek` | Seek: video #{id} "{title}" from {from} s to {to} s | シーク：動画#{id}「{title}」を{from}秒から{to}秒へ | Log entry (Settings, Log tab; the log file on disk keeps the English text). The owner jumped within the video; the numbers are seconds. |  |
| `log.seek_not_allowed` | Seek {request} skipped: not allowed for this video | シーク{request}をスキップしました：この動画では許可されていません | Log entry (Settings, Log tab; the log file on disk keeps the English text). {request} is an internal description of the jump, keep it. |  |
| `log.seek_nothing_to_seek` | Seek {request} skipped: nothing is playing or it is a live stream | シーク{request}をスキップしました：再生中の動画がないか、ライブ配信です | Log entry (Settings, Log tab; the log file on disk keeps the English text). {request} is an internal description of the jump, keep it. |  |
| `log.seek_to_end` | Seek to the end of video #{id} "{title}" — video skipped | 動画#{id}「{title}」の最後までシークしたため、次の動画に進みます | Log entry (Settings, Log tab; the log file on disk keeps the English text). A jump to the very end ends the video. |  |
| `log.seek_to_end_paused` | Seek to the end of paused video #{id} "{title}" — video skipped | 一時停止中の動画#{id}「{title}」の最後までシークしたため、次の動画に進みます | Log entry (Settings, Log tab; the log file on disk keeps the English text). A jump to the very end of a paused video ends it. |  |
| `log.segment_source_error` | Segment source error: {error} | セグメントのソースのエラー：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). The channel could not produce the next piece of the stream. |  |
| `log.server_aborted` | The server stopped working: {error} | サーバーが動作を停止しました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.server_ended_with_error` | The server ended with an error: {error} | サーバーがエラーで終了しました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.server_error` | Server error: {error} | サーバーエラー：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). An unexpected error while answering a request. |  |
| `log.server_restarting` | Restarting the server... | サーバーを再起動しています… | Log entry (Settings, Log tab; the log file on disk keeps the English text). The server restarts (new port or network setting). |  |
| `log.server_start_failed` | The server did not start: {error} | サーバーが起動しませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.server_stop_timeout` | The server did not stop in time — aborting it. | サーバーが時間内に停止しなかったため、強制終了します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The server was forced to stop. |  |
| `log.server_without_channel` | The server runs without the channel: {url} | サーバーはチャンネルなしで動作しています：{url} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {url} is the admin panel address. |  |
| `log.session_secret_missing` | Warning: SESSION_SECRET is missing in .env — you will have to log in again after a restart. | 警告：.envにSESSION_SECRETがありません。再起動後に再度ログインが必要になります。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Headless server start; SESSION_SECRET and .env are names, keep them. |  |
| `log.setup_already` | {tool} is already there. | {tool}はすでにあります。 | Console message of the `vrcinema setup` command. {tool} is a program name. |  |
| `log.setup_done` | Done. | 完了しました。 | Console message of the `vrcinema setup` command. |  |
| `log.setup_downloading` | Downloading {url} | {url}をダウンロードしています | Console message of the `vrcinema setup` command. |  |
| `log.setup_env_created` | Created .env from .env.example — set ADMIN_PASSWORD in it. | .env.exampleから.envを作成しました。ADMIN_PASSWORDを設定してください。 | Console message of the `vrcinema setup` command. Keep the names .env, .env.example and ADMIN_PASSWORD. |  |
| `log.setup_unpacking` | Unpacking ffmpeg... | ffmpegを展開しています… | Console message of the `vrcinema setup` command. |  |
| `log.setup_version_unknown` | error | エラー | Console message of the `vrcinema setup` command. Printed after the program name when its version could not be read. |  |
| `log.skip_server_down` | Cannot skip the video: the server is not running. | 動画をスキップできません：サーバーが動作していません。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Tray menu "skip". |  |
| `log.slate_empty` | The slate "{kind}" has no segments. | 待機画面「{kind}」にセグメントがありません。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {kind} is a technical word (idle, buffering, pause). |  |
| `log.slate_failed` | Could not generate the slate "{kind}": {message} | 待機画面「{kind}」を生成できませんでした：{message} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {kind} is a technical word (idle, buffering, pause). |  |
| `log.slate_qr_failed` | QR code: {error} | QRコード：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.slate_failed. |  |
| `log.slate_qr_save_failed` | Saving the QR code: {error} | QRコードの保存：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.slate_failed. |  |
| `log.slates_font_missing` | The slate font ({path}) was not found, so the slates have no text. Set FONT_FILE in .env. | 待機画面のフォント（{path}）が見つからないため、待機画面に文字が表示されません。.envでFONT_FILEを設定してください。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {path} is the font file path. FONT_FILE is a setting name in the .env file - keep it as is. |  |
| `log.slates_japanese_font_missing` | No Japanese font (YuGothM.ttc, meiryo.ttc) in the Windows Fonts folder. The slates are in English. | Windowsのフォントフォルダーに日本語フォント（YuGothM.ttc、meiryo.ttc）がありません。待機画面は英語で表示されます。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Shown when the slate language is Japanese but Windows has no Japanese font; the slates then stay in English. YuGothM.ttc and meiryo.ttc are file names - keep them. |  |
| `log.slates_refresh_failed` | Could not refresh the slates: {reason}. The previous slates stay. | 待機画面を更新できませんでした：{reason}。以前の待機画面をそのまま使用します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {reason} is why. |  |
| `log.slates_refreshed` | The slates were refreshed (set {generation}). | 待機画面を更新しました（セット{generation}）。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). Written whenever a new slate set goes on air: for a new public address, a new slate language or while the tunnel connects, so it names no reason. {generation} is a number. |  |
| `log.start` | VRCinema {version}: start | VRCinema {version}：起動 | Log entry (Settings, Log tab; the log file on disk keeps the English text). First entry of a run; {version} is the program version. |  |
| `log.started_after_update` | Started after an update. | アップデート後に起動しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The program was restarted by its own update. |  |
| `log.status_failed` | failed | 失敗 | Word used inside log entries (Settings, Log tab). Status of a queue item; lowercase, fits after "(". |  |
| `log.status_pending_approval` | awaiting approval | 承認待ち | Word used inside log entries (Settings, Log tab). Status of a queue item; lowercase, fits after "(". |  |
| `log.status_played` | played | 再生済み | Word used inside log entries (Settings, Log tab). Status of a queue item; lowercase, fits after "(". |  |
| `log.status_playing` | playing | 再生中 | Word used inside log entries (Settings, Log tab). Status of a queue item; lowercase, fits after "(". |  |
| `log.status_queued` | queued | 再生待ち | Word used inside log entries (Settings, Log tab). Status of a queue item; lowercase, fits after "(". | 再生待ち = waiting to be played (same as the queue heading queue.section_queued). |
| `log.status_rejected` | rejected | 却下 | Word used inside log entries (Settings, Log tab). Status of a queue item; lowercase, fits after "(". |  |
| `log.status_removed` | removed | 削除済み | Word used inside log entries (Settings, Log tab). Status of a queue item; lowercase, fits after "(". |  |
| `log.stop_too_slow` | Stopping takes too long (over {seconds} s) — finishing without waiting. | 終了に時間がかかりすぎています（{seconds}秒以上）。待たずに終了します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The program is quitting; {seconds} is a number with a decimal. |  |
| `log.ticker_applied` | Info ticker settings changed: video #{id} "{title}" starts again from {seconds} s. | テロップの設定が変更されました：動画#{id}「{title}」を{seconds}秒の位置から再開します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The owner changed how the info ticker looks; the video on air is restarted from where it was so the change shows at once. |  |
| `log.ticker_failed_plain` | Video #{id} "{title}" could not start with the info ticker ({error}). It starts again without the ticker. | 動画#{id}「{title}」をテロップ付きで開始できませんでした（{error}）。テロップなしで再開します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). ffmpeg failed to draw the info ticker (e.g. a broken font); the video is started again without it. {error} is ffmpeg's message. |  |
| `log.ticker_font_missing` | The info ticker font ({path}) was not found: video #{id} "{title}" plays without the ticker. Set FONT_FILE in .env. | テロップのフォント（{path}）が見つからないため、動画#{id}「{title}」はテロップなしで再生されます。.envでFONT_FILEを設定してください。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The font file for the info ticker is missing; {path} is a file path. FONT_FILE and .env are names - keep them. |  |
| `log.ticker_live_next_item` | Info ticker settings changed: a live stream is not restarted, the change shows from the next video. | テロップの設定が変更されました：ライブ配信は再開されないため、変更は次の動画から反映されます。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The owner changed how the info ticker looks while a live stream airs. |  |
| `log.ticker_slow` | The computer cannot keep up with the info ticker (video #{id} "{title}" converts at {speed}× for 30 s). Switch the ticker off or turn on conversion on the graphics card. | PCの処理がテロップに追いついていません（動画#{id}「{title}」は30秒間、変換速度が{speed}倍速です）。テロップをオフにするか、グラフィックボードでの変換をオンにしてください。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The conversion that drawing the info ticker needs is slower than real time; {speed} is a number like 0.85. |  |
| `log.ticker_text_failed` | Could not update the info ticker text: {error} | テロップのテキストを更新できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is a system error. |  |
| `log.tools_error` | Tools: {error} | ツール：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason a tools job could not start. |  |
| `log.tools_job_done` | Tools ({what}): done. | ツール（{what}）：完了しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {what} is log.tools_job_install / update / repair. |  |
| `log.tools_job_failed` | Tools ({what}): error — {error} | ツール（{what}）：エラー：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {what} is log.tools_job_install / update / repair; {error} is the reason. | Two colons (エラー：{error}) because en uses a dash; alt.: ツール（{what}）でエラーが発生しました：{error}. |
| `log.tools_job_install` | downloading | ダウンロード | Log entry (Settings, Log tab; the log file on disk keeps the English text). A word in log.tools_job_done / log.tools_job_failed: the tools were being downloaded. |  |
| `log.tools_job_repair` | repair | 修復 | Log entry (Settings, Log tab; the log file on disk keeps the English text). A word in log.tools_job_done / log.tools_job_failed: the tools were being repaired. |  |
| `log.tools_job_update` | update | 更新 | Log entry (Settings, Log tab; the log file on disk keeps the English text). A word in log.tools_job_done / log.tools_job_failed: a tool was being updated. |  |
| `log.tools_no_checksums` | Tools: the publisher gives no checksums for {tool}; installing without verification. | ツール：配布元が{tool}のチェックサムを提供していないため、検証せずにインストールします。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {tool} is a program name. |  |
| `log.tools_resumed_file_mismatch` | Tools: the resumed file {file} does not match its checksum (the publisher may have replaced it) — downloading again. | ツール：再開したファイル{file}がチェックサムと一致しません（配布元がファイルを差し替えた可能性があります）。もう一度ダウンロードします。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {file} is a file name. |  |
| `log.tray_copy_failed` | Could not copy the link: {error} | リンクをコピーできませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the system error. |  |
| `log.tray_create_failed` | Could not create the tray icon: {error} | 通知領域のアイコンを作成できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the system error. |  |
| `log.tray_menu_failed` | Could not update the tray menu: {error} | 通知領域のメニューを更新できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). The tray menu could not follow a change of language; {error} is the system error. |  |
| `log.tunnel_connection_lost` | Tunnel: cloudflared lost its connection to Cloudflare; waiting for it to come back. | トンネル：cloudflaredとCloudflareの接続が切れました。復旧を待っています。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.tunnel_describe_named` | permanent tunnel {hostname} to {local}, credentials {credentials} | {local}への固定トンネル{hostname}、資格情報：{credentials} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_started; {credentials} is a file path. |  |
| `log.tunnel_describe_quick` | quick tunnel to {local} | {local}へのクイックトンネル | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_started. |  |
| `log.tunnel_exit_code` | code {code} | コード{code} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_reason_exited. |  |
| `log.tunnel_exit_no_code` | no code | コードなし | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_reason_exited. |  |
| `log.tunnel_online` | Tunnel: online — {url} | トンネル：オンライン（{url}） | Log entry (Settings, Log tab; the log file on disk keeps the English text). The public address of the channel. | en "Tunnel: online — {url}"; Japanese has no en dash, so the address is in parentheses. |
| `log.tunnel_reason_exited` | cloudflared exited ({code}) | cloudflaredが終了しました（{code}） | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_retry; {code} is log.tunnel_exit_code or log.tunnel_exit_no_code. |  |
| `log.tunnel_reason_exited_with_output` | cloudflared exited ({code}): {output} | cloudflaredが終了しました（{code}）：{output} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_retry; {output} is its last line, in English. |  |
| `log.tunnel_reason_no_address` | no tunnel address after {seconds} s | {seconds}秒経ってもトンネルのアドレスがありません | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_retry. |  |
| `log.tunnel_reason_no_connection` | no connection to Cloudflare for {seconds} s although cloudflared is running | cloudflaredは動作していますが、{seconds}秒間Cloudflareに接続できていません | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_retry. |  |
| `log.tunnel_reason_rate_limited` | Cloudflare refused the quick tunnel (429 Too Many Requests) | Cloudflareがクイックトンネルを拒否しました（429 Too Many Requests） | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_retry; keep "429 Too Many Requests". |  |
| `log.tunnel_reason_start_failed` | cannot start cloudflared: {detail} | cloudflaredを起動できません：{detail} | Log entry (Settings, Log tab; the log file on disk keeps the English text). Part of log.tunnel_retry; starts with a lowercase letter. |  |
| `log.tunnel_retry` | Tunnel: {reason}; trying again in {seconds} s. | トンネル：{reason}。{seconds}秒後に再試行します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {reason} is one of the log.tunnel_reason_* entries. |  |
| `log.tunnel_started` | Tunnel: cloudflared started (pid {pid}, {config}). | トンネル：cloudflaredを起動しました（PID {pid}、{config}）。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). {config} is log.tunnel_describe_quick or log.tunnel_describe_named. |  |
| `log.tunnel_stopped` | Tunnel: cloudflared stopped. | トンネル：cloudflaredを停止しました。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.tunnel_warning` | Tunnel: {warning} | トンネル：{warning} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {warning} is a tunnel status note. |  |
| `log.update_available` | A new version of the program is available: {version} | プログラムの新しいバージョンが利用可能です：{version} | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.update_check_failed` | Checking for updates failed: {error} | アップデートの確認に失敗しました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.update_failed` | The update failed: {error} | アップデートに失敗しました：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |
| `log.update_installed` | Version {version} installed; restarting the program. | バージョン{version}をインストールしました。プログラムを再起動します。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.update_installing` | Installing version {version}... | バージョン{version}をインストールしています… | Log entry (Settings, Log tab; the log file on disk keeps the English text). |  |
| `log.window_controls_failed` | Window: the title bar buttons could not be switched on: {error} | ウィンドウ：タイトルバーのボタンを有効にできませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). The minimize/maximize/close buttons of the window's own title bar; {error} is the system error. |  |
| `log.window_create_failed` | Could not create the window: {error} | ウィンドウを作成できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the system error. |  |
| `log.window_follows_port` | The server runs on port {port} — switching the window. | サーバーはポート{port}で動作しています。ウィンドウを切り替えます。 | Log entry (Settings, Log tab; the log file on disk keeps the English text). The port changed; {port} is a number. |  |
| `log.window_switch_failed` | Could not switch the window: {error} | ウィンドウを切り替えられませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the system error. |  |
| `log.ytdlp_update` | yt-dlp: {line} | yt-dlp：{line} | Log entry (Settings, Log tab; the log file on disk keeps the English text). The last line yt-dlp printed when updating itself; {line} is in English from the program, keep the entry as it is. |  |
| `log.ytdlp_update_failed` | Could not update yt-dlp: {error} | yt-dlpを更新できませんでした：{error} | Log entry (Settings, Log tab; the log file on disk keeps the English text). {error} is the reason. |  |

### login

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `login.hint` | Enter the remote panel password set in VRCinema on the computer (Settings → Remote panel). | PCのVRCinemaで設定したリモートパネルのパスワードを入力してください（設定 → リモートパネル）。 | Sign-in screen of the remote panel, under the heading. 'Settings → Remote panel' names the place in the program; keep the arrow. |  |
| `login.language` | Language of this panel | このパネルの言語 | Language choice at the bottom of the sign-in screen; it is kept in this browser only. |  |
| `login.password` | Password | パスワード | Label of the password field on the sign-in screen. |  |
| `login.signing_in` | Signing in… | ログイン中… | The sign-in button while the password is checked. | Button text while checking: ログイン中…. |
| `login.submit` | Sign in | ログイン | Button of the sign-in screen. |  |
| `login.title` | Sign in to the channel panel | チャンネルのパネルにログイン | Heading of the sign-in screen of the remote panel (a browser on another device, e.g. a phone). |  |
| `login.window_hint` | Close this window and open the panel again from the VRCinema icon in the notification area. | このウィンドウを閉じて、通知領域のVRCinemaアイコンからパネルをもう一度開いてください。 | Text under login.window_title. 'Notification area' is the Windows system tray. |  |
| `login.window_title` | The window's session has ended | ウィンドウのセッションが終了しました | Heading shown in the program's own window when its session ended (it never asks for a password). |  |

### named

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `named.back_to_quick` | The quick tunnel runs (its address changes at every start). The permanent link files stay — you can go back to it. | クイックトンネルが動作しています（アドレスは起動するたびに変わります）。固定リンクのファイルは残っているので、いつでも戻せます。 | Permanent-link wizard (app window, Settings → Internet): result notice after "back to the quick tunnel". |  |
| `named.deleted` | Permanent link deleted — the quick tunnel runs. The DNS record {hostname} may remain in Cloudflare: delete it in the Cloudflare dashboard (DNS) if it is no longer needed. | 固定リンクを削除しました。クイックトンネルが動作しています。CloudflareにDNSレコード{hostname}が残っている場合があります。不要であれば、Cloudflareのダッシュボード（DNS）で削除してください。 | Permanent-link wizard (app window, Settings → Internet): result notice after deleting the permanent link; {hostname} is its address. |  |
| `named.deleted_no_host` | Permanent link deleted — the quick tunnel runs. The DNS record of the permanent link may remain in Cloudflare: delete it in the Cloudflare dashboard (DNS) if it is no longer needed. | 固定リンクを削除しました。クイックトンネルが動作しています。Cloudflareに固定リンクのDNSレコードが残っている場合があります。不要であれば、Cloudflareのダッシュボード（DNS）で削除してください。 | Permanent-link wizard (app window, Settings → Internet): like named.deleted when the address is not known. |  |
| `named.logged_in` | Connected to Cloudflare. Now enter the permanent link address. | Cloudflareに接続しました。固定リンクのアドレスを入力してください。 | Permanent-link wizard (app window, Settings → Internet): result of step 1 (login). |  |
| `named.logged_in_before` | Connected to Cloudflare through an earlier login of cloudflared on this computer: the account and domain chosen then apply. Now enter the permanent link address in that domain. (To use another account, delete the certificate from the .cloudflared folder in your profile and connect again.) | このPCで以前に行ったcloudflaredのログインを使って、Cloudflareに接続しました。そのときに選んだアカウントとドメインが使用されます。そのドメイン内で固定リンクのアドレスを入力してください。（別のアカウントを使う場合は、ユーザープロファイルの.cloudflaredフォルダーから証明書を削除して、もう一度接続してください。） | Permanent-link wizard (app window, Settings → Internet): result of step 1 when an older certificate was reused; .cloudflared is a folder name. | ユーザープロファイル = the Windows user profile folder (%USERPROFILE%). |
| `named.no_reason` | cloudflared gave no reason | cloudflaredから理由は示されませんでした | Permanent-link wizard (app window, Settings → Internet): used as {reason} when cloudflared printed nothing. |  |
| `named.ready` | Permanent link {url} is ready — the tunnel is connecting to Cloudflare. | 固定リンク{url}の準備ができました。トンネルはCloudflareに接続しています。 | Permanent-link wizard (app window, Settings → Internet): result of step 2; {url} is the new address. |  |
| `named.step.creating_tunnel` | Creating the tunnel {tunnel} in Cloudflare… | Cloudflareにトンネル{tunnel}を作成しています… | Permanent-link wizard (app window, Settings → Internet): progress line; {tunnel} is the tunnel name. |  |
| `named.step.deleting` | Deleting the tunnel {tunnel} in Cloudflare… | Cloudflareのトンネル{tunnel}を削除しています… | Permanent-link wizard (app window, Settings → Internet): progress line. |  |
| `named.step.fetching_credentials` | The tunnel {tunnel} already exists — fetching its credentials… | トンネル{tunnel}はすでにあります。資格情報を取得しています… | Permanent-link wizard (app window, Settings → Internet): progress line. |  |
| `named.step.routing_dns` | Setting the address {hostname} in Cloudflare DNS… | Cloudflare DNSにアドレス{hostname}を設定しています… | Permanent-link wizard (app window, Settings → Internet): progress line; {hostname} is the chosen address. |  |
| `named.step.starting` | Starting the Cloudflare login… | Cloudflareへのログインを開始しています… | Permanent-link wizard (app window, Settings → Internet): progress line. |  |
| `named.step.switching` | Switching the tunnel… | トンネルを切り替えています… | Permanent-link wizard (app window, Settings → Internet): progress line. |  |
| `named.step.waiting_for_browser` | Log in to Cloudflare in the browser and choose the domain for the permanent link. | ブラウザーでCloudflareにログインし、固定リンクに使うドメインを選んでください。 | Permanent-link wizard (app window, Settings → Internet): progress line while the browser login page is open. |  |

### nav

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `nav.channel` | Channel | チャンネル | New interface, left rail: label under the icon of the main view (preview, up next, add link). Max ~12 characters (72 px wide rail). | Left rail, 72 px: 5 full-width characters fit. |
| `nav.history` | History | 履歴 | New interface, left rail: label of the view with the videos that already played. Max ~12 characters. |  |
| `nav.label` | Views | ビュー | New interface, left rail: accessible name of the list of views (read by screen readers, not shown). | Screen-reader only. Alt.: 表示の切り替え. |
| `nav.pending` | one: {count} video waiting for approval<br>other: {count} videos waiting for approval | other: 承認待ちの動画：{count}本 | New interface, left rail: tooltip and screen-reader text of the red counter on "Channel" (videos added by viewers that need the owner's approval). | Tooltip of the red counter; Japanese has only the `other` plural form. |
| `nav.settings` | Settings | 設定 | New interface, left rail: label of the settings view (bottom of the rail). Max ~13 characters; a soft hyphen (U+00AD) may be used to allow a break. |  |
| `nav.share` | Share | 共有 | New interface, left rail: label of the view with the channel link, the request page and its QR code. Max ~12 characters. |  |

### net

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `net.connecting` | Connecting | 接続中 | New interface, player bar: status pill while the tunnel connects. Max ~16 characters. | Status pill; no ellipsis like en. |
| `net.local` | Local only | ローカルのみ | New interface, player bar: status pill when the tunnel is off and there is no public address. Max ~16 characters. |  |
| `net.manual` | Manual address | 手動アドレス | New interface, player bar: status pill when the tunnel is off and the owner typed a public address. Max ~16 characters. |  |
| `net.offline` | No internet | インターネットなし | New interface, player bar: status pill when the tunnel is lost or failed. Max ~16 characters. | Status pill: インターネットなし as in the Windows network icon. Alt.: オフライン. |
| `net.online` | Online | オンライン | New interface, player bar: green status pill — the channel is reachable from the internet. Max ~16 characters. |  |

### permalink

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `permalink.back_named` | Back to the permanent link ({host}) | 固定リンクに戻す（{host}） | New interface, permanent link: button; {host} is the saved address. |  |
| `permalink.back_quick` | Back to the quick tunnel | クイックトンネルに戻す | New interface, permanent link: button that switches to the quick tunnel. |  |
| `permalink.cancel` | Cancel | キャンセル | New interface, permanent link wizard: button. |  |
| `permalink.close` | Close | 閉じる | New interface, permanent link wizard: button shown when it is done. |  |
| `permalink.connect` | Connect to Cloudflare | Cloudflareに接続 | New interface, permanent link wizard, step 1: button. Cloudflare is a company name, keep it. |  |
| `permalink.create` | Create the permanent link | 固定リンクを作成 | New interface, permanent link wizard, step 2: button. |  |
| `permalink.create_failed` | The permanent link could not be created. | 固定リンクを作成できませんでした。 | New interface, permanent link wizard: shown when the engine gave no reason. |  |
| `permalink.creating` | Creating the permanent link… | 固定リンクを作成しています… | New interface, permanent link wizard: progress message. |  |
| `permalink.delete` | Delete the permanent link | 固定リンクを削除 | New interface, permanent link: button. |  |
| `permalink.delete_confirm` | Delete the permanent link? The tunnel is removed from your Cloudflare account, and the certificate and credentials from this computer. If the DNS record of this name stays on the account, delete it by hand in the Cloudflare dashboard. | 固定リンクを削除しますか？　トンネルはCloudflareアカウントから、証明書と資格情報はこのPCから削除されます。この名前のDNSレコードがアカウントに残った場合は、Cloudflareのダッシュボードで手動で削除してください。 | New interface, permanent link: confirmation before deleting. DNS is the domain name system, keep the abbreviation. | A full-width space follows ？ before the next sentence (Japanese typography). |
| `permalink.delete_yes` | Delete the permanent link | 固定リンクを削除 | New interface, permanent link: button that confirms the deletion. |  |
| `permalink.deleting` | Deleting the permanent link… | 固定リンクを削除しています… | New interface, permanent link: progress message. |  |
| `permalink.dns_taken` | The name {host} already has another DNS record in Cloudflare. Replace it so that it points to this channel? | 名前{host}には、Cloudflareに別のDNSレコードがすでにあります。このチャンネルを指すように置き換えますか？ | New interface, permanent link wizard: question after a name conflict; {host} is the address the owner typed. |  |
| `permalink.done` | Done. The channel now has a permanent address: https://{host} | チャンネルの固定リンクを設定しました：https://{host} | New interface, permanent link wizard: shown when it succeeded; {host} is the address without the scheme. |  |
| `permalink.help_domain` | How to add a domain | ドメインを追加する方法 | New interface, permanent link wizard: link to a help page, after a message that the domain is missing in Cloudflare. |  |
| `permalink.help_login` | How to do it | 手順を見る | New interface, permanent link wizard: link to a help page, after a failed login. |  |
| `permalink.host_label` | Permanent link address (e.g. channel.yourdomain.com) | 固定リンクのアドレス（例：channel.yourdomain.com） | New interface, permanent link wizard, step 2: label of the address field. |  |
| `permalink.host_placeholder` | channel.yourdomain.com | channel.yourdomain.com | New interface, permanent link wizard, step 2: example address in the empty field. |  |
| `permalink.intro` | A permanent link keeps the same address after every start of the program. You need a free Cloudflare account with your own domain added. | 固定リンクを使うと、プログラムを起動するたびに同じアドレスが使われます。独自ドメインを追加した無料のCloudflareアカウントが必要です。 | New interface, permanent link wizard: what the permanent link is and what it needs. |  |
| `permalink.login_cancelled` | The login was cancelled. | ログインはキャンセルされました。 | New interface, permanent link wizard: shown after the owner cancelled the Cloudflare login. |  |
| `permalink.login_failed` | The login did not work. | ログインできませんでした。 | New interface, permanent link wizard: shown when the Cloudflare login failed without a reason. |  |
| `permalink.login_link` | open the login page | ログインページを開く | New interface, permanent link wizard: link text, follows "If the page did not open:". | Link after permalink.login_link_lead 「ページが開かない場合：」. |
| `permalink.login_link_lead` | If the page did not open: | ページが開かない場合： | New interface, permanent link wizard: followed by a link ("open the login page"). |  |
| `permalink.mode_named` | Permanent link: https://{host}. The address does not change. | 固定リンク：https://{host}。アドレスは変わりません。 | New interface, Share view: the channel uses a permanent link; {host} is its address. |  |
| `permalink.mode_quick` | Quick link: the address is random (trycloudflare.com) and changes every time the program starts. | クイックリンク：アドレスはランダム（trycloudflare.com）で、プログラムを起動するたびに変わります。 | New interface, Share view: the channel uses a quick tunnel. trycloudflare.com is a domain name, keep it. |  |
| `permalink.named_running` | The permanent link is running. | 固定リンクが動作しています。 | New interface, permanent link: message after switching back to it. |  |
| `permalink.other_name` | Choose another name | 別の名前を選ぶ | New interface, permanent link wizard: button after a name conflict. |  |
| `permalink.overwrite` | Replace the record | レコードを置き換える | New interface, permanent link wizard: button after a name conflict; replaces the DNS record. |  |
| `permalink.retry` | Try again | 再試行 | New interface, permanent link wizard, step 1: button after a failed login. |  |
| `permalink.setup` | Set up a permanent link | 固定リンクを設定 | New interface, permanent link: button that opens the wizard. |  |
| `permalink.starting` | Starting the login… | ログインを開始しています… | New interface, permanent link wizard: shown until the engine reports the login progress. |  |
| `permalink.step_host` | Step 2 of 2. Enter the full address under which the channel should work. The domain must be on your Cloudflare account. | ステップ2/2：チャンネルを公開するアドレスを完全な形で入力してください。ドメインはCloudflareアカウントに追加済みのものを使います。 | New interface, permanent link wizard, step 2. |  |
| `permalink.step_login` | Step 1 of 2. I will open the Cloudflare page where you log in and choose the domain for the channel. | ステップ1/2：Cloudflareのページでログインし、チャンネルに使うドメインを選びます。 | New interface, permanent link wizard, step 1. | en is first person ("I will open"); Japanese describes the step: Cloudflareのページでログインし、…選びます. |
| `permalink.step_login_wait` | Step 1 of 2. Log in to Cloudflare and choose the domain in the browser window that opened. You have 5 minutes. | ステップ1/2：開いたブラウザーのウィンドウでCloudflareにログインし、ドメインを選んでください。制限時間は5分です。 | New interface, permanent link wizard, step 1, while waiting for the login in the browser. |  |
| `permalink.switching` | Switching to the quick tunnel… | クイックトンネルに切り替えています… | New interface, permanent link: progress message. |  |
| `permalink.waiting` | Waiting for the login… | ログインを待っています… | New interface, permanent link wizard: shown while the login is not finished. |  |
| `permalink.window_only` | Setting up and deleting the permanent link is done in the program on the computer (it needs the Cloudflare login). | 固定リンクの設定と削除は、PCのプログラムで行います（Cloudflareへのログインが必要なため）。 | Note in a remote panel under the permanent link buttons. |  |
| `permalink.wizard_label` | Set up a permanent link | 固定リンクの設定 | New interface, permanent link wizard: accessible name of the wizard's panel. |  |

### player

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `player.about_time` | about {time} | 約{time} | New interface, player bar: screen-reader text of a position shown with "≈" (when copying the stream a seek lands on the previous keyframe). |  |
| `player.added_by` | added by {name} | 追加：{name} | New interface, player bar: second line under the title; {name} is the viewer's nick. Impersonal form (the program does not know the viewer's gender). | Impersonal 追加：{name} (the program does not know the viewer). Alt.: {name}さんが追加 (but さん would also follow ホスト). |
| `player.back_10` | −10 s | −10秒 | New interface, player bar: text of the small button that seeks 10 seconds back. Keep it as short as the English one. | Small button; −10秒 is as short as the English. |
| `player.back_10_label` | Back 10 seconds | 10秒戻る | New interface, player bar: tooltip and screen-reader name of the −10 s button. |  |
| `player.copied` | Copied | コピーしました | New interface: shown for a moment in place of "Copy link" after copying. Max ~16 characters. |  |
| `player.copy_link` | Copy link | リンクをコピー | New interface, player bar: button text. Max ~16 characters. |  |
| `player.copy_link_label` | Copy the channel link for the VRChat video player | VRChatの動画プレーヤー用にチャンネルのリンクをコピー | New interface, player bar: tooltip of the "Copy link" button. |  |
| `player.copy_video_link` | Copy video link | 動画のリンクをコピー | New interface, player menu (right click on the player bar): copies the source address of the video on air. |  |
| `player.forward_10` | +10 s | +10秒 | New interface, player bar: text of the small button that seeks 10 seconds forward. Keep it short. |  |
| `player.forward_10_label` | Forward 10 seconds | 10秒進む | New interface, player bar: tooltip and screen-reader name of the +10 s button. |  |
| `player.label` | Playback | 再生 | New interface, player bar at the bottom: accessible name of the bar (screen readers). |  |
| `player.live` | LIVE | ライブ | New interface, player bar: shown instead of the times for a live stream (it cannot be paused or seeked). Short, upper case. | Tag shown instead of the times. en is upper case LIVE; Japanese has no case. Alt.: LIVE (often left in English in Japanese UIs). |
| `player.menu` | Playback menu | 再生メニュー | New interface: screen-reader name of the menu opened by a right click on the player bar. |  |
| `player.mute` | Mute the preview | プレビューをミュート | New interface, player bar: tooltip of the speaker button while the preview has sound. |  |
| `player.next` | Next video | 次の動画 | New interface, player bar (⏭ button tooltip) and the player menu: skips the current video. |  |
| `player.nothing_playing` | Nothing is playing | 再生中の動画はありません | New interface, player bar: title when no video is on air. |  |
| `player.nothing_playing_hint` | The channel shows the waiting slate until a video is added. | 動画が追加されるまで、チャンネルには待機画面が表示されます。 | New interface, player bar: second line when no video is on air. One short line (it is cut with … when too long). |  |
| `player.pause` | Pause | 一時停止 | New interface, player bar: tooltip of the big amber button while the channel plays (pauses the channel for all viewers). |  |
| `player.paused_manual` | Paused | 一時停止中 | New interface, player bar: second line while the owner paused the channel. |  |
| `player.paused_no_viewers` | Paused — nobody is watching | 一時停止中（視聴者なし） | New interface, player bar: second line while the channel waits for a viewer (it resumes by itself). | Second line in the player bar. en uses a dash; parentheses instead. |
| `player.paused_tunnel` | Paused — no internet connection | 一時停止中（インターネット接続なし） | New interface, player bar: second line while the channel waits for the internet connection (it resumes by itself). |  |
| `player.position_of` | {position} of {duration} | {position}（全体{duration}） | New interface, player bar: screen-reader value of the timeline, e.g. "1:32 of 3:33". | Screen-reader value, e.g. 1:32（全体3:33）. Alt.: 3:33中1:32. |
| `player.restart` | From the beginning | 最初から | New interface, player bar (⏮ button tooltip) and the player menu: plays the current video again from 0:00 for everybody. |  |
| `player.resume` | Resume | 再開 | New interface, player bar: tooltip of the big amber button while the owner paused the channel. |  |
| `player.shortcuts` | Keyboard shortcuts | キーボードショートカット | New interface, player menu: opens the list of keyboard shortcuts (also the ? key). |  |
| `player.timeline` | Position in the video | 動画の再生位置 | New interface, player bar: screen-reader name of the timeline (click or drag to seek). |  |
| `player.unmute` | Unmute the preview | プレビューのミュートを解除 | New interface, player bar: tooltip of the speaker button while the preview is muted. |  |
| `player.untitled` | Untitled | タイトルなし | New interface: shown for a video whose source gave no title. |  |
| `player.volume` | Preview volume | プレビューの音量 | New interface, player bar: screen-reader name of the volume slider. It changes only the preview in this window, not the channel. |  |

### preview

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `preview.caption` | Preview · about {seconds} s behind the server | プレビュー · サーバーから約{seconds}秒遅れ | New interface: small caption under the preview. {seconds} is the measured delay of the preview, rounded. | Keeps the interface's separator " · " (the code joins other parts with the same sign). |
| `preview.caption_plain` | Preview | プレビュー | New interface: caption under the preview while the delay is not measured yet. |  |
| `preview.copy_channel_link` | Copy channel link | チャンネルのリンクをコピー | New interface, preview context menu: copies the channel address for the VRChat video player. |  |
| `preview.label` | Live preview of the channel | チャンネルのライブプレビュー | New interface: screen-reader name of the video preview on the Channel view. |  |
| `preview.menu` | Preview menu | プレビューのメニュー | New interface: screen-reader name of the menu opened by a right click on the preview. |  |
| `preview.retry` | Try again | 再試行 | New interface: button under the "Preview unavailable" message; tries to play the stream again. Max ~20 characters. |  |
| `preview.seek_bubble` | Viewers will see this in about {seconds} s | 視聴者には約{seconds}秒後に表示されます | New interface: bubble over the preview after the owner seeks. {seconds} is the measured delay of the preview, rounded. |  |
| `preview.seek_bubble_unknown` | Viewers will see this in a few seconds | 視聴者には数秒後に表示されます | New interface: bubble over the preview after a seek when the delay is not measured yet. |  |
| `preview.unavailable` | Preview unavailable | プレビューを表示できません | New interface: message over the preview when the stream could not be played after several tries. |  |

### queue

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `queue.add_button` | Add | 追加 | New interface, Channel view: button next to the link field (the same as pressing Enter). |  |
| `queue.add_label` | Add a link | リンクを追加 | New interface, Channel view: screen-reader label of the field where the owner pastes a link. |  |
| `queue.add_placeholder` | Paste a link to a video or playlist… | 動画または再生リストのリンクを貼り付け… | New interface, Channel view: placeholder of the link field. Keep the three dots. |  |
| `queue.added_count` | one: Added {count} video.<br>other: Added {count} videos. | other: {count}本の動画を追加しました。 | New interface: short message after several videos were added at once (a playlist or dropped files). |  |
| `queue.added_one` | Added: {title} | 追加しました：{title} | New interface: short message after one link was added; {title} is the video title. |  |
| `queue.adding` | Adding… | 追加中… | New interface, Channel view: shown on the button while the program reads the link. Keep the three dots. |  |
| `queue.anonymous` | anonymous | 匿名 | New interface, queue row: detail of a video a viewer added without a nick. Lower case, short. |  |
| `queue.approve` | Approve | 承認 | New interface: tooltip of the check button that lets a viewer's video into the queue. |  |
| `queue.approve_label` | Approve “{title}” | 「{title}」を承認 | New interface: screen-reader label of the approve button; {title} is the video title. |  |
| `queue.by_host` | added here | パネルから追加 | New interface, queue row: detail of a video the owner added in the program (not a viewer). Lower case, short. | Detail of a queue row (the owner's own video): パネルから追加 (review fix 1; was ここで追加). |
| `queue.copy_link` | Copy link | リンクをコピー | New interface, menu of a queued video: copies the address of the video. |  |
| `queue.count` | one: {count} video<br>other: {count} videos | other: {count}本の動画 | New interface, Channel view: number of videos in the queue, next to the heading (followed by their total length, e.g. "4 videos · 1:52:40"). | Followed by the total length: 4本の動画 · 1:49:54. |
| `queue.empty` | The queue is empty. Paste a link to add a video. | キューは空です。リンクを貼り付けて動画を追加してください。 | New interface, Channel view: shown in the queue panel when nothing is waiting. |  |
| `queue.from_disk` | from disk | ディスクから | New interface, queue row: detail of a video that is a file on this computer. Lower case, short. | Alt.: PCのファイル. |
| `queue.menu` | Video menu | 動画のメニュー | New interface: accessible name of the context menu of a video in the queue. |  |
| `queue.more` | More for “{title}” | 「{title}」のその他の操作 | Screen-reader label of the ⋮ button on a queue row (phone layout); {title} is the video title. |  |
| `queue.more_short` | More | その他 | Tooltip of the ⋮ button on a queue row (phone layout). |  |
| `queue.move_down` | Move down | 下へ移動 | Queue context menu item: moves the video one place down. |  |
| `queue.move_up` | Move up | 上へ移動 | Queue context menu item: moves the video one place up. |  |
| `queue.moved` | “{title}” moved to position {position} of {total} | 「{title}」を{total}本中{position}番目に移動しました | New interface: announced to screen readers after a video was moved in the queue with the keyboard. |  |
| `queue.paste_not_link` | The clipboard does not hold a link (http or https). | クリップボードにリンク（httpまたはhttps）がありません。 | New interface: error after Ctrl+V when the pasted text is not a web link. |  |
| `queue.play_now` | Play now | 今すぐ再生 | New interface, menu of a queued video: puts it on air at once. |  |
| `queue.reject` | Reject | 却下 | New interface: tooltip of the cross button that refuses a viewer's video. |  |
| `queue.reject_label` | Reject “{title}” | 「{title}」を却下 | New interface: screen-reader label of the reject button; {title} is the video title. |  |
| `queue.remove` | Remove from the queue | キューから削除 | New interface, menu of a queued video: removes it. Destructive action. |  |
| `queue.reorder_hint` | Drag to change the order. With the keyboard: Alt and the up or down arrow. | ドラッグして順番を変更できます。キーボードではAltキーと上下の矢印キーを使います。 | New interface: screen-reader hint of a queue row. Alt is the keyboard key. |  |
| `queue.requested_anonymously` | requested anonymously | 匿名のリクエスト | New interface, queue row waiting for approval: a viewer without a nick sent it. |  |
| `queue.requested_by` | requested by {name} | リクエスト：{name} | New interface, queue row waiting for approval: who sent it; {name} is the viewer's nick. Impersonal form in Polish. | Impersonal: リクエスト：{name}. |
| `queue.section_pending` | Waiting for approval | 承認待ち | New interface, Channel view: small heading over the videos that viewers sent and the owner has to approve (moderation switched on). |  |
| `queue.section_queued` | In the queue | 再生待ち | New interface, Channel view: small heading over the videos waiting to be played. | 再生待ち pairs with the other heading 承認待ち. |
| `queue.title` | Up next | 次に再生 | New interface, Channel view: heading of the queue panel on the right. | 次に再生 as YouTube's queue panel. Alt.: 次の動画. |
| `queue.to_top` | Move to the top | 先頭へ移動 | New interface, menu of a queued video: moves it to the first place in the queue. |  |
| `queue.unknown_length` | Total length of the known ones | 長さがわかっている動画の合計 | New interface: tooltip of the total length when some videos are live streams or have no length ("+" is shown after the time). |  |

### settings

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `settings.about.available` | A new version is available: {version}. | 新しいバージョンが利用可能です：{version}。 | New interface, Settings view, About: banner; {version} is a number. |  |
| `settings.about.check` | Check for updates | アップデートを確認 | New interface, Settings view, About: button. |  |
| `settings.about.checking` | Checking… | 確認しています… | New interface, Settings view, About: note. |  |
| `settings.about.install` | Update | アップデート | New interface, Settings view, About: button that installs the new version. |  |
| `settings.about.install_failed` | The update did not work: {error} | アップデートできませんでした：{error} | New interface, Settings view, About: error; {error} is the reason. |  |
| `settings.about.installed` | Version {version} is installed. The program restarts… | バージョン{version}をインストールしました。プログラムを再起動します… | New interface, Settings view, About: confirmation. |  |
| `settings.about.installing` | Installing the new version… | 新しいバージョンをインストールしています… | New interface, Settings view, About: note. |  |
| `settings.about.latest` | You have the latest version ({version}). If there is no internet or the repository is private, the program shows nothing: the details are in the log. | 最新バージョン（{version}）を使用しています。インターネットに接続されていない場合やリポジトリが非公開の場合は、何も表示されません。詳細はログで確認できます。 | New interface, Settings view, About: note after a check with no update; {version} is a number. |  |
| `settings.about.skip` | Skip this version | このバージョンをスキップ | New interface, Settings view, About: button. |  |
| `settings.about.skipped` | Version {version} is skipped. | バージョン{version}はスキップされます。 | New interface, Settings view, About: confirmation. |  |
| `settings.about.version` | VRCinema, version {version} | VRCinema（バージョン{version}） | New interface, Settings view, About: {version} is a number like 0.2.0. | Shown as VRCinema（バージョン0.3.0）. |
| `settings.advanced.cookies` | Cookies file for yt-dlp | yt-dlp用のCookieファイル | New interface, Settings view, Advanced: label. Keep yt-dlp. |  |
| `settings.advanced.cookies_clear` | Do not use the file | ファイルを使用しない | New interface, Settings view, Advanced: button. |  |
| `settings.advanced.cookies_cleared` | The cookies file is no longer used. | Cookieファイルは使用されなくなりました。 | New interface, Settings view, Advanced: confirmation. |  |
| `settings.advanced.cookies_hint` | For videos that need a login. | ログインが必要な動画用です。 | New interface, Settings view, Advanced: hint. |  |
| `settings.advanced.cookies_none_picked` | No file chosen. | ファイルが選択されませんでした。 | New interface, Settings view, Advanced: note when the picker was closed without a choice. |  |
| `settings.advanced.cookies_pick` | Choose a file… | ファイルを選択… | New interface, Settings view, Advanced: button that opens a file picker. |  |
| `settings.advanced.cookies_picking` | Choose the file in the window that opened… | 開いたウィンドウでファイルを選択してください… | New interface, Settings view, Advanced: note while the picker is open. |  |
| `settings.advanced.cookies_saved` | The cookies file is saved. | Cookieファイルを保存しました。 | New interface, Settings view, Advanced: confirmation. |  |
| `settings.advanced.cookies_unset` | No file chosen. | ファイルが選択されていません。 | New interface, Settings view, Advanced: shown where the path of the file would be. |  |
| `settings.advanced.pixeldrain` | pixeldrain API key (optional) | pixeldrainのAPIキー（省略可） | New interface, Settings view, Advanced: label. pixeldrain is a site name. |  |
| `settings.advanced.pixeldrain_hint` | Helps with files that have a download limit. | ダウンロード制限があるファイルに役立ちます。 | New interface, Settings view, Advanced: hint. |  |
| `settings.advanced.pixeldrain_placeholder_saved` | •••• saved — type a new key to replace it | •••• 保存済み。置き換えるには新しいキーを入力 | Placeholder of the empty pixeldrain key field when a key is saved (the key itself is never shown). | Placeholder; the dash of en became 。. |
| `settings.advanced.pixeldrain_remove` | Remove key | キーを削除 | Button removing the saved pixeldrain key. |  |
| `settings.advanced.pixeldrain_removed` | The pixeldrain key is removed. | pixeldrainのキーを削除しました。 | New interface, Settings view, Advanced: confirmation. |  |
| `settings.advanced.pixeldrain_replace` | Replace | 置き換え | Button saving a new pixeldrain key in place of the saved one. |  |
| `settings.advanced.pixeldrain_saved` | The pixeldrain key is saved. | pixeldrainのキーを保存しました。 | New interface, Settings view, Advanced: confirmation. |  |
| `settings.cancel` | Cancel | キャンセル | New interface, Settings view: button that closes a dialog or a form without saving. |  |
| `settings.channel.allowed` | Allowed sites | 許可したサイト | New interface, Settings view, Channel: title of a list of sites shown as chips. | 許可したサイト / ブロックしたサイト (parallel forms). Alt.: 許可サイト / ブロックサイト. |
| `settings.channel.allowed_empty_warning` | The list of allowed sites is empty, so every link from viewers will be refused. | 許可したサイトのリストが空のため、視聴者からのリンクはすべて拒否されます。 | New interface, Settings view, Channel: warning shown in the "only allowed sites" mode when the allowed list is empty. |  |
| `settings.channel.allowed_hint` | Used in the "only allowed sites" mode. A site includes its subdomains. | 「許可したサイトのみ」モードで使用します。サイトにはそのサブドメインも含まれます。 | New interface, Settings view, Channel: hint under the list. | Quotes the radio label 「許可したサイトのみ」 exactly. |
| `settings.channel.blocked` | Blocked sites | ブロックしたサイト | New interface, Settings view, Channel: title of a list of sites shown as chips. |  |
| `settings.channel.blocked_empty_warning` | The list of blocked sites is empty, so nothing is blocked: links from any site are accepted. | ブロックしたサイトのリストが空のため、何もブロックされません。すべてのサイトのリンクが受け付けられます。 | New interface, Settings view, Channel: warning shown in the "all except blocked sites" mode when the blocked list is empty. |  |
| `settings.channel.blocked_hint` | Used in the "all except blocked" mode. A site includes its subdomains. | 「ブロックしたサイト以外すべて」モードで使用します。サイトにはそのサブドメインも含まれます。 | New interface, Settings view, Channel: hint under the list. | Quotes the radio label 「ブロックしたサイト以外すべて」 exactly. |
| `settings.channel.discontinuity` | Mark transitions between videos in the stream | ストリーム内の動画の切り替わりをマークする | New interface, Settings view, Channel: switch. The tag EXT-X-DISCONTINUITY is a technical name. |  |
| `settings.channel.discontinuity_hint` | Adds the EXT-X-DISCONTINUITY tag; most players need it. | EXT-X-DISCONTINUITYタグを追加します。ほとんどのプレーヤーで必要です。 | New interface, Settings view, Channel: hint. Keep EXT-X-DISCONTINUITY. |  |
| `settings.channel.discontinuity_off` | Transitions are not marked. | 切り替わりはマークされません。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.discontinuity_on` | Transitions are marked. | 切り替わりがマークされます。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.domain_add` | Add | 追加 | New interface, Settings view, Channel: button that adds the site typed to a list. |  |
| `settings.channel.domain_add_label` | Add a site to the list: {list} | リストにサイトを追加：{list} | New interface, Settings view, Channel: accessible name of the field; {list} is the title of the list. |  |
| `settings.channel.domain_invalid` | "{text}" is not a site address (for example youtube.com). | 「{text}」はサイトのアドレスではありません（例：youtube.com）。 | New interface, Settings view, Channel: error when the site typed is not valid. |  |
| `settings.channel.domain_remove` | Remove {domain} | {domain}を削除 | New interface, Settings view, Channel: accessible name and tooltip of the × on a site chip; {domain} is e.g. youtube.com. |  |
| `settings.channel.domains_empty` | The list is empty. | リストは空です。 | New interface, Settings view, Channel: shown in an empty list of sites. |  |
| `settings.channel.domains_saved` | List of sites saved. | サイトのリストを保存しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.force_transcode` | Always convert the picture | 常に映像を変換する | New interface, Settings view, Channel: switch. |  |
| `settings.channel.force_transcode_hint` | Slower, but the stream is more uniform. | 処理は遅くなりますが、ストリームが均一になります。 | New interface, Settings view, Channel: hint. |  |
| `settings.channel.force_transcode_off` | Videos are converted only when needed. | 動画は必要な場合にのみ変換されます。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.force_transcode_on` | Every video will be converted. | すべての動画が変換されます。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.home_network` | Whatever the mode, links to the home network or to this computer are always refused. | どのモードでも、ホームネットワークやこのPCへのリンクは常に拒否されます。 | New interface, Settings view, Channel: note under the lists. |  |
| `settings.channel.limits` | Limits | 制限 | New interface, Settings view, Channel: heading of the limits for viewers. |  |
| `settings.channel.limits_invalid` | Every limit must be a whole number, at least 1. | 制限はすべて1以上の整数にしてください。 | New interface, Settings view, Channel: error. |  |
| `settings.channel.limits_saved` | Limits saved. | 制限を保存しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.links` | Links from viewers | 視聴者からのリンク | New interface, Settings view, Channel: heading. Which sites viewers may send links from; the owner's own links are never limited. |  |
| `settings.channel.links_hint` | Which links viewers may send. Your own links are never limited. | 視聴者が送信できるリンクを設定します。パネルから追加するリンクは制限されません。 | New interface, Settings view, Channel: hint under the heading. | en "Your own links"; without a pronoun: パネルから追加するリンク (the links added in the panel). |
| `settings.channel.max_active` | Videos in the queue per person | 1人あたりのキュー内の動画数 | New interface, Settings view, Channel: label of a number field. |  |
| `settings.channel.max_duration` | Longest video (minutes) | 動画の最大の長さ（分） | New interface, Settings view, Channel: label of a number field. |  |
| `settings.channel.mode_all` | Any site | すべてのサイト | New interface, Settings view, Channel: choice of the link mode (radio). |  |
| `settings.channel.mode_all_hint` | Viewers can send a link from any site. | 視聴者はどのサイトのリンクでも送信できます。 | New interface, Settings view, Channel: explains the mode "any site". |  |
| `settings.channel.mode_allowlist` | Only allowed sites | 許可したサイトのみ | New interface, Settings view, Channel: choice of the link mode (radio). |  |
| `settings.channel.mode_allowlist_hint` | Only the sites on the allowed list. | 「許可したサイト」リストにあるサイトのみ。 | New interface, Settings view, Channel: explains the mode. |  |
| `settings.channel.mode_blocklist` | All except blocked sites | ブロックしたサイト以外すべて | New interface, Settings view, Channel: choice of the link mode (radio). |  |
| `settings.channel.mode_blocklist_hint` | Every site except those on the blocked list. | 「ブロックしたサイト」リストにあるサイト以外のすべてのサイト。 | New interface, Settings view, Channel: explains the mode. |  |
| `settings.channel.mode_saved` | Link mode saved. | リンクのモードを保存しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.moderation` | Approve viewers' requests before they join the queue | 視聴者のリクエストをキューに入れる前に承認する | New interface, Settings view, Channel: switch. |  |
| `settings.channel.moderation_block` | Approval | 承認 | New interface, Settings view, Channel: heading. |  |
| `settings.channel.moderation_hint` | Requests then wait in "Pending approval" on the Channel view. | リクエストは「チャンネル」ビューの「承認待ち」で待機します。 | New interface, Settings view, Channel: hint. "Pending approval" is the name of a section of the Channel view. | Quotes the real section heading 「承認待ち」 (queue.section_pending) and the view 「チャンネル」. |
| `settings.channel.moderation_off` | Requests join the queue at once. | リクエストはすぐにキューに入ります。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.moderation_on` | Requests will wait for your approval. | リクエストは承認されるまで待機します。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.popular` | Add popular sites | 人気のサイトを追加 | New interface, Settings view, Channel: button that adds a ready set of well-known video sites to the allowed list. |  |
| `settings.channel.request_count` | Requests from one address | 1つのアドレスからのリクエスト数 | New interface, Settings view, Channel: label of a number field (how many requests one viewer may send ...). |  |
| `settings.channel.request_window` | within (minutes) | 期間（分） | New interface, Settings view, Channel: label of a number field continuing the previous one ("... within N minutes"). | Continues request_count ("… within N minutes"). Alt.: 時間枠（分）. |
| `settings.channel.slate_language` | Language of the slates | 待機画面の言語 | New interface, Settings view, Channel: label. Slates are the screens the stream shows when nothing plays ("No videos in the queue"); the language also applies to the info ticker. | 待機画面 for every slate (idle, buffering, pause); never a bare 画面. |
| `settings.channel.slate_language_hint` | The language of the screens on the stream and of the info ticker. | ストリームに表示される待機画面とテロップの言語です。 | New interface, Settings view, Channel: hint. |  |
| `settings.channel.slate_language_saved` | Language of the slates saved. | 待機画面の言語を保存しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.channel.slate_language_saved_restart` | Language of the slates saved. The video on air starts again from where it is. | 待機画面の言語を保存しました。放送中の動画は現在の位置から再開されます。 | New interface, Settings view, Channel: confirmation when the info ticker is on (its font may change, so the film restarts). |  |
| `settings.channel.stream` | Stream | ストリーム | New interface, Settings view, Channel: heading of the settings that concern the stream. |  |
| `settings.general.autostart` | Start VRCinema with Windows | Windowsの起動時にVRCinemaを自動的に開始する | New interface, Settings view, General: switch. |  |
| `settings.general.autostart_hint` | The program starts minimized in the tray when you sign in. | サインイン時に、プログラムが通知領域に最小化された状態で起動します。 | New interface, Settings view, General: hint under the autostart switch. | サインイン = Windows sign-in (Windows wording). |
| `settings.general.autostart_off` | The program will no longer start with Windows. | Windowsの起動時にプログラムが自動で開始されなくなります。 | New interface, Settings view, General: confirmation after switching autostart off. |  |
| `settings.general.autostart_on` | The program will start with Windows. | Windowsの起動時にプログラムが自動で開始されます。 | New interface, Settings view, General: confirmation after switching autostart on. |  |
| `settings.general.lan` | Allow opening the page from other devices on the home network | ホームネットワーク上の他のデバイスからページを開けるようにする | New interface, Settings view, General: switch. |  |
| `settings.general.lan_hint` | Changing this restarts the server for a moment. | 変更するとサーバーが一時的に再起動します。 | New interface, Settings view, General: hint under the home network switch. |  |
| `settings.general.lan_off` | The page is available only on this computer. | ページはこのPCでのみ利用できます。 | New interface, Settings view, General: confirmation. |  |
| `settings.general.lan_on` | Other devices on the home network can now open the page. | ホームネットワーク上の他のデバイスからページを開けるようになりました。 | New interface, Settings view, General: confirmation. |  |
| `settings.general.language` | Language | 言語 | New interface, Settings view, General: label of the choice of the program's language (the interface, tray menu and log). |  |
| `settings.general.language_env` | This server has no window: set the language with LANGUAGE in the .env file. | このサーバーにはウィンドウがありません。言語は.envファイルのLANGUAGEで設定してください。 | New interface, Settings view, General: shown instead of the language choice on the headless server. LANGUAGE and .env are names, keep them. |  |
| `settings.general.language_hint` | The language of the program: the interface, the tray menu and the log. | プログラムの言語（画面、通知領域のメニュー、ログ）です。 | New interface, Settings view, General: hint under the language choice. |  |
| `settings.general.panel_language` | Language of this panel | このパネルの言語 | Settings → General, outside the program's window: the language of the panel in this browser. |  |
| `settings.general.panel_language_hint` | Kept in this browser only; the program's own language does not change. | このブラウザーにのみ保存されます。プログラム自体の言語は変わりません。 | Hint of settings.general.panel_language. |  |
| `settings.general.port` | Server port | サーバーのポート | New interface, Settings view, General: label of the number field (1024-65535). |  |
| `settings.general.port_hint` | A new port or network access restarts the server: the video on air is interrupted for a moment and the channel link gets the new port. | ポートまたはネットワークアクセスを変更するとサーバーが再起動します。放送中の動画は一時的に中断され、チャンネルのリンクのポートも新しくなります。 | New interface, Settings view, General: warning under the port field. |  |
| `settings.general.port_restarted` | The server restarted on port {port}. The window moves to the new address and the channel link has this port now. | サーバーがポート{port}で再起動しました。ウィンドウは新しいアドレスに移動し、チャンネルのリンクもこのポートになりました。 | New interface, Settings view, General: confirmation after the restart; {port} is a number. |  |
| `settings.general.port_same` | This is already the current port. | 現在のポートと同じです。 | New interface, Settings view, General: note when the port typed is the one in use. |  |
| `settings.general.port_save` | Save port | ポートを保存 | New interface, Settings view, General: button next to the port field. |  |
| `settings.general.port_saved` | Port {port} saved. | ポート{port}を保存しました。 | New interface, Settings view, General: confirmation; {port} is a number. |  |
| `settings.general.sign_out` | Sign out | ログアウト | Button (Settings → General and the phone top bar) that ends the password session of the remote panel in this browser. |  |
| `settings.general.sign_out_hint` | Ends the session of this panel in this browser. | このブラウザーでのこのパネルのセッションを終了します。 | Hint of settings.general.sign_out. |  |
| `settings.internet.address` | Channel address: {url} | チャンネルのアドレス：{url} | New interface, Settings view, Internet: {url} is the public address. |  |
| `settings.internet.connection_ended` | Done — the connection of this panel ended with it. Open the panel again at the new address or from the home network. | 完了しました。このパネルの接続も終了しました。新しいアドレスまたはホームネットワークからパネルを開き直してください。 | Note after switching the tunnel off, reconnecting or changing the link from a panel opened through the tunnel, when the connection dropped as expected. |  |
| `settings.internet.link_type` | Link type | リンクの種類 | New interface, Settings view, Internet: heading of the quick or permanent link block. |  |
| `settings.internet.lose_access` | You opened this panel through the tunnel. Switching it off ends this connection: you will lose access to this panel. When the tunnel is switched on again, a quick link gets a new address. | このパネルはトンネル経由で開かれています。トンネルをオフにするとこの接続が切れ、このパネルにアクセスできなくなります。トンネルを再びオンにすると、クイックリンクには新しいアドレスが割り当てられます。 | Body of the confirmation dialog before switching the tunnel off from a remote panel opened through it (spec: 'You will lose access to this panel'). A quick link is the temporary trycloudflare.com address. |  |
| `settings.internet.lose_access_cancel` | Keep it on | オンのままにする | Cancel button of the dialog: the tunnel stays on. |  |
| `settings.internet.lose_access_mode` | You opened this panel through the tunnel. Switching between the quick and the permanent link changes the channel's address: you will lose access to this panel at this address. | このパネルはトンネル経由で開かれています。クイックリンクと固定リンクを切り替えるとチャンネルのアドレスが変わり、このアドレスではパネルにアクセスできなくなります。 | Body of the dialog before changing the link type from a session that came through the tunnel. |  |
| `settings.internet.lose_access_mode_title` | Change the link? | リンクを変更しますか？ | Title of the confirmation dialog before switching between the quick and the permanent link from a session that came through the tunnel. |  |
| `settings.internet.lose_access_mode_yes` | Change and lose access | それでも変更する | Confirming button of the link type dialog (danger style). | Danger button "Change and lose access": それでも変更する (the dialog text says access is lost). |
| `settings.internet.lose_access_reconnect` | You opened this panel through the tunnel. Reconnecting ends this connection, and a quick link gets a new address: you may lose access to this panel at this address. | このパネルはトンネル経由で開かれています。再接続するとこの接続が切れ、クイックリンクには新しいアドレスが割り当てられるため、このアドレスではパネルにアクセスできなくなる可能性があります。 | Body of the dialog before reconnecting the tunnel from a session that came through it. |  |
| `settings.internet.lose_access_reconnect_title` | Reconnect the tunnel? | トンネルを再接続しますか？ | Title of the confirmation dialog before reconnecting the tunnel from a remote panel opened through it. |  |
| `settings.internet.lose_access_reconnect_yes` | Reconnect anyway | それでも再接続する | Confirming button of the reconnect dialog (danger style). |  |
| `settings.internet.lose_access_title` | Switch the tunnel off? | トンネルをオフにしますか？ | Title of the confirmation dialog when the tunnel is switched off from a remote panel opened through that tunnel. |  |
| `settings.internet.lose_access_yes` | Switch off and lose access | それでもオフにする | Confirming button of the dialog (danger style). | Danger button "Switch off and lose access": それでもオフにする. |
| `settings.internet.manual` | Public address of the channel | チャンネルの公開アドレス | New interface, Settings view, Internet: heading and label of the address typed by hand (shown only with the tunnel off). |  |
| `settings.internet.manual_hint` | With the tunnel off, you can type the address under which the channel is reachable (for example from another tunnel). | トンネルがオフのときは、チャンネルにアクセスできるアドレス（例：別のトンネルのアドレス）を入力できます。 | New interface, Settings view, Internet: hint. |  |
| `settings.internet.manual_saved` | Public address saved. | 公開アドレスを保存しました。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.pause_loss` | Pause when the internet is gone | インターネットが切断されたら一時停止する | New interface, Settings view, Internet: switch. |  |
| `settings.internet.pause_loss_off` | The channel will not pause when the internet is gone. | インターネットが切断されても、チャンネルは一時停止しません。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.pause_loss_on` | The channel will pause when the internet is gone. | インターネットが切断されると、チャンネルは一時停止します。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.pause_viewers` | Pause when nobody is watching | 誰も視聴していないときは一時停止する | New interface, Settings view, Internet: switch. |  |
| `settings.internet.pause_viewers_off` | The channel will not pause when nobody is watching. | 誰も視聴していなくても、チャンネルは一時停止しません。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.pause_viewers_on` | The channel will pause when nobody is watching. | 誰も視聴していないと、チャンネルは一時停止します。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.pauses` | Pausing the channel | チャンネルの一時停止 | New interface, Settings view, Internet: heading. |  |
| `settings.internet.pauses_hint` | A paused video resumes from the same place. Live streams are not paused. | 一時停止した動画は同じ位置から再開されます。ライブ配信は一時停止されません。 | New interface, Settings view, Internet: hint. |  |
| `settings.internet.state_connecting` | Connecting to the internet… | インターネットに接続しています… | New interface, Settings view, Internet: state of the tunnel. |  |
| `settings.internet.state_error` | No internet. | インターネットに接続できません。 | New interface, Settings view, Internet: state of the tunnel. | en "No internet." as a sentence. |
| `settings.internet.state_off` | The tunnel is off. | トンネルはオフです。 | New interface, Settings view, Internet: state of the tunnel. |  |
| `settings.internet.state_online` | Online. | オンラインです。 | New interface, Settings view, Internet: state of the tunnel. |  |
| `settings.internet.state_reconnecting` | The connection dropped — reconnecting… | 接続が切れました。再接続しています… | New interface, Settings view, Internet: state of the tunnel. |  |
| `settings.internet.trust` | Trust Cloudflare headers | Cloudflareのヘッダーを信頼する | New interface, Settings view, Internet: switch. Cloudflare is a company name. |  |
| `settings.internet.trust_hint` | Needed when the channel runs behind Cloudflare and should see the real addresses of viewers. | Cloudflare経由でチャンネルを公開していて、視聴者の実際のIPアドレスを取得したい場合に必要です。 | New interface, Settings view, Internet: hint under the switch. |  |
| `settings.internet.trust_off` | Cloudflare headers are ignored. | Cloudflareのヘッダーは無視されます。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.trust_on` | Viewer addresses will be taken from Cloudflare headers. | 視聴者のアドレスはCloudflareのヘッダーから取得されます。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.tunnel` | Share the channel on the internet (built-in Cloudflare tunnel) | インターネットでチャンネルを公開する（内蔵のCloudflareトンネル） | New interface, Settings view, Internet: switch. Cloudflare is a company name. |  |
| `settings.internet.tunnel_hint` | The program connects to the internet by itself and gives the ready channel address for VRChat. When off, the channel works only on this computer (or at the address typed by hand). | プログラムが自動でインターネットに接続し、VRChat用のチャンネルのアドレスを用意します。オフの場合、チャンネルはこのPCでのみ（または手動で入力したアドレスで）動作します。 | New interface, Settings view, Internet: hint under the tunnel switch. |  |
| `settings.internet.tunnel_off` | The tunnel is off. The channel works only locally or at the address typed by hand. | トンネルはオフです。チャンネルはローカルまたは手動で入力したアドレスでのみ動作します。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.tunnel_on` | The tunnel is on — connecting to the internet. | トンネルはオンです。インターネットに接続しています。 | New interface, Settings view, Internet: confirmation. |  |
| `settings.internet.via_tunnel` | This panel is open through the tunnel. | このパネルはトンネル経由で開かれています。 | Note under the tunnel switch in a remote session that came through the tunnel. |  |
| `settings.language_name.de` | Deutsch | Deutsch | New interface, Settings view and the language selector of the request page: the language's name in that language (an endonym), the same in every dictionary. Do not translate it. |  |
| `settings.language_name.en` | English | English | New interface, Settings view and the language selector of the request page: the language's name in that language (an endonym), the same in every dictionary. Do not translate it. |  |
| `settings.language_name.es` | Español | Español | New interface, Settings view and the language selector of the request page: the language's name in that language (an endonym), the same in every dictionary. Do not translate it. |  |
| `settings.language_name.fr` | Français | Français | New interface, Settings view and the language selector of the request page: the language's name in that language (an endonym), the same in every dictionary. Do not translate it. |  |
| `settings.language_name.ja` | 日本語 (beta) | 日本語 (beta) | New interface, Settings view and the language selector of the request page: the language's name in that language (an endonym), the same in every dictionary. Do not translate it; "(beta)" marks that Japanese is not fully reviewed yet. |  |
| `settings.language_name.pl` | Polski | Polski | New interface, Settings view and the language selector of the request page: the language's name in that language (an endonym), the same in every dictionary. Do not translate it. |  |
| `settings.loading` | Loading… | 読み込み中… | New interface, Settings view: shown while the settings are being read from the program. |  |
| `settings.log.copy` | Copy | コピー | New interface, Settings view, Log: button that copies the log text. |  |
| `settings.log.empty` | The log is empty. | ログは空です。 | New interface, Settings view, Log: shown when there are no entries. |  |
| `settings.log.label` | Latest entries of the log | ログの最新の項目 | New interface, Settings view, Log: accessible name of the log text. |  |
| `settings.log.refresh` | Refresh | 更新 | New interface, Settings view, Log: button. |  |
| `settings.nav.about` | About | バージョン情報 | New interface, Settings view: name of a section (version, updates). | バージョン情報 as Windows' About. |
| `settings.nav.advanced` | Advanced | 詳細設定 | New interface, Settings view: name of a section. |  |
| `settings.nav.channel` | Channel | チャンネル | New interface, Settings view: name of a section (links from viewers, limits, info ticker, nicks). |  |
| `settings.nav.general` | General | 全般 | New interface, Settings view: name of a section (list on the left and its heading). | 全般 as Windows property tabs. |
| `settings.nav.internet` | Internet | インターネット | New interface, Settings view: name of a section (the tunnel that gives the channel a public address). |  |
| `settings.nav.label` | Settings sections | 設定のセクション | New interface, Settings view: accessible name of the list of sections. |  |
| `settings.nav.log` | Log | ログ | New interface, Settings view: name of a section (the program's log). |  |
| `settings.nav.remote` | Remote panel | リモートパネル | New interface, Settings view: name of a section (logging in to the panel from other devices with a password). |  |
| `settings.nav.tools` | Tools | ツール | New interface, Settings view: name of a section (yt-dlp, ffmpeg, Deno, cloudflared and the graphics card). |  |
| `settings.nicks.block` | Block device | デバイスをブロック | New interface, Settings view, Channel: button that refuses requests from the device that holds the nick; also the confirm button of its dialog. |  |
| `settings.nicks.block_body` | Requests from the device that holds the nick {nick} will be refused. You can lift the block here at any time. | ニックネーム「{nick}」を使っているデバイスからのリクエストは拒否されます。ブロックはここでいつでも解除できます。 | New interface, Settings view, Channel: text of the confirmation dialog. |  |
| `settings.nicks.block_label` | Block the device of {nick} | 「{nick}」が使っているデバイスをブロック | New interface, Settings view, Channel: accessible name of the button. |  |
| `settings.nicks.block_title` | Block the device? | デバイスをブロックしますか？ | New interface, Settings view, Channel: title of the confirmation dialog. |  |
| `settings.nicks.blocked_at` | blocked {when} | {when}にブロック | New interface, Settings view, Channel: {when} is a time like 21:04 or Oct 12, 21:04. |  |
| `settings.nicks.blocked_devices` | one: {count} blocked device<br>other: {count} blocked devices | other: ブロック中のデバイス（{count}台） | New interface, Settings view, Channel: heading of the list of blocked devices; plural by {count}. | Heading with counter 台 for devices. |
| `settings.nicks.blocked_done` | The device of {nick} is blocked. | 「{nick}」が使っているデバイスをブロックしました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.nicks.blocked_tag` | blocked | ブロック中 | New interface, Settings view, Channel: small tag on a nick whose device is blocked. |  |
| `settings.nicks.blocked_title` | Blocked devices | ブロック中のデバイス | New interface, Settings view, Channel: accessible name of the list. |  |
| `settings.nicks.empty` | No viewer has taken a nick yet. | ニックネームを使った視聴者はまだいません。 | New interface, Settings view, Channel: empty list. |  |
| `settings.nicks.hint` | The nicks viewers have used recently, with their icon. Release a nick to let someone else take it; block a device to refuse its requests. | 最近視聴者が使ったニックネームとそのアイコンです。ニックネームを解放すると他の人が使えるようになり、デバイスをブロックするとそのデバイスからのリクエストは拒否されます。 | New interface, Settings view, Channel: hint. |  |
| `settings.nicks.last_used` | last used {when} | 最終使用：{when} | New interface, Settings view, Channel: {when} is a time like 21:04 or Oct 12, 21:04. |  |
| `settings.nicks.no_nick` | (no nick) | （ニックネームなし） | New interface, Settings view, Channel: shown for a blocked device that had no nick. |  |
| `settings.nicks.release` | Release | 解放 | New interface, Settings view, Channel: button that frees a nick for others. | 解放 = free a nick. Alt.: 解除. |
| `settings.nicks.release_label` | Release the nick {nick} | ニックネーム「{nick}」を解放 | New interface, Settings view, Channel: accessible name of the button. |  |
| `settings.nicks.released` | The nick {nick} is free. | ニックネーム「{nick}」を解放しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.nicks.title` | Viewer nicks | 視聴者のニックネーム | New interface, Settings view, Channel: heading of the list of nicks viewers have taken. |  |
| `settings.nicks.unblock` | Unblock | ブロック解除 | New interface, Settings view, Channel: button that lifts a block. |  |
| `settings.nicks.unblock_label` | Unblock the device of {nick} | 「{nick}」が使っているデバイスのブロックを解除 | New interface, Settings view, Channel: accessible name of the button. |  |
| `settings.nicks.unblocked` | The block of {nick} is lifted. | 「{nick}」のブロックを解除しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.remote.change` | Change password | パスワードを変更 | New interface, Settings view, Remote panel: button. |  |
| `settings.remote.change_password` | Change the password | パスワードの変更 | New interface, Settings view, Remote panel: label of the password row when one is set. |  |
| `settings.remote.changed` | The password was changed. Remote sessions signed in earlier have ended. | パスワードを変更しました。以前にログインしたリモートセッションは終了しました。 | New interface, Settings view, Remote panel: confirmation. |  |
| `settings.remote.enable` | Allow logging in to the panel with a password from other devices (for example through the public address) | 他のデバイスからパスワードでパネルにログインできるようにする（例：公開アドレス経由） | New interface, Settings view, Remote panel: switch. |  |
| `settings.remote.enable_hint` | The window of this program needs no password. The password only protects remote access. | このプログラムのウィンドウではパスワードは不要です。パスワードはリモートアクセスのみを保護します。 | New interface, Settings view, Remote panel: hint. |  |
| `settings.remote.has_password` | The password is set. | パスワードは設定済みです。 | New interface, Settings view, Remote panel: state. |  |
| `settings.remote.no_password` | The password is not set. | パスワードは設定されていません。 | New interface, Settings view, Remote panel: state. |  |
| `settings.remote.off` | The remote panel is off. | リモートパネルはオフです。 | New interface, Settings view, Remote panel: confirmation. |  |
| `settings.remote.on` | The remote panel is on. | リモートパネルはオンです。 | New interface, Settings view, Remote panel: confirmation. |  |
| `settings.remote.password` | New password | 新しいパスワード | New interface, Settings view, Remote panel: accessible name of the password field. |  |
| `settings.remote.password_first` | First set a password (at least {min} characters). The remote panel turns on with it. | 先にパスワード（{min}文字以上）を設定してください。設定するとリモートパネルがオンになります。 | New interface, Settings view, Remote panel: note when the switch is turned on without a password; {min} is a number (8). |  |
| `settings.remote.password_hint` | At least {min} characters. | {min}文字以上。 | New interface, Settings view, Remote panel: hint; {min} is a number (8). |  |
| `settings.remote.set` | Set password | パスワードを設定 | New interface, Settings view, Remote panel: button. |  |
| `settings.remote.set_and_on` | The password is set and the remote panel is on. | パスワードを設定し、リモートパネルをオンにしました。 | New interface, Settings view, Remote panel: confirmation. |  |
| `settings.remote.set_password` | Set a password | パスワードの設定 | New interface, Settings view, Remote panel: label of the password row. |  |
| `settings.restarting` | The server is restarting. The change will be checked in a moment. | サーバーを再起動しています。まもなく変更を確認します。 | New interface, Settings view: note after a change of the port or network access made the server restart. |  |
| `settings.save` | Save | 保存 | New interface, Settings view: button that saves a field or a form. |  |
| `settings.ticker.enabled` | Show the info ticker | テロップを表示する | New interface, Settings view, Channel: switch. |  |
| `settings.ticker.enabled_hint` | The title of the video and who added it. | 動画のタイトルと、動画を追加した人を表示します。 | New interface, Settings view, Channel: hint. |  |
| `settings.ticker.enabled_off` | The info ticker is off. | テロップをオフにしました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.ticker.enabled_off_restart` | The info ticker is off. The video on air starts again from where it is. | テロップをオフにしました。放送中の動画は現在の位置から再開されます。 | New interface, Settings view, Channel: confirmation after switching the ticker off. |  |
| `settings.ticker.enabled_on` | The info ticker is on. The video on air starts again from where it is. | テロップをオンにしました。放送中の動画は現在の位置から再開されます。 | New interface, Settings view, Channel: confirmation. | Confirmation after the switch: テロップをオンにしました (also _off / _off_restart). |
| `settings.ticker.encoder_amd` | Conversion: AMD graphics card (h264_amf). | 変換：AMDのグラフィックボード（h264_amf）。 | New interface, Settings view, Channel: which converter the channel uses. h264_amf is a name. |  |
| `settings.ticker.encoder_cpu` | Conversion: processor (libx264). | 変換：CPU（libx264）。 | New interface, Settings view, Channel: which converter the channel uses. libx264 is a name. |  |
| `settings.ticker.encoder_cpu_note` | With the info ticker the computer may not keep up. A graphics card works better (Settings → Tools). | テロップを使うと、PCの処理が追いつかない場合があります。グラフィックボードのほうが適しています（設定 → ツール）。 | New interface, Settings view, Channel: follows the converter's name when it is the processor. |  |
| `settings.ticker.encoder_gpu_note` | The info ticker will not load the computer much. | テロップによるPCの負荷はあまり大きくありません。 | New interface, Settings view, Channel: follows the converter's name when it is a graphics card. |  |
| `settings.ticker.encoder_intel` | Conversion: Intel graphics (h264_qsv, Quick Sync). | 変換：Intelのグラフィックス（h264_qsv、Quick Sync）。 | New interface, Settings view, Channel: which converter the channel uses. h264_qsv is a name. |  |
| `settings.ticker.encoder_nvidia` | Conversion: NVIDIA graphics card (h264_nvenc). | 変換：NVIDIAのグラフィックボード（h264_nvenc）。 | New interface, Settings view, Channel: which converter the channel uses. h264_nvenc is a name. |  |
| `settings.ticker.hint` | Burned into the picture of the stream — everyone in VRChat sees it. Needs every video to be converted. | ストリームの映像に焼き込まれ、VRChatの全員に表示されます。すべての動画の変換が必要です。 | New interface, Settings view, Channel: hint under the heading. |  |
| `settings.ticker.interval` | Interval | 間隔 | New interface, Settings view, Channel: label of the choice (1, 2, 5 or 10 minutes). |  |
| `settings.ticker.interval_hint` | How often the band appears (in the "from time to time" mode) | テロップが表示される頻度（「一定間隔」モードの場合） | New interface, Settings view, Channel: hint. | Quotes the option 「一定間隔」. |
| `settings.ticker.message_add` | Add message | メッセージを追加 | New interface, Settings view, Channel: button. |  |
| `settings.ticker.message_edit` | Edit | 編集 | New interface, Settings view, Channel: button on a message; also the accessible name of its edit field. |  |
| `settings.ticker.message_new` | New message | 新しいメッセージ | New interface, Settings view, Channel: placeholder and accessible name of the field. |  |
| `settings.ticker.message_remove` | Remove | 削除 | New interface, Settings view, Channel: button on a message. |  |
| `settings.ticker.message_remove_label` | Remove the message: {text} | メッセージを削除：{text} | New interface, Settings view, Channel: accessible name of the remove button; {text} is the message. |  |
| `settings.ticker.messages` | Your messages | メッセージ | New interface, Settings view, Channel: heading of the list of the owner's messages. | en "Your messages"; no pronoun. |
| `settings.ticker.messages_hint` | Shown in the band after the information about the video, one at a time — the next one each time the band appears. Up to {max}. Not translated. | テロップの中で、動画の情報の後に1件ずつ表示されます（テロップが表示されるたびに次のメッセージ）。最大{max}件。翻訳はされません。 | New interface, Settings view, Channel: hint; {max} is a number (10). |  |
| `settings.ticker.messages_saved` | Messages saved. | メッセージを保存しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.ticker.mode` | Mode | モード | New interface, Settings view, Channel: label of the choice. |  |
| `settings.ticker.mode_continuous` | Continuous | 常時 | New interface, Settings view, Channel: option. | Alt.: 常に表示, ループ. |
| `settings.ticker.mode_hint` | From time to time, or all the time in a loop | 一定間隔で表示するか、常にループで表示するか | New interface, Settings view, Channel: hint. |  |
| `settings.ticker.mode_interval` | From time to time | 一定間隔 | New interface, Settings view, Channel: option. | en "From time to time"; 一定間隔 (at intervals) because the next control is 間隔. Alt.: ときどき. |
| `settings.ticker.progress` | Video progress line | 動画の進行状況バー | New interface, Settings view, Channel: switch. | 進行状況バー as Windows names a progress bar. Alt.: 再生位置バー, プログレスバー. |
| `settings.ticker.progress_hint` | A thin line at the bottom of the picture | 映像の下端に表示される細い線 | New interface, Settings view, Channel: hint. |  |
| `settings.ticker.qr` | QR code of the request page | リクエストページのQRコード | New interface, Settings view, Channel: label of the choice. |  |
| `settings.ticker.qr_always` | Always | 常に表示 | New interface, Settings view, Channel: option. |  |
| `settings.ticker.qr_hint` | In the corner of the picture, above the band | 映像の隅、テロップの上に表示 | New interface, Settings view, Channel: hint. |  |
| `settings.ticker.qr_off` | Off | オフ | New interface, Settings view, Channel: option. |  |
| `settings.ticker.qr_with_ticker` | With the band | テロップと一緒に表示 | New interface, Settings view, Channel: option. |  |
| `settings.ticker.restart_note` | Changing the switch, mode, interval, speed, QR code or progress line starts the video on air again from where it is (a live stream: from the next item). The texts and "next video" change at once. | 「テロップを表示する」「モード」「間隔」「スクロール速度」「リクエストページのQRコード」「動画の進行状況バー」を変更すると、放送中の動画が現在の位置から再開されます（ライブ配信の場合は次の動画から）。テキストと「次の動画を表示する」の変更はすぐに反映されます。 | New interface, Settings view, Channel: note near the ticker controls. | Quotes the seven control labels exactly: テロップを表示する, モード, 間隔, スクロール速度, リクエストページのQRコード, 動画の進行状況バー, 次の動画を表示する. |
| `settings.ticker.sample_title` | Title of the video | 動画のタイトル | New interface, Settings view, Channel: stand-in title used to estimate how long the band is on the screen when nothing is playing. |  |
| `settings.ticker.saved` | Info ticker settings saved. | テロップの設定を保存しました。 | New interface, Settings view, Channel: confirmation. |  |
| `settings.ticker.saved_restart` | Info ticker settings saved. The video on air starts again from where it is. | テロップの設定を保存しました。放送中の動画は現在の位置から再開されます。 | New interface, Settings view, Channel: confirmation of a change of the ticker that restarts the video on air. |  |
| `settings.ticker.show_address` | Show the address of the request page | リクエストページのアドレスを表示する | New interface, Settings view, Channel: switch. |  |
| `settings.ticker.show_address_hint` | As text, in the band | テロップ内にテキストで表示 | New interface, Settings view, Channel: hint. |  |
| `settings.ticker.show_next` | Show the next video | 次の動画を表示する | New interface, Settings view, Channel: switch. |  |
| `settings.ticker.slow` | The computer cannot keep up with the info ticker — turn it off or turn on conversion on the graphics card. | PCの処理がテロップに追いついていません。テロップをオフにするか、グラフィックボードでの変換をオンにしてください。 | New interface, Settings view, Channel: warning banner shown when the video converts slower than real time. |  |
| `settings.ticker.speed` | Scrolling speed | スクロール速度 | New interface, Settings view, Channel: label of the choice. |  |
| `settings.ticker.speed_fast` | Fast | 速い | New interface, Settings view, Channel: option. |  |
| `settings.ticker.speed_hint` | It decides how long the text stays on the screen (now about {seconds} s) | テキストが画面に表示される時間が変わります（現在は約{seconds}秒） | New interface, Settings view, Channel: hint; {seconds} is a number computed for the current text. |  |
| `settings.ticker.speed_normal` | Normal | 標準 | New interface, Settings view, Channel: option. |  |
| `settings.ticker.speed_slow` | Slow | 遅い | New interface, Settings view, Channel: option. |  |
| `settings.ticker.title` | Info ticker | テロップ | New interface, Settings view, Channel: heading of the block that sets the band burned into the stream (title, who added it, next video, owner's messages). | テロップ (TV term for burned-in text). Alt.: ティッカー, 情報バー. |
| `settings.ticker.too_long` | A message can have at most {max} characters. | メッセージは{max}文字以内にしてください。 | New interface, Settings view, Channel: error; {max} is a number (200). |  |
| `settings.ticker.too_many` | There can be at most {max} messages. | メッセージは最大{max}件です。 | New interface, Settings view, Channel: error; {max} is a number (10). |  |
| `settings.tools.channel_down` | The channel did not start although the tools are downloaded — they may be damaged. | ツールはダウンロード済みですが、チャンネルが起動しませんでした。ツールが破損している可能性があります。 | New interface, Settings view, Tools: banner. |  |
| `settings.tools.channel_down_reason` | Reason: {reason} | 理由：{reason} | New interface, Settings view, Tools: continues the previous text; {reason} is the engine's message. |  |
| `settings.tools.detected_now` | Result of the test: {result}. | テストの結果：{result}。 | New interface, Settings view, Tools: {result} is one of the "settings.tools.result_*" texts. |  |
| `settings.tools.detecting` | Checking the graphics card. It takes a few seconds… | グラフィックボードを確認しています。数秒かかります… | New interface, Settings view, Tools: note while the test runs. |  |
| `settings.tools.done` | Done | 完了 | New interface, Settings view, Tools: state of a program in the progress list. |  |
| `settings.tools.download` | Download | ダウンロード | New interface, Settings view, Tools: button on a program that is not installed. |  |
| `settings.tools.downloading` | Downloading {done} | ダウンロード中：{done} | New interface, Settings view, Tools: progress; {done} is a size like 12.3 MB. |  |
| `settings.tools.downloading_of` | Downloading {done} of {total} | ダウンロード中：{done}/{total} | New interface, Settings view, Tools: progress; {done} and {total} are sizes like 12.3 MB. |  |
| `settings.tools.enc_amd` | AMD card (AMF) | AMDのグラフィックボード（AMF） | New interface, Settings view, Tools: option. |  |
| `settings.tools.enc_auto` | Automatic (recommended) | 自動（推奨） | New interface, Settings view, Tools: option. |  |
| `settings.tools.enc_cpu` | Processor (libx264) | CPU（libx264） | New interface, Settings view, Tools: option. |  |
| `settings.tools.enc_intel` | Intel graphics (Quick Sync) | Intelのグラフィックス（Quick Sync） | New interface, Settings view, Tools: option. | Intelのグラフィックス (Intel's own product name; integrated graphics). AMD/NVIDIA: グラフィックボード. |
| `settings.tools.enc_nvidia` | NVIDIA card (NVENC) | NVIDIAのグラフィックボード（NVENC） | New interface, Settings view, Tools: option. |  |
| `settings.tools.encoder` | Picture conversion | 映像の変換 | New interface, Settings view, Tools: label of the choice of the converter. |  |
| `settings.tools.encoder_auto` | The converter is picked automatically. | 変換方式は自動で選択されます。 | New interface, Settings view, Tools: confirmation. |  |
| `settings.tools.encoder_hint` | A graphics card converts without loading the processor. | グラフィックボードを使うと、CPUに負荷をかけずに変換できます。 | New interface, Settings view, Tools: hint. |  |
| `settings.tools.encoder_saved` | Converter saved. | 変換方式を保存しました。 | New interface, Settings view, Tools: confirmation. |  |
| `settings.tools.failed` | It did not work: {error} | 失敗しました：{error} | New interface, Settings view, Tools: error of a download, update or repair; {error} is the reason. |  |
| `settings.tools.gpu` | Graphics card | グラフィックボード | New interface, Settings view, Tools: heading of the block that picks the converter. | グラフィックボード (common Japanese term). Alt.: GPU, グラフィックカード. |
| `settings.tools.hint` | The channel needs these programs. The program downloads and checks them. | チャンネルにはこれらのプログラムが必要です。ダウンロードと確認はプログラムが自動で行います。 | New interface, Settings view, Tools: hint. |  |
| `settings.tools.installed` | Installed | インストール済み | New interface, Settings view, Tools: state of a program in the progress list. |  |
| `settings.tools.job_install` | The missing tools are downloaded. | 不足しているツールをダウンロードしました。 | New interface, Settings view, Tools: confirmation. |  |
| `settings.tools.job_repair` | The tools are repaired. | ツールを修復しました。 | New interface, Settings view, Tools: confirmation. |  |
| `settings.tools.job_update` | The tool is updated. | ツールを更新しました。 | New interface, Settings view, Tools: confirmation. |  |
| `settings.tools.last_test` | Result of the last test: {result}. | 前回のテストの結果：{result}。 | New interface, Settings view, Tools: {result} is one of the "settings.tools.result_*" texts. |  |
| `settings.tools.missing_running` | Tools are missing: {tools}. The channel is on air, but some functions do not work. | ツールが不足しています：{tools}。チャンネルは放送中ですが、一部の機能は動作しません。 | New interface, Settings view, Tools: banner; {tools} is a list of program names. |  |
| `settings.tools.missing_stopped` | Tools are missing, so the channel is not on air. | ツールが不足しているため、チャンネルは放送されていません。 | New interface, Settings view, Tools: banner. |  |
| `settings.tools.name_cloudflared` | cloudflared (public address of the channel) | cloudflared（チャンネルの公開アドレス） | New interface, Settings view, Tools: name of a program with what it does. Keep cloudflared. |  |
| `settings.tools.name_deno` | Deno (required by yt-dlp) | Deno（yt-dlpに必要） | New interface, Settings view, Tools: name of a program with what it does. Keep Deno and yt-dlp. |  |
| `settings.tools.name_ffmpeg` | ffmpeg (converting the picture) | ffmpeg（映像の変換） | New interface, Settings view, Tools: name of a program with what it does. Keep ffmpeg. |  |
| `settings.tools.name_other` | {name} | {name} | New interface, Settings view, Tools: name of a program not known to the interface; {name} is the name the program reports. |  |
| `settings.tools.name_ytdlp` | yt-dlp (downloading from sites) | yt-dlp（サイトからのダウンロード） | New interface, Settings view, Tools: name of a program with what it does. Keep yt-dlp. |  |
| `settings.tools.no_test` | The card has not been tested yet. | まだテストされていません。 | New interface, Settings view, Tools: state. | Shortened (the block heading already says グラフィックボード); the long form was cut with … at 960 px. |
| `settings.tools.not_installed` | not installed | 未インストール | New interface, Settings view, Tools: state of a program. |  |
| `settings.tools.phase_install` | Installing | インストール中 | New interface, Settings view, Tools: step of an installation. |  |
| `settings.tools.phase_verify` | Checking the checksum | チェックサムを確認中 | New interface, Settings view, Tools: step of an installation. |  |
| `settings.tools.phase_version` | Checking the version | バージョンを確認中 | New interface, Settings view, Tools: step of an installation. |  |
| `settings.tools.preparing` | Preparing | 準備中 | New interface, Settings view, Tools: state of a program in the progress list. |  |
| `settings.tools.repair` | Repair tools | ツールを修復 | New interface, Settings view, Tools: button that downloads and checks all programs again. |  |
| `settings.tools.restart` | Restart the channel | チャンネルを再起動 | New interface, Settings view, Tools: button. |  |
| `settings.tools.restarted` | The channel was restarted. | チャンネルを再起動しました。 | New interface, Settings view, Tools: confirmation. |  |
| `settings.tools.result_amd` | conversion on an AMD card | AMDのグラフィックボードで変換 | New interface, Settings view, Tools: result of the test, inserted into a sentence. |  |
| `settings.tools.result_cpu` | conversion on the processor | CPUで変換 | New interface, Settings view, Tools: result of the test, inserted into a sentence. | Inserted in 「テストの結果：{result}。」. |
| `settings.tools.result_intel` | conversion on Intel graphics (Quick Sync) | Intelのグラフィックス（Quick Sync）で変換 | New interface, Settings view, Tools: result of the test, inserted into a sentence. |  |
| `settings.tools.result_nvidia` | conversion on an NVIDIA card | NVIDIAのグラフィックボードで変換 | New interface, Settings view, Tools: result of the test, inserted into a sentence. |  |
| `settings.tools.test` | Test the card | グラフィックボードをテスト | New interface, Settings view, Tools: button that checks which graphics card can convert. |  |
| `settings.tools.title` | Programs | プログラム | New interface, Settings view, Tools: heading of the list of programs the channel needs. |  |
| `settings.tools.update` | Update | 更新 | New interface, Settings view, Tools: button on an installed program. | 更新 for a tool; アップデート only for VRCinema's own new version (About). |
| `settings.tools.version` | version {version} | バージョン{version} | New interface, Settings view, Tools: installed version of a program. |  |
| `settings.tools.version_unknown` | unknown | 不明 | New interface, Settings view, Tools: shown after "version" when the version could not be read. | Inserted after バージョン: バージョン不明. |
| `settings.tools.waiting` | Waiting | 待機中 | New interface, Settings view, Tools: state of a program in the progress list. |  |

### share

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `share.channel_hint` | Paste it into the video player in VRChat (e.g. ProTV). | VRChatの動画プレーヤー（ProTVなど）に貼り付けてください。 | New interface, Share view: under the title of the channel link. VRChat and ProTV are names, keep them. |  |
| `share.channel_title` | Channel link | チャンネルのリンク | New interface, Share view: card with the link for the VRChat video player. |  |
| `share.copied` | Copied | コピーしました | New interface, Share view: replaces "Copy" for a moment after copying. |  |
| `share.copy` | Copy | コピー | New interface, Share view: button that copies a link. |  |
| `share.internet_title` | Internet | インターネット | New interface, Share view: card about the internet connection. |  |
| `share.open` | Open | 開く | New interface, Share view: link that opens the request page in the browser. |  |
| `share.qr_alt` | QR code of the request page {url} | リクエストページのQRコード：{url} | New interface, Share view: description of the QR picture for screen readers; {url} is the page address. |  |
| `share.qr_hint` | To scan with a phone | スマートフォンでスキャン | New interface, Share view: under the title of the QR card. | Alt.: スマホでスキャン (more casual). |
| `share.qr_none` | There is no public address yet. The code appears when the channel is online. | まだ公開アドレスがありません。チャンネルがオンラインになるとコードが表示されます。 | New interface, Share view: shown in place of the QR code. |  |
| `share.qr_note` | On the channel's slate this code is shown when nothing is playing. | 何も再生されていないとき、チャンネルの待機画面にこのコードが表示されます。 | New interface, Share view: note under the QR code. |  |
| `share.qr_title` | QR code | QRコード | New interface, Share view: title of the card with the QR code. |  |
| `share.reason` | Reason: {reason} | 理由：{reason} | New interface, Share view: why the tunnel has no connection; {reason} is a sentence. |  |
| `share.reconnect` | Reconnect | 再接続 | New interface, Share view: button that restarts the tunnel. |  |
| `share.reconnected` | The tunnel was restarted. A quick link gets a new address. | トンネルを再起動しました。クイックリンクには新しいアドレスが割り当てられます。 | New interface, Share view: message after the tunnel was restarted. |  |
| `share.reconnecting` | Reconnecting… | 再接続中… | New interface, Share view: label of the button while the tunnel restarts. |  |
| `share.request_hint` | Viewers add videos here. The same address is in the QR code on the channel's slate. | 視聴者はここから動画を追加します。チャンネルの待機画面のQRコードにも同じアドレスが入っています。 | New interface, Share view: under the title of the request page card. |  |
| `share.request_title` | Request page | リクエストページ | New interface, Share view: card with the address of the page where viewers add videos. |  |
| `share.sub` | The link for the VRChat player and the page where viewers add videos. | VRChatのプレーヤー用のリンクと、視聴者が動画を追加するページです。 | New interface, Share view: explanation under the title. VRChat is a name, keep it. |  |
| `share.title` | Share | 共有 | New interface: title of the Share view (the same word as the left rail). |  |
| `share.tunnel_off` | The tunnel is off, so the channel works only locally or at the address set in Settings → Internet. | トンネルがオフのため、チャンネルはローカル、または設定したアドレス（設定 → インターネット）でのみ動作します。 | New interface, Share view: shown when the built-in tunnel is switched off. "Settings → Internet" names the Settings view and its section; keep the names as in the left rail. |  |

### shortcuts

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `shortcuts.help` | This list | この一覧 | New interface, keyboard shortcuts dialog: what the ? key does. |  |
| `shortcuts.intro` | They work when the cursor is not in a text field. | テキスト欄にカーソルがないときに使えます。 | New interface, keyboard shortcuts dialog: first line. |  |
| `shortcuts.key_ctrl` | Ctrl | Ctrl | New interface, keyboard shortcuts dialog: name of the Ctrl key as printed on keyboards in this language. |  |
| `shortcuts.key_home` | Home | Home | New interface, keyboard shortcuts dialog: name of the Home key as printed on keyboards in this language. |  |
| `shortcuts.key_shift` | Shift | Shift | New interface, keyboard shortcuts dialog: name of the Shift key as printed on keyboards in this language. |  |
| `shortcuts.key_space` | Space | スペース | New interface, keyboard shortcuts dialog: name of the space bar as printed on keyboards in this language. | JIS keyboards print nothing on the space bar; スペース as in Japanese help texts. Ctrl, Shift, Home stay as printed. |
| `shortcuts.next` | Next video | 次の動画 | New interface, keyboard shortcuts dialog: what the N key does. |  |
| `shortcuts.paste` | Add a link from the clipboard | クリップボードからリンクを追加 | New interface, keyboard shortcuts dialog: what Ctrl+V does outside text fields. |  |
| `shortcuts.play_pause` | Pause / resume | 一時停止 / 再開 | New interface, keyboard shortcuts dialog: what the Space key does. |  |
| `shortcuts.restart` | From the beginning | 最初から | New interface, keyboard shortcuts dialog: what the Home key does. |  |
| `shortcuts.seek_10` | 10 seconds back / forward | 10秒戻る / 進む | New interface, keyboard shortcuts dialog: what the ← and → keys do. |  |
| `shortcuts.seek_60` | 60 seconds back / forward | 60秒戻る / 進む | New interface, keyboard shortcuts dialog: what Shift+← and Shift+→ do. |  |
| `shortcuts.stats` | Stats for nerds | 詳細統計情報 | New interface, keyboard shortcuts dialog: what the I key does (the overlay with technical details, as on YouTube). |  |
| `shortcuts.title` | Keyboard shortcuts | キーボードショートカット | New interface: title of the dialog listing the keyboard shortcuts. |  |

### slates

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `slates.buffering_subtitle` | The video is about to start | まもなく動画が始まります | Stream slate (drawn into the video, 1920 px wide, large font): subtitle under "Buffering…". Max ~40 characters. |  |
| `slates.buffering_title` | Buffering… | 読み込み中… | Stream slate title while a video loads. Max ~30 characters. | 読み込み中… (YouTube's wording). |
| `slates.connecting` | Connecting to the internet… | インターネットに接続しています… | Stream slate (idle): subtitle while the tunnel connects. Max ~40 characters. |  |
| `slates.idle_hint` | Add a video on the request page | リクエストページから動画を追加できます | Stream slate (idle): subtitle when there is no public address. Max ~40 characters. | Stream slate (players read it): soft polite, できます (invitation) instead of an order. |
| `slates.idle_title` | No videos in the queue | キューに動画がありません | Stream slate (idle) title. Max ~30 characters. |  |
| `slates.pause_title` | Paused — back in a moment | 一時停止中です。まもなく再開します | Stream slate shown while the channel is paused. Max ~30 characters. | Two short sentences (review fix 1): 一時停止中です。まもなく再開します; 17 of 30 characters, 1204 px of 1920 at 72 px. |

### stats

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `stats.copy` | Copy | コピー | New interface, stats overlay: button that copies all the numbers as text. Max ~10 characters. |  |
| `stats.group_ffmpeg` | ffmpeg | ffmpeg | Group heading in the stats overlay: the encoder process. ffmpeg is a program name, keep it. |  |
| `stats.group_hls` | HLS channel | HLSチャンネル | Group heading in the stats overlay: the published HLS playlist. HLS is a streaming format name, keep it. |  |
| `stats.group_next` | Next video | 次の動画 | Group heading in the stats overlay: the video being prepared for after the current one. |  |
| `stats.group_on_air` | On air | 放送中 | Group heading in the stats overlay: the video being broadcast. |  |
| `stats.group_preview` | Preview in this window | このウィンドウのプレビュー | Group heading in the stats overlay: measurements of the preview in this window. |  |
| `stats.group_tunnel` | Internet (tunnel) | インターネット（トンネル） | Group heading in the stats overlay: the Cloudflare tunnel that makes the channel public. |  |
| `stats.group_viewers` | Viewers | 視聴者 | Group heading in the stats overlay: people watching the channel. |  |
| `stats.reason_manual` | paused by you | 手動で一時停止 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, pause reason: the owner paused it. |  |
| `stats.reason_no_viewers` | nobody is watching | 視聴者なし | New interface, preview overlay "Stats for nerds" (like on YouTube): value, pause reason: automatic pause because nobody is watching. |  |
| `stats.reason_tunnel` | no internet connection | インターネット接続なし | New interface, preview overlay "Stats for nerds" (like on YouTube): value, pause reason: the tunnel is down. |  |
| `stats.row_address` | Address | アドレス | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, the public address of the tunnel of the channel. |  |
| `stats.row_buffer` | Server buffer | サーバーのバッファー | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, segments of the video already cut and waiting. |  |
| `stats.row_connections` | Connections | 接続 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, number of connections to Cloudflare and their locations. |  |
| `stats.row_delay` | Delay | 遅延 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, how far behind the server this preview is, the buffered time and dropped video frames. |  |
| `stats.row_encoder` | Encoder | エンコーダー | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, the video encoder in use (for example h264_nvenc). |  |
| `stats.row_fetches` | Segment fetches | セグメントの取得 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, how many video pieces viewers download per minute and how much data per second; followed by a small bar chart of the last minute. |  |
| `stats.row_format` | Source | ソース | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, technical parameters of the video file (resolution, frame rate, codecs). |  |
| `stats.row_mode` | Mode | モード | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, whether the video is copied or converted. |  |
| `stats.row_next_state` | State | 状態 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, how far the preparation of the next video is. |  |
| `stats.row_pause` | Pause | 一時停止 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, whether the channel is paused and why. |  |
| `stats.row_position` | Server position | サーバーの再生位置 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, where the server is cutting the video (viewers are a few seconds behind). | Where the server is cutting the video: サーバーの再生位置. |
| `stats.row_process` | Process | プロセス | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, the ffmpeg process (PID, speed, CPU, memory). |  |
| `stats.row_sequence` | Sequence | シーケンス | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, numbers of the HLS playlist. |  |
| `stats.row_timestamps` | Timestamps | タイムスタンプ | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, whether the timestamps of the stream (PTS/DTS) never go back. |  |
| `stats.row_tunnel_state` | State | 状態 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, state of the tunnel and how long it has been online. |  |
| `stats.row_video` | Video | 動画 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, the title of the video and the site it comes from. |  |
| `stats.row_watching` | Watching | 視聴中 | New interface, preview overlay "Stats for nerds" (like on YouTube): row label, number of different viewer addresses that fetched video in the last 30 seconds. | Number of viewer addresses in the last 30 s. Alt.: 視聴者数. |
| `stats.title` | Stats for nerds | 詳細統計情報 | New interface: title of the overlay with technical details on the preview (as on YouTube) and its screen-reader name; also the preview menu item (key I). | YouTube Japan names the overlay 詳細統計情報. |
| `stats.tunnel_connecting` | connecting | 接続中 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the tunnel is starting. |  |
| `stats.tunnel_error` | error | エラー | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the tunnel failed. |  |
| `stats.tunnel_online` | online | オンライン | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the tunnel works. |  |
| `stats.tunnel_reconnecting` | reconnecting | 再接続中 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the tunnel lost its connection and tries again. |  |
| `stats.v_about` | ≈ {n} | ≈ {n} | New interface, preview overlay "Stats for nerds" (like on YouTube): value, "about N" (an approximate count). |  |
| `stats.v_attempt` | attempt {n} | {n}回目の試行 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, which attempt at playing the video this is (after a failure the program tries again). |  |
| `stats.v_audio` | audio | 音声あり | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the video has a sound track. |  |
| `stats.v_behind` | {seconds} s behind the server | サーバーから{seconds}秒遅れ | New interface, preview overlay "Stats for nerds" (like on YouTube): value, how far behind the server the preview is. |  |
| `stats.v_buffered` | buffer {seconds} s | バッファー{seconds}秒 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, seconds of video the preview has loaded ahead. |  |
| `stats.v_connections` | {n} to Cloudflare | Cloudflareへ{n}本 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, number of connections of the tunnel to Cloudflare. | 本 as the counter for connections (回線). Alt.: Cloudflareへの接続{n}件. |
| `stats.v_continuous` | continuous ✓ | 逆戻りなし ✓ | New interface, preview overlay "Stats for nerds" (like on YouTube): value, timestamps never went back (good). | Timestamps never went back: 逆戻りなし ✓ (pairs with 逆戻りあり ✗). |
| `stats.v_cpu` | CPU {percent}% | CPU {percent}% | New interface, preview overlay "Stats for nerds" (like on YouTube): value, processor use of the process. |  |
| `stats.v_discontinuities` | discontinuities {n} | 切り替わり{n}回 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, number of discontinuity markers published (one for every change of video source). | Discontinuity markers = transitions between videos, as in settings.channel.discontinuity (切り替わり). |
| `stats.v_dropped` | dropped frames {n} | フレーム落ち{n} | New interface, preview overlay "Stats for nerds" (like on YouTube): value, video frames the preview skipped. | フレーム落ち (common term). Alt.: ドロップフレーム. |
| `stats.v_fps` | {fps} fps | {fps} fps | New interface, preview overlay "Stats for nerds" (like on YouTube): value, frames per second. Keep it short. |  |
| `stats.v_hours_minutes` | {h} h {m} min | {h}時間{m}分 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, a duration of hours and minutes. |  |
| `stats.v_kb_s` | {n} KB/s sent | 送信{n} KB/s | New interface, preview overlay "Stats for nerds" (like on YouTube): value, data sent to viewers per second, in kilobytes. |  |
| `stats.v_mb_s` | {n} MB/s sent | 送信{n} MB/s | New interface, preview overlay "Stats for nerds" (like on YouTube): value, data sent to viewers per second, in megabytes. |  |
| `stats.v_minutes` | {m} min | {m}分 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, a duration of minutes. |  |
| `stats.v_mode_copy` | stream copy (no conversion) | ストリームコピー（変換なし） | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the video is passed through as it is. | ストリームコピー = ffmpeg's stream copy. Alt.: そのままコピー. |
| `stats.v_mode_transcode` | conversion | 変換 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the video is converted to another format. |  |
| `stats.v_next_failed` | failed | 失敗 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, preparing the next video failed. |  |
| `stats.v_next_preparing` | preparing | 準備中 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the next video is being looked up and analysed. |  |
| `stats.v_next_ready` | ready | 準備完了 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the next video is being cut. |  |
| `stats.v_no` | no | なし | New interface, preview overlay "Stats for nerds" (like on YouTube): value, plain "no". | Value of the Pause row when not paused: なし ("none") reads better than いいえ. |
| `stats.v_no_audio` | no audio | 音声なし | New interface, preview overlay "Stats for nerds" (like on YouTube): value, the video has no sound track. |  |
| `stats.v_not_continuous` | went back ✗ | 逆戻りあり ✗ | New interface, preview overlay "Stats for nerds" (like on YouTube): value, timestamps went back at least once (bad). | Alt.: 逆行あり ✗. |
| `stats.v_online_for` | online for {time} | 接続時間{time} | New interface, preview overlay "Stats for nerds" (like on YouTube): value, how long the tunnel has worked; {time} is a duration like "1 h 12 min". | Value after the state: オンライン · 接続時間1時間12分. |
| `stats.v_per_min` | {n}/min | {n}/分 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, count per minute. |  |
| `stats.v_pid` | PID {pid} | PID {pid} | New interface, preview overlay "Stats for nerds" (like on YouTube): value, process number. PID is a technical term, keep it. |  |
| `stats.v_pts` | PTS {seconds} s | PTS {seconds}秒 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, end of the last published packet on the channel timeline. PTS is a technical term, keep it. |  |
| `stats.v_ram` | RAM {mb} MB | RAM {mb} MB | New interface, preview overlay "Stats for nerds" (like on YouTube): value, memory use of the process. |  |
| `stats.v_ready` | one: {count} ready (about {seconds} s)<br>other: {count} ready (about {seconds} s) | other: {count}セグメント準備済み（約{seconds}秒） | New interface, preview overlay "Stats for nerds": value, number of cut segments waiting and the playing time they make; {count} segments, {seconds} the playing time. Plural forms by {count}. |  |
| `stats.v_reconnects` | reconnects {n} | 再接続{n}回 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, how many times the tunnel had to be started again. |  |
| `stats.v_seconds` | {s} s | {s}秒 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, a duration of seconds. |  |
| `stats.v_segment` | segment {seconds} s | セグメント{seconds}秒 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, length of one segment in seconds. |  |
| `stats.v_speed` | {speed}× real time | {speed}倍速 | New interface, preview overlay "Stats for nerds" (like on YouTube): value, how many times faster than playing speed ffmpeg works. | {speed}倍速 (e.g. 1.9倍速). |
| `stats.v_window` | window {n} | ウィンドウ{n} | New interface, preview overlay "Stats for nerds" (like on YouTube): value, number of segments in the HLS playlist. |  |

### ticker

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `ticker.added_by` | added by {nick} | 追加：{nick} | Info ticker burned into the stream picture (seen by every viewer in VRChat), in the slate language. Who requested the video; {nick} is the viewer's nick. Impersonal form (the program does not know the gender). | Burned into the stream: impersonal 追加：{nick}. |
| `ticker.added_by_host` | added by Host | 追加：ホスト | Info ticker burned into the stream picture (seen by every viewer in VRChat), in the slate language. The video was added by the channel owner ("Host" is the owner's badge, also used next to queue items). | ホスト must match the reserved nick in engine/src/nicks.rs. |
| `ticker.address` | add a video: {address} | リクエストはこちら：{address} | Info ticker burned into the stream picture (seen by every viewer in VRChat), in the slate language. {address} is the request page web address. | Invitation to the request page (review fix 1): リクエストはこちら：{address}. |
| `ticker.next` | next: {title} | 次：{title} | Info ticker burned into the stream picture (seen by every viewer in VRChat), in the slate language. The video after the current one; {title} is its title. |  |
| `ticker.now` | ► Now: {title} | ▶ 再生中：{title} | Info ticker burned into the stream picture (seen by every viewer in VRChat), in the slate language. Start of the ticker; {title} is the video on air. Keep the ► sign (the ticker font Arial has it; ▶ is not in Arial). Japanese uses ▶ instead: its ticker font Yu Gothic has no ►. | ▶ instead of the ► of the other languages: the Japanese ticker font (Yu Gothic) has no ► (checked on a real frame). The engine also writes ▶ for any language whose ticker is drawn with the Japanese font (a Japanese title). |
| `ticker.qr_caption` | Add a video | 動画を追加 | Info ticker burned into the stream picture (seen by every viewer in VRChat), in the slate language. Caption under the QR code in the corner of the stream picture (the code opens the request page). Very short. |  |

### titlebar

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `titlebar.close` | Close | 閉じる | New interface, title bar: tooltip of the close button. Closing only hides the program to the notification area. |  |
| `titlebar.maximize` | Maximize | 最大化 | New interface, title bar: tooltip of the maximize button (same words as Windows uses). |  |
| `titlebar.minimize` | Minimize | 最小化 | New interface, title bar of the window: tooltip of the minimize button (same words as Windows uses). |  |
| `titlebar.restore` | Restore Down | 元に戻す (縮小) | New interface, title bar: tooltip of the button that un-maximizes the window (same words as Windows uses). | As Windows shows it: 元に戻す (縮小), with half-width parentheses. |

### tray

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `tray.copy_channel_link` | Copy channel link | チャンネルのリンクをコピー | Tray icon of the desktop app. Menu entry; copies the address VRChat plays. Max about 30 characters. |  |
| `tray.copy_submit_link` | Copy request page link | リクエストページのリンクをコピー | Tray icon of the desktop app. Menu entry; copies the address of the page where viewers request videos. Max about 35 characters. |  |
| `tray.hidden_text` | VRCinema keeps running in the background — the channel is still on air. | VRCinemaはバックグラウンドで動作しています。チャンネルは放送中です。 | Tray icon of the desktop app. Text of the balloon shown once when the window is closed to the tray. |  |
| `tray.hidden_title` | VRCinema | VRCinema | Tray icon of the desktop app. Title of the balloon shown once when the window is closed to the tray. Keep the product name. |  |
| `tray.link_without_internet` | {link} (no internet) | {link}（インターネットなし） | Tray icon of the desktop app. Added to a copied link when the tunnel is on but has no address yet: the link works only on this computer. |  |
| `tray.net_connecting` | Connecting to the internet | インターネットに接続中 | Tray icon of the desktop app. Tooltip: the tunnel is starting. Used as {net} in tray.tooltip_head. |  |
| `tray.net_manual` | Manual address | 手動アドレス | Tray icon of the desktop app. Tooltip: the tunnel is off and the owner typed an address. Used as {net} in tray.tooltip_head. |  |
| `tray.net_offline` | No internet | インターネットなし | Tray icon of the desktop app. Tooltip: the tunnel is down. Used as {net} in tray.tooltip_head. |  |
| `tray.net_online` | Online | オンライン | Tray icon of the desktop app. Tooltip: the channel can be reached from the internet. Used as {net} in tray.tooltip_head. |  |
| `tray.open_panel` | Open panel | パネルを開く | Tray icon of the desktop app. Menu entry; opens the admin window. Max about 30 characters. |  |
| `tray.quit` | Quit | 終了 | Tray icon of the desktop app. Menu entry; quits the program and stops the channel. |  |
| `tray.skip` | Skip video | 動画をスキップ | Tray icon of the desktop app. Menu entry; ends the video that is on air. Max about 30 characters. |  |
| `tray.tooltip_head` | VRCinema — {net} | VRCinema：{net} | Tray icon of the desktop app. Start of the tooltip; {net} is one of tray.net_*. Keep the product name VRCinema. | Tray tooltip VRCinema：オンライン; idle/playing add ・休止中 / ・再生中：{title}. |
| `tray.tooltip_idle` | {head} — break | {head}・待機中 | Tray icon of the desktop app. Tooltip when no video is on air; {head} is the start of the tooltip. Windows keeps only about 127 characters of a tooltip. | en "break": 待機中 (review fix 1). |
| `tray.tooltip_playing` | {head} — now playing: {title} | {head}・再生中：{title} | Tray icon of the desktop app. Tooltip while a video is on air; {title} is the video title (cut when too long). |  |

### tunnel

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `tunnel.fetching` | Downloading cloudflared — the tunnel connects by itself once it is downloaded. | cloudflaredをダウンロードしています。ダウンロードが完了すると、トンネルは自動で接続されます。 | Tunnel status reason (app window, Settings → Internet). |  |
| `tunnel.named_fallback` | The permanent link files are missing — the quick tunnel runs (its address changes at every start). Set up the permanent link again. | 固定リンクのファイルがないため、クイックトンネルが動作しています（アドレスは起動するたびに変わります）。固定リンクをもう一度設定してください。 | Tunnel warning (app window, Settings → Internet). |  |
| `tunnel.rate_limited` | Cloudflare is temporarily refusing new quick tunnels (too many attempts). Retrying in {seconds} s. | Cloudflareが新しいクイックトンネルを一時的に拒否しています（試行回数が多すぎます）。{seconds}秒後に再試行します。 | Tunnel status reason (app window). |  |
| `tunnel.updating` | cloudflared is being updated — the tunnel will be back in a moment. | cloudflaredを更新しています。まもなくトンネルが復旧します。 | Tunnel status reason (app window). |  |

### ui

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `ui.close` | Close | 閉じる | New interface: button that closes a dialog or a message. |  |
| `ui.error_status` | The program answered with error {status}. | プログラムからエラー{status}が返されました。 | New interface: message of a failed request that came without a description; {status} is the HTTP status number. |  |
| `ui.no_answer` | The program does not answer. Check that VRCinema is running. | プログラムが応答しません。VRCinemaが動作しているか確認してください。 | New interface: message when a request gets no answer at all (the program is closed or restarting). |  |
| `ui.window_only` | Available in the program on the computer | PCのプログラムで利用できます | New interface: note in place of a control that works only in the program's own window, not in the remote panel. |  |

### units

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `units.hours_minutes` | {h} h {m} min | {h}時間{m}分 | A duration with hours and minutes, inside other messages (e.g. errors.too_long). |  |
| `units.minutes` | {m} min | {m}分 | A duration in minutes, inside other messages. |  |
| `units.seconds` | {s} s | {s}秒 | A duration in seconds, inside other messages. |  |

### viewer

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `viewer.add_button` | Add to queue | キューに追加 | Request page (the page viewers open): button that adds the video link to the queue. Short. |  |
| `viewer.add_heading` | Add a video | 動画を追加 | Request page: small heading of the form with the video link and the nick. |  |
| `viewer.added` | Added “{title}” to the queue. | 「{title}」をキューに追加しました。 | Request page: short message after a video was queued. {title} is the video's title. |  |
| `viewer.added_by_host` | added by the host | 追加：ホスト | Request page: detail of a video the channel owner added, shown where a viewer's nick would be (Now playing and Up next). Lower case, short, impersonal; "host" is the owner's badge (ticker.added_by_host). | Request page, shown where a nick would be: 追加：ホスト, the same form as the ticker. |
| `viewer.adding` | Checking the video… | 動画を確認中… | Request page: label of the add button while the channel checks the link (can take several seconds). |  |
| `viewer.channel_link` | Channel link | チャンネルのリンク | Request page: screen-reader name of the box that holds the channel's address (in the 'How to watch' section). |  |
| `viewer.copied` | Copied | コピーしました | Request page: the copy button for a second after the channel address was copied. |  |
| `viewer.copy` | Copy | コピー | Request page: button that copies the channel address. |  |
| `viewer.hint_any` | Paste a link to a video. | 動画のリンクを貼り付けてください。 | Request page: hint under the form when any site is accepted. |  |
| `viewer.howto_no_seek` | Seeking in the player does not work, because this is a live channel. | ライブ配信なので、プレーヤーで再生位置を動かすことはできません。 | Request page, 'How to watch in VRChat': third point. The channel is a live stream, so the in-world player cannot seek. |  |
| `viewer.howto_paste` | Paste this link into the video player in the world: | ワールドの動画プレーヤーにこのリンクを貼り付けてください： | Request page, 'How to watch in VRChat': introduces the channel address in the box below. |  |
| `viewer.howto_same_moment` | Everyone sees the same moment of the video. Whoever joins later jumps into the video that is already playing. | 全員が動画の同じ場面を見ます。あとから参加した人は、再生中の動画の途中から見ることになります。 | Request page, 'How to watch in VRChat': second point. |  |
| `viewer.howto_title` | How to watch in VRChat | VRChatでの視聴方法 | Request page: title of the collapsible section with instructions for VRChat. |  |
| `viewer.howto_untrusted` | Everyone in the instance must turn on Allow Untrusted URLs in VRChat (Settings, Comfort & Safety tab), or they will see a black screen. In Public and Group Public instances VRChat may block this address even with that option on (the world decides which sites play there); if the screen stays black, try a Friends or Invite instance. | インスタンス内の全員が、VRChatの設定（Settings）の「Comfort & Safety」タブで「Allow Untrusted URLs」をオンにする必要があります。オフのままだと画面が真っ黒になります。PublicやGroup Publicのインスタンスでは、このオプションがオンでもVRChatがこのアドレスをブロックすることがあります（そこで再生できるサイトはワールドが決めます）。画面が黒いままの場合は、FriendsやInviteのインスタンスで試してみてください。 | Request page, 'How to watch in VRChat': first point. 'Allow Untrusted URLs', 'Settings' and 'Comfort & Safety' are the names in the VRChat menu; keep them in English (or as the game shows them in your language). 'Public', 'Group Public', 'Friends' and 'Invite' are VRChat's instance types; keep them as the game shows them. The second part is deliberately hedged ('may block'): since VRChat build 1546 the option no longer opens every address in Public and Group Public instances (the world's list of allowed domains decides there), which the project has not verified itself; it ends with what to try (a Friends or Invite instance). Informal address like the rest of the request page. | VRChat menu names kept in English (Settings, Comfort & Safety, Allow Untrusted URLs); if the Japanese VRChat client shows Japanese names, they should be used. Instance types (Public, Group Public, Friends, Invite) as the game shows them. The second part is hedged (ブロックすることがあります) and ends with a soft suggestion (試してみてください); the two parts are joined without a space after 。. |
| `viewer.language` | Language | 言語 | Request page: screen-reader name of the language selector (the selector lists the language names in themselves). |  |
| `viewer.limit` | A video can be up to {limit} long. | 動画の長さは{limit}までです。 | Request page, under the form. {limit} is a length such as '2 h' or '30 min' (viewer.limit_hours / viewer.limit_minutes). | {limit} is viewer.limit_hours / _minutes: 2時間, 30分. The page joins this sentence and viewer.moderation without a space in Japanese (sentences() in public/app/i18n.js). |
| `viewer.limit_hours` | {n} h | {n}時間 | Request page: a length in hours inside viewer.limit; {n} is a number that may have one decimal. |  |
| `viewer.limit_minutes` | {n} min | {n}分 | Request page: a length in minutes inside viewer.limit; {n} is a whole number. |  |
| `viewer.link_label` | Video link | 動画のリンク | Request page: label of the field for the video's address. |  |
| `viewer.link_placeholder` | Paste a link to a video… | 動画のリンクを貼り付け… | Request page: example text shown in the empty link field. |  |
| `viewer.live_stream` | Live stream | ライブ配信 | Request page, 'Now playing' card: shown beside the LIVE tag for a live stream (no progress bar). |  |
| `viewer.moderation` | The channel owner approves requests before they are played. | リクエストは、チャンネルのオーナーが承認してから再生されます。 | Request page, under the form, after viewer.limit: new requests wait for the owner's approval. |  |
| `viewer.nick_hint` | Optional. Shown next to the videos added with it. | 省略可。このニックネームで追加した動画の横に表示されます。 | Request page: small hint under the nick field. The nick appears next to the videos added with it. Impersonal wording. |  |
| `viewer.nick_label` | Your nick | ニックネーム | Request page: label of the optional nick field (maximum 24 characters). | en "Your nick"; no pronoun. |
| `viewer.now_playing` | Now playing | 再生中 | Request page: small heading of the card with the video that is playing now. |  |
| `viewer.offline` | Cannot reach the channel. Trying again… | チャンネルに接続できません。再試行しています… | Request page: shown at the top while the channel does not answer; the page retries by itself. |  |
| `viewer.send_failed` | Could not send the request. Check the connection and try again. | リクエストを送信できませんでした。接続を確認して、もう一度お試しください。 | Request page: shown under the form when a request could not be sent at all (no connection, or no answer in time). The page does not send it again by itself, so the viewer is asked to check the connection and press the button again. Informal like the rest of the request page. | Soft polite request-page tone (お試しください as in errors.too_many_requests). |
| `viewer.paused` | Paused | 一時停止中 | Request page: small tag in the 'Now playing' heading when the owner paused the channel. |  |
| `viewer.pending` | “{title}” is waiting for the channel owner's approval. | 「{title}」はチャンネルのオーナーの承認待ちです。 | Request page: short message after a request that waits for the owner's approval. {title} is the video's title. |  |
| `viewer.progress` | Progress of the video | 動画の進行状況 | Request page: screen-reader name of the progress bar of the playing video. |  |
| `viewer.queue_empty` | The queue is empty. Add the first video. | キューは空です。最初の動画を追加してみましょう。 | Request page: shown instead of the 'Up next' list when it is empty. | Soft invitation: 追加してみましょう. |
| `viewer.sites_none` | No sites are allowed on this channel right now. | 現在、このチャンネルで許可されているサイトはありません。 | Request page: the owner allows only listed sites, but the list is empty, so every link is refused. |  |
| `viewer.sites_title` | Links from these sites work: | 次のサイトのリンクが使えます： | Request page: introduces the list of sites that work (shown only when the owner allows listed sites only). Ends with a colon; the sites follow. |  |
| `viewer.title` | Video channel | 動画チャンネル | Request page: heading and browser tab title (the channel has no name of its own). | Heading and tab title (the channel has no name). |
| `viewer.use_nick` | Use “{nick}” | 「{nick}」を使う | Request page: button after a nick was refused as taken; it fills the field with a free alternative. {nick} is that nick. |  |
| `viewer.your_icon` | Your icon on this device | このデバイスでのアイコン | Request page: screen-reader name and tooltip of the viewer's own small icon beside the nick field. | No pronoun: このデバイスでのアイコン. |

### wizard

| Key | English | Japanese | Context | Notes |
|---|---|---|---|---|
| `wizard.done` | Done | 完了 | Wizard, internet step: button that ends the wizard once the channel is on the internet. |  |
| `wizard.encoder_cpu` | No supported card was found, so the processor converts the picture. The channel will work, it only loads the computer more. | 対応するグラフィックボードが見つからなかったため、CPUで映像を変換します。チャンネルは動作しますが、PCの負荷が大きくなります。 | Wizard step 3: result when only the processor can be used. |  |
| `wizard.encoder_failed` | The test failed: {error} | テストに失敗しました：{error} | Wizard step 3: the test failed; {error} is the reason. |  |
| `wizard.encoder_gpu` | The graphics card will convert the picture. | グラフィックボードで映像を変換します。 | Wizard step 3: result, under the name of the card type. |  |
| `wizard.encoder_intro` | Converting the picture on the graphics card is much lighter for the computer than on the processor. I am checking what can be used. | 映像の変換は、CPUよりグラフィックボードで行うほうがPCへの負荷がずっと軽くなります。使用できる変換方式を確認しています。 | Wizard step 3: why the test is made. | en first person ("I am checking"); subject-less: 使用できる変換方式を確認しています. |
| `wizard.encoder_skip` | Skip this step | このステップをスキップ | Wizard step 3: button after a failed test that goes on. |  |
| `wizard.encoder_testing` | Checking the graphics card. It takes a few seconds… | グラフィックボードを確認しています。数秒かかります… | Wizard step 3: shown while the test runs. |  |
| `wizard.encoder_title` | Graphics card test | グラフィックボードのテスト | Wizard step 3: title. |  |
| `wizard.language_hint` | The language of the program: the interface, the tray menu and the log. You can change it later in Settings. | プログラムの言語（画面、通知領域のメニュー、ログ）です。あとで「設定」から変更できます。 | First wizard step: hint under the language choice. |  |
| `wizard.language_title` | Language | 言語 | First wizard step: heading of the language choice (a list of buttons with the language names). |  |
| `wizard.net_changes` | This address changes every time the program starts. If you want a permanent link, you can set it up now or later in Settings. | このアドレスはプログラムを起動するたびに変わります。固定リンクは今すぐ、またはあとで「設定」から作成できます。 | Wizard step 4: note under the links about the quick link and the permanent link. |  |
| `wizard.net_channel_link` | Channel link: paste it into the VRChat player | チャンネルのリンク：VRChatのプレーヤーに貼り付けてください | Wizard step 4: label of the channel link (the .m3u8 address). VRChat is a game name, keep it. |  |
| `wizard.net_connecting` | Connecting to the internet… | インターネットに接続しています… | Wizard step 4: shown while the connection is being made. |  |
| `wizard.net_failed` | Could not connect to the internet. | インターネットに接続できませんでした。 | Wizard step 4: the connection failed (no reason given). |  |
| `wizard.net_intro` | The program connects the channel to the internet by itself, so the player in VRChat can open it. You do not have to install or type anything. | プログラムがチャンネルを自動でインターネットに接続するので、VRChatのプレーヤーで開けるようになります。インストールや入力の必要はありません。 | Wizard step 4: what happens. VRChat is a game name, keep it. |  |
| `wizard.net_online` | The channel is on the internet. | チャンネルはインターネットに接続されています。 | Wizard step 4: the connection works. |  |
| `wizard.net_qr_alt` | QR code of the request page | リクエストページのQRコード | Wizard step 4: alternative text of the QR code image. |  |
| `wizard.net_qr_hint` | Viewers can scan this code with a phone to add a video. | 視聴者はこのコードをスマートフォンでスキャンして動画を追加できます。 | Wizard step 4: next to the QR code. |  |
| `wizard.net_request_link` | Request page for viewers | 視聴者用のリクエストページ | Wizard step 4: label of the address of the page where viewers add videos. |  |
| `wizard.net_retrying` | Could not connect to the internet: the program keeps trying. | インターネットに接続できませんでした。再試行を続けています。 | Wizard step 4: shown while the connection is lost and the program retries. |  |
| `wizard.net_skip_hint` | You can try again or skip this step: the channel then works only on this computer, and you can connect later in Settings, in the Internet section. | 再試行するか、このステップをスキップできます。スキップした場合、チャンネルはこのPCでのみ動作します。あとで「設定」の「インターネット」セクションから接続できます。 | Wizard step 4: what can be done when there is no connection. |  |
| `wizard.net_title` | Your channel on the internet | インターネットでチャンネルを公開 | Wizard step 4: title. |  |
| `wizard.next` | Next | 次へ | Wizard: button that goes to the next step. |  |
| `wizard.permanent` | I want a permanent link | 固定リンクを使う | Wizard, internet step: button that ends the wizard and opens Settings, Internet, where the permanent link is set up. |  |
| `wizard.progress` | Wizard progress | ウィザードの進行状況 | Wizard: accessible label of the row of progress dots. |  |
| `wizard.retry` | Try again | 再試行 | Wizard: button that repeats a failed step. |  |
| `wizard.skip` | Skip | スキップ | Wizard, internet step: button that ends the wizard without the internet connection. |  |
| `wizard.start` | Let's start | 始める | Button of the first wizard step that goes to the downloading of the tools. | Alt.: はじめる, 開始. |
| `wizard.step` | Step {n} of {total} | ステップ{n}/{total} | Wizard: which step is shown; {n} is the step, {total} the number of steps. | ステップ{n}/{total}, also in the permanent link wizard (ステップ1/2). |
| `wizard.tools_failed` | The tools could not be downloaded: {error} | ツールをダウンロードできませんでした：{error} | Wizard step 2: the download failed; {error} is the reason. |  |
| `wizard.tools_incomplete` | Not all the tools were installed. | 一部のツールがインストールされませんでした。 | Wizard step 2: the download ended but a tool is still missing (used as the {error} of the failure message). |  |
| `wizard.tools_intro` | Downloading the programs the channel uses. Do not close the window. | チャンネルで使うプログラムをダウンロードしています。ウィンドウを閉じないでください。 | Wizard step 2: what happens now. |  |
| `wizard.tools_present` | The tools are already installed. | ツールはすでにインストールされています。 | Wizard step 2: nothing had to be downloaded. |  |
| `wizard.tools_ready` | All the tools are ready. | すべてのツールの準備ができました。 | Wizard step 2: the download is finished. |  |
| `wizard.tools_skip` | Skip and go to the panel | スキップしてパネルへ | Wizard step 2: button after a failed download that ends the wizard and opens the program. |  |
| `wizard.tools_title` | Downloading the tools | ツールのダウンロード | Wizard step 2: title. |  |
| `wizard.welcome_intro` | This program runs your own live video channel for VRChat: you add videos to the queue, and the player in your VRChat world shows them one after another. | このプログラムで、VRChat用の自分だけのライブ動画チャンネルを運用できます。キューに動画を追加すると、VRChatのワールドにあるプレーヤーで順番に再生されます。 | First wizard step: what the program does. VRChat is a game name, keep it. | 自分だけの…を運用できます (review fix 1). |
| `wizard.welcome_setup` | First, a few helper programs have to be downloaded once and the graphics card checked. It takes a few minutes, and I will guide you step by step. | 最初に、いくつかの補助プログラムを一度だけダウンロードし、グラフィックボードを確認します。数分かかりますが、画面の案内に沿って順番に進められます。 | First wizard step: what the setup will do. | en "I will guide you"; 画面の案内に沿って順番に進められます (no subject). |
| `wizard.welcome_title` | Welcome to VRCinema | VRCinemaへようこそ | Title of the first wizard step (first run of the program). |  |
