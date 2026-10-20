# VRChat HLS Channel

A small self-hosted server that turns links (YouTube, pixeldrain, CDA and anything else
[yt-dlp](https://github.com/yt-dlp/yt-dlp) understands) and video files from your own disk into
**one continuous live HLS channel**. Paste the channel link into a video player in a VRChat world once,
and the queue plays back to back. Everyone in the lobby sees the same moment.

Polish version: [README.pl.md](README.pl.md). The program speaks English, Polish, Spanish, German, French and Japanese (beta).

- **Player window / owner panel** (`/admin`): live preview, pause, seek, play now, the queue and the history, stats for nerds, settings.
- **Public request page** (`/`): friends can suggest videos with a nick, with rate limits and optional approval.
- **Never goes off air**: an idle screen plays when the queue is empty, so the VRChat player never disconnects.
- **Info ticker**: an optional band burned into the picture with the title, who added it, what comes next, your messages, a QR code of the request page and a progress line.
- **Light on CPU**: H.264 sources up to 1080p are repackaged without re-encoding. Everything else is converted, optionally on an NVIDIA, Intel or AMD GPU.

> **Intended for private watching with friends.** Whoever runs the server is responsible for what it streams.
> Re-streaming content you don't have rights to may break copyright law and the terms of the sites you stream from.

## Desktop app (Windows)

`VRCinema.exe` is the whole server in one window: no Node.js, no installer, no `.env` file.

- **Download** `VRCinema.exe` from the [Releases](https://github.com/wordoftiger-ux/vrcinema/releases) page (about 25 MB) and put it anywhere, for example on the desktop. Windows 10/11, 64-bit; it needs the Microsoft Edge WebView2 Runtime, which is already part of Windows 11 and most Windows 10 installations (the app tells you where to get it if it is missing).
- **First run**: a wizard asks for the language (it starts with the language of Windows), downloads the tools (ffmpeg, yt-dlp, the Deno runtime that YouTube needs and cloudflared for the public link, about 300 MB in total) into `%LOCALAPPDATA%\VRCinema`, tests your graphics card to pick the encoder (NVIDIA, Intel, AMD or CPU), starts the channel and shows its public link. After that the app opens straight to the player.
- **SmartScreen**: the file is not code-signed, so Windows may say "Windows protected your PC". Click **More info**, then **Run anyway**.
- **Adding films**: paste a link into the field above the queue (or press Ctrl+V anywhere outside a text field), or drag video files or whole folders onto the window. Dropped files are played from where they are on your disk.
- **Tray**: closing the window keeps the channel running in the tray (a balloon tells you the first time). The tray menu opens the window, copies the channel link or the request page link, skips the current film and quits. **Quit** from the tray stops the channel and every ffmpeg process.
- **Remote panel**: off by default; the panel is reachable only from this computer. To manage the channel from another device (a phone, a laptop), set a password in **Settings** → **Remote panel** (see [Remote panel and phone](#remote-panel-and-phone)).
- **Updates**: the app checks for a newer `VRCinema.exe` at start and every 24 hours and offers to install it (the file is verified with its SHA-256 and the old one is kept until the new one runs). This needs the repository's releases to be public.
- **Data**: everything is in `%LOCALAPPDATA%\VRCinema` (`config.json`, `tools\`, `data\`, `logs\`). Set `VRCINEMA_HOME` to use another folder. Deleting the folder resets the app.
- **Size and memory** (measured, release build): `VRCinema.exe` is 25 MB. While a film plays with the window open it uses about 45 MB itself; the WebView2 processes that draw the window take about 600 MB while the live preview plays; cloudflared takes about 40 MB and ffmpeg is separate (about 30 MB when copying, about 300 MB when converting on the graphics card).
- **No window** (a server, Docker): see [VPS with Docker](#vps-with-docker) and [Rust version](#rust-version-experimental) below; the headless `vrcinema` server has the same panel and request page.

### The player

The window looks and works like a media player. It has no Windows frame: the 32-pixel bar at the top is the title bar (drag it to move the window, double-click it to maximise; the minimise, maximise and close buttons are on the right; Windows snapping works). Close hides the window in the tray. The window remembers its size, position and maximised state.

- **Rail on the left** with four views:
  - **Channel**: the live preview of the stream (with the delay under it, "Preview · about 12 s behind the server"), the **Up next** queue (drag to reorder; the menu of an item has **Play now**, **Move to the top**, **Copy link** and **Remove from the queue**), the link field, **Pending approval** when approval is on, and the drop zone for files.
  - **History**: what has been on the channel (played, skipped, error) with **Play now** / **Try again** and **Add to queue**.
  - **Share**: the channel link for VRChat, the request page with its QR code, the internet status and the permanent link.
  - **Settings**: General (language, autostart, home network, port), Internet, Channel (links from viewers, limits, approval, language of the slates, info ticker, viewer nicks, conversion), Tools, Remote panel, Advanced, Log, About.
- **Player bar at the bottom**: the title and who added it; −10 s, ⏮ (from the beginning), pause/resume, ⏭ (next video), +10 s; a timeline you can click or drag (it shows the time under the pointer); the preview volume; the internet status; **Copy link** (the channel link for the VRChat player).
- **Keyboard shortcuts** (when the focus is not in a text field; **?** shows the list):

  | Key | Action |
  |---|---|
  | Space | Pause / resume |
  | ← / → | Back / forward 10 s |
  | Shift + ← / → | Back / forward 60 s |
  | Home | From the beginning of this video |
  | N | Next video |
  | I | Stats for nerds |
  | Ctrl + V | Add the link on the clipboard to the queue |
  | ? | Keyboard shortcuts |

- **Context menu**: right-click a queue item or the preview (the preview's menu has **Stats for nerds** and **Copy channel link**). Shift+F10 or the Menu key opens it from the keyboard; Alt+↑/↓ moves the focused queue item.

### Controls and the delay

- **Pause** shows "Paused — back in a moment" on the channel and keeps the position; **Resume** continues from the same place after a few seconds of the buffering screen. Pause, seek and ⏮ are off for live streams (⏭ works).
- **Seeking** (±10 s, ±60 s, the timeline, ⏮) restarts the same film at the new position. Several quick clicks are merged into one jump (the app waits 0.7 s after the last one), so eight clicks of +10 s are one jump of 80 s. A seek during a pause only moves the paused position. Seeking to the last second plays the next video.
- **Accuracy**: a copied (not converted) film jumps to the key frame at or before the target, up to one key-frame interval of the source earlier, often a few seconds (the time shows "≈"); a converted film (for example with the info ticker on) jumps exactly.
- **Play now** (queue or History) puts the video on air at once; the current one ends as skipped. From History it is added again as a new queue entry (same link and nick).
- **The delay**: the channel is cut into 4-second segments and players start a few segments behind the live edge, so **viewers see every change (pause, seek, next video) about 10–15 seconds later** than you press the button. The preview in the window has the same delay; after a seek a bubble says "Viewers will see this in about N s".
- **A manual pause survives server restarts** (a new port, home network access, repaired tools) **but not an app restart**: after quitting and starting VRCinema again the channel plays.

### Stats for nerds

Press **I** or right-click the preview → **Stats for nerds**: an overlay like YouTube's, refreshed every second while it is open. It shows the film on air (source, resolution, copy or conversion, encoder, ready segments), the HLS channel (sequence, discontinuities, whether the timestamps are continuous), the next film, ffmpeg (speed, CPU, memory), viewers (players that fetched a segment in the last 30 s, fetches per minute, upload with a 60-second graph), the tunnel (state, time online, reconnects, Cloudflare locations) and the preview in the window (delay, buffer, dropped frames). **Copy** copies it as text. Viewer addresses are only counted, never shown.

### Languages

- Three separate choices: **the program** (window, remote panel, tray, log: **Settings** → **General** → **Language**, and the first wizard step; it starts with the language of Windows if it is one of the six, otherwise English), **the slates and the info ticker** (**Settings** → **Channel** → **Language of the slates**, English by default — the language your VRChat viewers read), and **the request page** (each viewer picks it with the switch at the top; it starts with the browser's language and is remembered in that browser).
- Languages: English, Polski, Español, Deutsch, Français and 日本語 (beta). Error messages of the server come in the viewer's or panel's language.
- **Server without a window** (`vrcinema` with `.env`): English unless `LANGUAGE=pl` (or `es`, `de`, `fr`, `ja`) is set in `.env`; there is no language switch in its panel.
- **Japanese is a beta**: the texts were translated without a native speaker. All Japanese texts with their English source and context are in [docs/i18n/ja-review.md](docs/i18n/ja-review.md) for a native review. The slates and the ticker use the Windows font Yu Gothic (Meiryo as a fallback); without either the slates stay in English (with a log entry).
- **Contributing a translation**: the dictionaries are `public/locales/<language>.json` (English, `en.json`, is the source). `public/locales/context.json` says where each text appears and its length limit, and [docs/i18n/glossary.md](docs/i18n/glossary.md) fixes the terms. Keep the `{placeholders}` and the plural categories; `cargo test` in `engine/` checks every language for missing keys, placeholders, plurals and lengths.

### Info ticker

An optional band burned into the picture of the stream, so everyone in VRChat sees it: "▶ Now: <title> · added by <nick | Host> · next: <title> · add a video: <address>" plus your own messages, a QR code of the request page in the bottom right corner with the caption "Add a video", and a thin progress line at the bottom edge.

- **Settings** → **Channel** → **Info ticker**: on/off; mode **From time to time** (every 1, 2, 5 or 10 minutes; each time the information about the video and then the next of your messages) or **Continuous** (the information and all messages in a loop); scrolling speed (the panel shows how long the text stays on screen); show the next video; show the address; QR code **Off**, **With the band** or **Always**; progress line; up to 10 messages of up to 200 characters (they are not translated).
- **Hardware**: with the ticker on, every film is converted (the ticker is drawn into the picture), which is light on an NVIDIA, Intel or AMD graphics card and heavy on the processor. It is switched on by default only when the wizard found a graphics card encoder. If the computer cannot keep up (conversion slower than real time for 30 s), the log and the panel say so: turn the ticker off or use the graphics card (**Settings** → **Tools**).
- **When changes show up**: the text (your messages, "next", the address, the language of the slates, queue changes) changes on the stream within about **20–60 seconds** (the film is prepared ahead of time; about 20 s for the film on air, up to about a minute for a film that was prepared in advance). Switching the ticker on or off, the mode, interval, speed, QR code or progress line restarts the film on air from where it is (a few seconds of the buffering screen); so does a new public address while the QR code is shown.
- The text is white with a shadow on a dark band; the progress line is amber. Live streams have no progress line.

### Viewer nicks

- Viewers can type a nick (up to 24 characters) on the request page. The first request with a nick creates a random device key in that browser; the server keeps only its SHA-256 hash. The nick then belongs to that device for 30 days after its last use: the same nick from another browser or device is refused ("This nick is already taken by someone else"). Look-alike nicks count as the same nick: capitals, accents, spaces, dots, dashes, invisible characters and Cyrillic or Greek look-alikes are ignored (Kasia, KASIA, kásia, K a s i a, Ka5ia are one nick). "Host" and its translations are reserved.
- Next to the nick the panel and the request page show an icon (a colour and a shape) made from the device key. It is a **visual aid** to spot impersonation, not proof of identity. Your own videos show the **Host** badge.
- **Settings** → **Channel** → **Viewer nicks** lists the nicks with their icons; **Release** frees a nick, **Block device** refuses requests from that device (and **Unblock** lifts it).
- Limits: a nick needs the device key, so it works only in a browser that keeps its site data; a viewer in a private window or after clearing the site data gets a new key, so the old nick shows as taken; they can pick another nick or send without one. A blocked viewer can still send requests **without a nick** from a new browser profile (anonymously); the request limits per address still apply, and approval (**Settings** → **Channel** → approval) stops anything you don't want.

### Remote panel and phone

- The remote panel is the same interface as the window at `<public address>/admin` (or `http://<this computer>:<port>/admin` with home network access), after logging in with the password from **Settings** → **Remote panel**. It can do everything the window can **except five things that stay in the window**: the port and home network access, the password and switching the remote panel off, **Trust Cloudflare headers** (it decides who a request comes from), the Cloudflare login (and therefore setting up and deleting the permanent link), and picking the cookies file (a file dialog on the computer; the remote panel can detach it and sees only its file name, not its folder). Dropping files and the first-run wizard are window-only too. Those places show "Available in the program on the computer".
- **Sign out**: **Settings** → **General** → **Sign out** (on a phone: the icon at the top right). Changing the password or switching the remote panel off signs out every remote device.
- **Tunnel switch**: when you opened the panel through the tunnel, switching the tunnel off (or reconnecting it, or changing between the quick and the permanent link) asks first, because it ends your own connection ("You will lose access to this panel").
- **Phone layout**: below 700 px of width the rail becomes tabs at the bottom and the player bar sits under the preview with bigger buttons. The pixeldrain API key can be replaced but is never shown back.

### Public link (built-in tunnel)

The app puts the channel on the internet by itself: no account, no terminal, no address to type.

- **Quick tunnel, on by default**: at start the app runs its own copy of [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) and gets an address such as `https://abc-def.trycloudflare.com`. The player bar shows **Online** and **Copy link** copies the channel link (`<address>/live/main.m3u8`); the tray menu copies it too.
- **The address changes every time the app starts** (and when the tunnel has to reconnect). Give friends the new link, or set up a permanent link (below).
- **It replaces running your own cloudflared.** If you started `cloudflared tunnel --url …` yourself with the previous version, you no longer need to. While the built-in tunnel is on, the public address typed by hand (**Settings** → **Advanced**) and the "trust Cloudflare" option are not used; they apply only with the tunnel switched off in **Settings** → **Internet**. A configuration from the previous version gets the tunnel switched on too, and the app downloads the missing cloudflared by itself in the background (again every 5 minutes while it fails, e.g. without internet).
- **QR code on the idle screen**: while the queue is empty the channel shows "No videos in the queue" with a QR code of the request page and the address under it, so friends in VRChat can scan it with a phone and add a film. A new address replaces it live, without a restart. Without an address the screen says "Connecting to the internet…". (The slates use the language of the slates, English by default.)
- **Switching it off**: **Settings** → **Internet** → "Share the channel on the internet". With the tunnel off the channel is reachable only from this computer (or at the address entered by hand).
- **VRChat and untrusted addresses**: every viewer has to enable **Allow Untrusted URLs** in VRChat. According to VRChat's feedback board, since VRChat build 1546 that setting no longer opens every address in **Public** and **Group Public** instances: there only the domains the world's creator allowed play. A `trycloudflare.com` link may therefore play only in Friends+, Friends, Invite or Group instances, and a permanent link on your own domain does not change that unless the world allows your domain. We have not verified this ourselves: try it in the instance type you use (see the checklist below).

### Permanent link (your own domain)

A link that never changes needs a free [Cloudflare](https://dash.cloudflare.com/sign-up) account and a domain of your own:

1. Buy a domain at any registrar (a cheap one is fine) and [add it to your Cloudflare account](https://developers.cloudflare.com/fundamentals/setup/manage-domains/add-site/): Cloudflare gives you two nameservers to set at the registrar. Wait until Cloudflare shows the domain as **Active**.
2. In VRCinema open **Settings** → **Internet** → **Set up a permanent link**. The app opens your browser once: log in to Cloudflare and authorize the domain. The app waits 5 minutes for the login; **Cancel** stops it, and after a cancel or a timeout **Try again** starts it again. With an account that has no domain yet the login never finishes (add the domain first, step 1).
3. Type the full host name for the channel, for example `kanal.example.com`. The app creates a tunnel called `vrcinema` and a DNS record for that name. If the name already has a record, it asks whether to replace it or pick another name.

From then on the channel link is `https://kanal.example.com/live/main.m3u8`, also after restarts and port changes. **Back to the quick tunnel** goes back to the quick tunnel (the permanent link stays set up); **Delete the permanent link** deletes the tunnel (if the DNS record is still there afterwards, delete it in the Cloudflare dashboard). The Cloudflare certificate and the tunnel credentials stay in `%LOCALAPPDATA%\VRCinema\tunnel\` and are never shown or logged.

### Automatic pause

Viewers don't miss part of a film because the connection dropped:

- **Internet lost** (on by default): when the tunnel has been down for 10 seconds, the film pauses and the channel shows "Paused — back in a moment". When the tunnel is back, the film resumes from the same place. Shorter drops pause nothing.
- **Nobody watching** (off by default): after 30 seconds without a viewer the film pauses; it resumes as soon as someone starts watching (the preview in the window counts as a viewer while the window is visible; a minimised window or one hidden in the tray stops the preview).
- Live streams are never paused. Both options are in **Settings** → **Internet** → "Pausing the channel". A resumed film starts after a few seconds of the buffering screen; that is expected.

### Owner's manual test checklist

These need a person at the screen and are not covered by the automatic tests:

- [ ] Clean Windows account (or a PC that never had the app): first run from an empty folder, wizard (language step first), first film plays.
- [ ] Frameless window, main window and wizard: drag by the title bar, double-click to maximise and back, the minimise / maximise / close buttons (close hides to the tray), Windows snapping (drag to a screen edge, Win+←/→/↑), resize from the edges; the size and position come back after a restart.
- [ ] Second monitor with a different scale (e.g. 100 % and 150 %): move the window there, quit, start again; it opens on that monitor at the right size; maximised there too.
- [ ] First start on a small or high-DPI screen (e.g. a 1366×768 laptop or 4K at 200 %): the window fits on the screen (minimum 960×600) and nothing is cut off.
- [ ] Minimise for about 60 s and restore: the preview jumps back to live (not 60 s behind) and its delay caption settles; while minimised or hidden in the tray the preview stops fetching the stream (the stats for nerds show the viewers drop); a window that is only covered by another window keeps playing.
- [ ] Remote panel on a real phone: log in at `<public address>/admin`, the tabs at the bottom, the player bar under the preview, pause and seek work, window-only items say "Available in the program on the computer", sign out works; a wrong password is refused; switching the panel off or changing the password logs the phone out.
- [ ] `/desktop-login`: open `http://127.0.0.1:<port>/desktop-login?key=anything` in a normal browser: it ends on `/admin` with the sign-in screen, never the panel (the window uses a one-time key of its own).
- [ ] Info ticker in VRChat: with the ticker on, watch a film in a VRChat world: the band is readable on the world's screen at a normal distance, the QR code can be scanned with a phone from VRChat and opens the request page, the progress line moves.
- [ ] Allow Untrusted URLs: play the channel link in a **Public** or **Group Public** instance and in a Friends or Invite instance (with Allow Untrusted URLs on). Note where it plays (see the VRChat note above).
- [ ] Japanese review: a native speaker reads [docs/i18n/ja-review.md](docs/i18n/ja-review.md) (and the app with 日本語 chosen) and sends corrections.
- [ ] Drag and drop end to end: one file, several files, a folder, a non-video file (skipped); the page shows the result and the film plays.
- [ ] Following a port change: change the port in **Settings** → **General**; the window reloads on the new address.
- [ ] Cookies picker: choose a cookies file in **Settings** → **Advanced** and add a link that needs it.
- [ ] Autostart on/off: with Windows it starts in the tray without a window; off removes it.
- [ ] Update restart: install a locally served newer release; the app restarts and the old file disappears.
- [ ] Link clicks: external links in the panel open in the default browser, the window stays on the app.
- [ ] Tray menu items: open the window, copy the channel link and the request page link, skip, quit; double click on the icon.
- [ ] Close-to-tray balloon: closing the window hides it and shows the notice once.
- [ ] Second launch: with the app running (window hidden in the tray, or minimized), start `VRCinema.exe` again; the existing window comes back to the front and no second copy starts (also after **Quit**: a new launch starts normally within a few seconds).
- [ ] Repair: with broken tools (e.g. replace `tools\ffmpeg.exe` in the data folder with an empty file and restart the app) the panel says the channel did not start and offers to restart the channel and to repair the tools (**Settings** → **Tools**); repairing brings the channel back.
- [ ] Permanent link end to end with a real domain: **Set up a permanent link** opens the browser once for the Cloudflare login; the host name step follows; the channel plays at `https://<host>/live/main.m3u8` and keeps that link after an app restart and a port change. Quitting VRCinema (tray → **Quit**) during or after the login does not close the browser. A host name that already has a DNS record shows the replace question, and replacing works. **Back to the quick tunnel** and **Delete the permanent link** work (the tunnel disappears from the Cloudflare dashboard).
- [ ] QR code from VRChat: with an empty queue, scan the QR code on the idle screen in a VRChat world with a phone; the request page opens, and a film added there plays.
- [ ] Automatic pause: during a film unplug the network cable (or switch Wi-Fi off) for at least 15 s; the status shows "No internet" and the film pauses; after reconnecting the tunnel comes back (the same address if cloudflared kept running, a new one if it had to restart) and the film continues from the same place.
- [ ] Update from the previous version: start the new `VRCinema.exe` on the data folder of the previous version (its `config.json`); the tunnel is on; the app downloads cloudflared by itself in the background; the panel shows the download, never "the channel is not on air", then the public link; a film may pause while the download takes longer than 10 s. Links from viewers keep the old list of allowed sites (mode "Only allowed sites").

## Requirements

- Windows 10/11 or Linux (macOS works with ffmpeg from Homebrew).
- [Node.js](https://nodejs.org) **22.18 or newer** (24 recommended).
- A public HTTPS address so other players can reach you, e.g. [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) or a VPS.

ffmpeg and yt-dlp are downloaded automatically into `bin/`.

> The Node.js server is the previous implementation. The new interface of stage 4 is served only by the desktop app and the Rust server (`vrcinema`, below); the Node.js server's old panel page is no longer in the repository, so use the Rust server for a server without a window.

## Quick start (Windows)

1. Download or clone this repository.
2. Double-click **`start.bat`**. The first run installs dependencies, downloads ffmpeg and yt-dlp, and opens `.env` in Notepad.
3. In `.env` set `ADMIN_PASSWORD` (at least 8 characters) and `SESSION_SECRET` (any long random string), save, then run `start.bat` again.
4. Open `http://localhost:8080/admin`, log in and paste a link.

Manual equivalent:

```powershell
npm install
npm run setup     # downloads ffmpeg/ffprobe/yt-dlp, creates .env
npm start
```

## Making it reachable from VRChat

The desktop app does this by itself (see [Public link](#public-link-built-in-tunnel) above). This section is for the Node.js server and the headless `vrcinema` server.

Every player downloads the stream from your server, so it needs a public address.

```powershell
winget install --id Cloudflare.cloudflared
cloudflared tunnel --url http://localhost:8080
```

cloudflared prints an address such as `https://abc-def.trycloudflare.com`.

1. In `.env` set `TRUST_CLOUDFLARE=true` and `PUBLIC_URL=https://abc-def.trycloudflare.com`, then restart the server.
2. The link for VRChat is `https://abc-def.trycloudflare.com/live/main.m3u8`.

Notes:

- **Every player in the lobby must enable "Allow Untrusted URLs"** in VRChat settings. Your domain is not on VRChat's allowlist. In Public and Group Public instances even that may not be enough (since VRChat build 1546 the world's list of allowed domains decides there; see [the note above](#public-link-built-in-tunnel)).
- **Paste the link into the world's normal video URL field.** Some worlds have special "custom media" menus that only accept their own services.
- The quick tunnel address changes every time cloudflared restarts. Tip: put the channel link into a [Pastebin](https://pastebin.com) paste and give friends the paste's *raw* link. VRChat follows it to your channel, and you only edit the paste when the tunnel address changes.
- Quick tunnels are meant for testing and Cloudflare's free network is not designed for heavy video traffic. For regular use, a VPS is more reliable (see below).

### Bandwidth

Each viewer pulls their own stream: about 6 Mbit/s at 1080p, so 10 viewers need about 60 Mbit/s of upload.

## Playing files from your disk

List the folders you want to share in `.env`, separated by semicolons:

```ini
MEDIA_DIRS=D:\Movies;E:\Series
```

The server can then queue a single file or a whole folder in natural order (episode 2 before 10) through the panel API (`/api/admin/library`, `/api/admin/channels/main/local`). Only the owner can do this. Viewers only see the title, never the path. The new interface has no file browser yet (the old panel had one); in the desktop app drag files or folders onto the window instead.

## Settings

| Setting (admin panel) | Default |
|---|---|
| Links from viewers | any site (a new database); an existing database keeps its list in the "Only allowed sites" mode |
| Longest video a viewer may request | 2 hours |
| Requests per address | 3 per 10 minutes |
| Queued videos per person | 5 |
| Approve requests before they are queued | off |

The owner can add anything yt-dlp supports, live streams, YouTube playlists and pixeldrain lists. These limits don't apply to the owner.

### Links from viewers and your home network

The link mode (**Settings** → **Channel** → **Links from viewers**; `linkMode` in the channel settings) is **Any site** (`all`; the default for a new database), **Only allowed sites** (`allowlist`: only the listed sites and their subdomains; what an existing database becomes, with the list it had) or **All except blocked sites** (`blocklist`: every site except the listed ones). Both lists are always kept; **Add popular sites** fills in YouTube, Twitch, Kick, Vimeo, Dailymotion, pixeldrain, CDA, SoundCloud, bilibili, niconico, Streamable, Reddit, X/Twitter and TikTok. In the "Only allowed sites" mode the request page lists the allowed sites.

In every mode, a link from a viewer must be `http` or `https` and must not point into your own network: `localhost`, `*.local`, `*.lan`, `*.home.arpa`, `*.internal` and single-word names such as `nas` are refused, and so is any name that resolves to a loopback, private (RFC 1918, `fc00::/7`), link-local, CGNAT (`100.64.0.0/10`), multicast or otherwise reserved address (also an IPv4 address written inside an IPv6 one). A name whose lookup times out is refused too. The check runs when the link arrives and again when the video starts (the name may have changed). The address of the video that yt-dlp finds is checked the same way and ffprobe and ffmpeg are given exactly the checked address, never the text yt-dlp wrote; an address with a backslash, a space or a control character is refused. Links added by the owner are never limited.

Known limits (what is checked is the address, not what happens after it is fetched):

- yt-dlp's own redirects, and ffmpeg's HTTP redirects (a public video address that answers 302 to a page on your network), are not checked.
- A playlist or manifest (HLS, DASH) on a public address may list segments on your network; ffmpeg follows them.
- A name that changes its answer between the check and the fetch (DNS rebinding) is not caught.
- What a page on your network says can leak: through yt-dlp's generic extractor the title or the error text of such a page could show up as a queue title or an item error.

If your network matters more than convenience, use the `allowlist` mode with sites you trust.

`.env` options are documented in [.env.example](.env.example). They cover the language (`LANGUAGE`), the GPU encoder (`VIDEO_ENCODER=h264_nvenc`), YouTube cookies (`YTDLP_COOKIES`), a pixeldrain API key, and paths.

## First transition test in VRChat

The VRChat player (AVPro) usually handles switching between videos with different resolutions well, but players differ.
Queue two short videos of different quality and watch the switch. If the picture freezes or the player disconnects, change these in the panel settings:

1. Turn off "Mark transitions between videos in the stream" (**Settings** → **Channel**; the `EXT-X-DISCONTINUITY` tag).
2. If that doesn't help, turn it back on and enable "Always convert the picture". The desktop app converts on the graphics card it found; the headless server needs `VIDEO_ENCODER=h264_nvenc` (or `h264_qsv`, `h264_amf`) in `.env`.

## VPS with Docker

Create `deploy/.env`:

```ini
ADMIN_PASSWORD=your-long-password
SESSION_SECRET=any-long-random-string
DOMAIN=tv.example.com
```

Point the domain at the server, then:

```bash
cd deploy
docker compose up -d --build
```

Caddy obtains the HTTPS certificate automatically. Channel link: `https://tv.example.com/live/main.m3u8`.

## Troubleshooting

- **A video ends up in history with an error.** The reason is shown next to it. yt-dlp updates itself at start and once a day.
- **YouTube asks to sign in.** Export browser cookies to `cookies.txt` and set `YTDLP_COOKIES`.
- **pixeldrain hits its download limit.** Set `PIXELDRAIN_API_KEY`.
- **Viewers see a black screen.** Check "Allow Untrusted URLs" and that the link starts with `https://`. In a Public or Group Public instance try a Friends or Invite instance (see the VRChat note in [Public link](#public-link-built-in-tunnel)).

## Rust version (experimental)

There is a second implementation of the same server written in Rust, in `engine/`. It is a drop-in replacement: same HTTP API, same `.env`, same `data/` folder and database. It is one small executable instead of a Node.js process, and it uses about 22 MB of RAM (release build) while a film plays (measured working set; the debug build uses 29 MB; ffmpeg is a separate process and is not included).
The Node.js version stays in this repository until the switch-over is done.

```powershell
cd engine
cargo build --release     # needs Rust; result: engine\target\release\vrcinema.exe
cd ..
engine\target\release\vrcinema.exe setup   # downloads ffmpeg/ffprobe/yt-dlp, creates .env
engine\target\release\vrcinema.exe         # starts the channel (run it from the project folder)
```

Run only one version at a time on the same `data/` folder. To go back, stop `vrcinema` and start the Node.js version (`npm start`); the database is compatible both ways.
Switch over from a fresh PowerShell window: overrides set earlier with `$env:PORT` or `$env:DATA_DIR` linger in the old window and win over `.env`. The Rust version locks the data folder (`vrcinema.lock`), so a second Rust server on the same `data/` folder refuses to start.

## How it works

yt-dlp (or the disk library) resolves a video. ffmpeg cuts it into 4-second MPEG-TS segments, repackaging H.264 without re-encoding whenever possible.
A broadcaster publishes the segments in real time and rewrites their timestamps, so videos and idle screens form one continuous timeline.
Design notes: [docs/superpowers/specs](docs/superpowers/specs).

```powershell
npm test          # some tests use the real ffmpeg from ./bin
npm run typecheck
```

## License

[MIT](LICENSE)
