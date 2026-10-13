# VRChat HLS Channel

A small self-hosted server that turns links (YouTube, pixeldrain, CDA and anything else
[yt-dlp](https://github.com/yt-dlp/yt-dlp) understands) and video files from your own disk into
**one continuous live HLS channel**. Paste the channel link into a video player in a VRChat world once,
and the queue plays back to back. Everyone in the lobby sees the same moment.

Polish version: [README.pl.md](README.pl.md). The web interface is currently in Polish.

- **Owner panel** (`/admin`, password protected): add videos, playlists or files from your disk, reorder, skip, live preview, settings.
- **Public request page** (`/`): friends can suggest videos, with rate limits and optional approval.
- **Never goes off air**: an idle screen plays when the queue is empty, so the VRChat player never disconnects.
- **Light on CPU**: H.264 sources up to 1080p are repackaged without re-encoding. Everything else is converted, optionally on an NVIDIA, Intel or AMD GPU.

> **Intended for private watching with friends.** Whoever runs the server is responsible for what it streams.
> Re-streaming content you don't have rights to may break copyright law and the terms of the sites you stream from.

## Desktop app (Windows)

`VRCinema.exe` is the whole server in one window: no Node.js, no installer, no `.env` file.

- **Download** `VRCinema.exe` from the [Releases](https://github.com/wordoftiger-ux/vrcinema/releases) page (about 19 MB) and put it anywhere, for example on the desktop. Windows 10/11, 64-bit; it needs the Microsoft Edge WebView2 Runtime, which is already part of Windows 11 and most Windows 10 installations (the app tells you where to get it if it is missing).
- **First run**: a wizard downloads the tools (ffmpeg, yt-dlp, the Deno runtime that YouTube needs and cloudflared for the public link, about 300 MB in total) into `%LOCALAPPDATA%\VRCinema`, tests your graphics card to pick the encoder (NVIDIA, Intel, AMD or CPU), starts the channel and shows its public link. After that the app opens straight to the panel.
- **SmartScreen**: the file is not code-signed, so Windows may say "Windows protected your PC". Click **More info**, then **Run anyway**.
- **Adding films**: drag video files or whole folders onto the window, or paste links in the panel. Dropped files are played from where they are on your disk.
- **Tray**: closing the window keeps the channel running in the tray (a balloon tells you the first time). The tray menu opens the panel, copies the channel link, skips the current film and quits. **Quit** from the tray stops the channel and every ffmpeg process.
- **Remote panel**: off by default; the panel is reachable only from this computer. To manage the channel from another device, enable it in the **Ustawienia** tab and set a password.
- **Updates**: the app checks for a newer `VRCinema.exe` at start and every 24 hours and offers to install it (the file is verified with its SHA-256 and the old one is kept until the new one runs). This needs the repository's releases to be public.
- **Data**: everything is in `%LOCALAPPDATA%\VRCinema` (`config.json`, `tools\`, `data\`, `logs\`). Set `VRCINEMA_HOME` to use another folder. Deleting the folder resets the app.
- **Size and memory** (measured, release build): `VRCinema.exe` is 19.4 MB. With the window open it uses about 40 MB itself plus about 330 MB for the WebView2 processes that draw the window; cloudflared takes about 40 MB and ffmpeg is separate.
- **No window** (a server, Docker): see [VPS with Docker](#vps-with-docker) and [Rust version](#rust-version-experimental) below; the headless `vrcinema` server is unchanged.

### Public link (built-in tunnel)

The app puts the channel on the internet by itself: no account, no terminal, no address to type.

- **Quick tunnel, on by default**: at start the app runs its own copy of [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) and gets an address such as `https://abc-def.trycloudflare.com`. The badge at the top of the panel shows **W sieci** (online) with a button that copies the channel link (`<address>/live/main.m3u8`); the tray menu copies it too.
- **The address changes every time the app starts** (and when the tunnel has to reconnect). Give friends the new link, or set up a permanent link (below).
- **It replaces running your own cloudflared.** If you started `cloudflared tunnel --url …` yourself with the previous version, you no longer need to. While the built-in tunnel is on, the public address typed by hand (**Ustawienia** → **Zaawansowane**) and the "trust Cloudflare" option are not used; they apply only with the tunnel switched off in **Ustawienia** → **Internet**. A configuration from the previous version gets the tunnel switched on too, and the app downloads the missing cloudflared by itself in the background (again every 5 minutes while it fails, e.g. without internet).
- **QR code on the idle screen**: while the queue is empty the channel shows "Brak filmów w kolejce" with a QR code of the request page and the address under it, so friends in VRChat can scan it with a phone and add a film. A new address replaces it live, without a restart. Without an address the screen says "Łączenie z internetem…".
- **Switching it off**: **Ustawienia** → **Internet** → "Udostępniaj kanał w internecie". With the tunnel off the channel is reachable only from this computer (or at the address entered by hand).

### Permanent link (your own domain)

A link that never changes needs a free [Cloudflare](https://dash.cloudflare.com/sign-up) account and a domain of your own:

1. Buy a domain at any registrar (a cheap one is fine) and [add it to your Cloudflare account](https://developers.cloudflare.com/fundamentals/setup/manage-domains/add-site/): Cloudflare gives you two nameservers to set at the registrar. Wait until Cloudflare shows the domain as **Active**.
2. In VRCinema open **Ustawienia** → **Internet** → **Ustaw stały link**. The app opens your browser once: log in to Cloudflare and authorize the domain. The app waits 5 minutes for the login; **Anuluj** stops it, and after a cancel or a timeout **Spróbuj ponownie** starts it again. With an account that has no domain yet the login never finishes (add the domain first, step 1).
3. Type the full host name for the channel, for example `kanal.example.com`. The app creates a tunnel called `vrcinema` and a DNS record for that name. If the name already has a record, it asks whether to overwrite it or pick another name.

From then on the channel link is `https://kanal.example.com/live/main.m3u8`, also after restarts and port changes. **Wróć do szybkiego tunelu** goes back to the quick tunnel (the permanent link stays set up); **Usuń stały link** deletes the tunnel (if the DNS record is still there afterwards, delete it in the Cloudflare dashboard). The Cloudflare certificate and the tunnel credentials stay in `%LOCALAPPDATA%\VRCinema\tunnel\` and are never shown or logged.

### Automatic pause

Viewers don't miss part of a film because the connection dropped:

- **Internet lost** (on by default): when the tunnel has been down for 10 seconds, the film pauses and the channel shows "Pauza — wracamy za chwilę". When the tunnel is back, the film resumes from the same place. Shorter drops pause nothing.
- **Nobody watching** (off by default): after 30 seconds without a viewer the film pauses; it resumes as soon as someone starts watching (the preview in the panel counts as a viewer).
- Live streams are never paused. Both options are in **Ustawienia** → **Internet**. A resumed film starts after a few seconds of the buffering screen; that is expected.

### Owner's manual test checklist

These need a person at the screen and are not covered by the automatic tests:

- [ ] Clean Windows account (or a PC that never had the app): first run from an empty folder, wizard, first film plays.
- [ ] Drag and drop end to end: one file, several files, a folder, a non-video file (skipped); the page shows the result and the film plays.
- [ ] Following a port change: change the port in **Ustawienia**; the window reloads on the new address.
- [ ] Cookies picker: choose a cookies file in **Ustawienia** and add a link that needs it.
- [ ] Autostart on/off: with Windows it starts in the tray without a window; off removes it.
- [ ] Update restart: install a locally served newer release; the app restarts and the old file disappears.
- [ ] Link clicks: external links in the panel open in the default browser, the window stays on the app.
- [ ] Tray menu items: open panel, copy link, skip, quit; double click on the icon.
- [ ] Close-to-tray balloon: closing the window hides it and shows the notice once.
- [ ] Second launch: with the app running (window hidden in the tray, or minimized), start `VRCinema.exe` again; the existing window comes back to the front and no second copy starts (also after **Zakończ**: a new launch starts normally within a few seconds).
- [ ] Remote panel: in **Ustawienia** set a password and switch the remote panel on (with access from the network or a public address); log in from another device (e.g. a phone) at `/admin`; a wrong password is refused; switching the panel off or changing the password logs that device out.
- [ ] Repair banner: with broken tools (e.g. replace `tools\ffmpeg.exe` in the data folder with an empty file and restart the app) the panel shows "Kanał nie wystartował…" with **Uruchom ponownie kanał** and **Napraw narzędzia**; repairing brings the channel back.
- [ ] Permanent link end to end with a real domain: **Ustaw stały link** opens the browser once for the Cloudflare login; the host name step follows; the channel plays at `https://<host>/live/main.m3u8` and keeps that link after an app restart and a port change. Quitting VRCinema (tray → **Zakończ**) during or after the login does not close the browser. A host name that already has a DNS record shows the overwrite question, and overwriting works. **Wróć do szybkiego tunelu** and **Usuń stały link** work (the tunnel disappears from the Cloudflare dashboard).
- [ ] QR code from VRChat: with an empty queue, scan the QR code on the idle screen in a VRChat world with a phone; the request page opens, and a film added there plays.
- [ ] Automatic pause: during a film unplug the network cable (or switch Wi-Fi off) for at least 15 s; the badge shows "Brak internetu — kanał wstrzymany" and the film pauses; after reconnecting the tunnel comes back (the same address if cloudflared kept running, a new one if it had to restart) and the film continues from the same place.
- [ ] Update from the previous version: start the new `VRCinema.exe` on the data folder of the previous version (its `config.json`); the tunnel is on; the app downloads cloudflared by itself in the background; the panel shows the download (the banner "Pobieranie programu cloudflared…", never "kanał nie nadaje"), then the public link; a film may pause while the download takes longer than 10 s.

## Requirements

- Windows 10/11 or Linux (macOS works with ffmpeg from Homebrew).
- [Node.js](https://nodejs.org) **22.18 or newer** (24 recommended).
- A public HTTPS address so other players can reach you, e.g. [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) or a VPS.

ffmpeg and yt-dlp are downloaded automatically into `bin/`.

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

- **Every player in the lobby must enable "Allow Untrusted URLs"** in VRChat settings. Your domain is not on VRChat's allowlist.
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

The admin panel gets a **"Pliki z dysku"** (Files from disk) section. Browse the folders, queue a single file, or queue a whole folder in natural order (episode 2 before 10). Only the owner can do this. Viewers only see the title, never the path.

## Settings

| Setting (admin panel) | Default |
|---|---|
| Sites viewers may request from | youtube.com, youtu.be, pixeldrain.com, cda.pl |
| Longest video a viewer may request | 2 hours |
| Requests per IP address | 3 per 10 minutes |
| Queued videos per person | 5 |
| Approve requests before they are queued | off |

The owner can add anything yt-dlp supports, live streams, YouTube playlists and pixeldrain lists. These limits don't apply to the owner.

### Links from viewers and your home network

The link mode (`linkMode` in the channel settings) is `all` (any site; the default for a new database), `allowlist` (only the listed sites and their subdomains; what an existing database becomes, with the list it had) or `blocklist` (every site except the listed ones). Both lists are always kept.

In every mode, a link from a viewer must be `http` or `https` and must not point into your own network: `localhost`, `*.local`, `*.lan`, `*.home.arpa`, `*.internal` and single-word names such as `nas` are refused, and so is any name that resolves to a loopback, private (RFC 1918, `fc00::/7`), link-local, CGNAT (`100.64.0.0/10`), multicast or otherwise reserved address (also an IPv4 address written inside an IPv6 one). A name whose lookup times out is refused too. The check runs when the link arrives and again when the video starts (the name may have changed). The address of the video that yt-dlp finds is checked the same way and ffprobe and ffmpeg are given exactly the checked address, never the text yt-dlp wrote; an address with a backslash, a space or a control character is refused. Links added by the owner are never limited.

Known limits (what is checked is the address, not what happens after it is fetched):

- yt-dlp's own redirects, and ffmpeg's HTTP redirects (a public video address that answers 302 to a page on your network), are not checked.
- A playlist or manifest (HLS, DASH) on a public address may list segments on your network; ffmpeg follows them.
- A name that changes its answer between the check and the fetch (DNS rebinding) is not caught.
- What a page on your network says can leak: through yt-dlp's generic extractor the title or the error text of such a page could show up as a queue title or an item error.

If your network matters more than convenience, use the `allowlist` mode with sites you trust.

`.env` options are documented in [.env.example](.env.example). They cover the GPU encoder (`VIDEO_ENCODER=h264_nvenc`), YouTube cookies (`YTDLP_COOKIES`), a pixeldrain API key, and paths.

## First transition test in VRChat

The VRChat player (AVPro) usually handles switching between videos with different resolutions well, but players differ.
Queue two short videos of different quality and watch the switch. If the picture freezes or the player disconnects, change these in the panel settings:

1. Turn off "Oznaczaj przejścia między filmami" (mark transitions with `EXT-X-DISCONTINUITY`).
2. If that doesn't help, turn it back on and enable "Zawsze konwertuj obraz" (always re-encode). With an NVIDIA GPU, set `VIDEO_ENCODER=h264_nvenc`.

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
- **Viewers see a black screen.** Check "Allow Untrusted URLs" and that the link starts with `https://`.

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
