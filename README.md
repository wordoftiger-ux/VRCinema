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
