<p align="center">
  <img src="desktop/icons/icon.png" alt="VRCinema" width="128">
</p>

<h1 align="center">VRCinema</h1>

<p align="center"><b>Your own cinema in VRChat.</b><br>
Paste a link or drop a video file, and the whole lobby watches it together, at the same moment.</p>

<p align="center"><a href="README.pl.md">Po polsku</a> · <a href="docs/manual.md">Full manual</a></p>

---

## What is it?

VRCinema is a Windows program that turns the videos **you** pick into one never-ending live channel. You give VRChat a single link, once. After that you only add films to a queue, and they play one after another, for everybody, in sync.

- **You** add films from YouTube and many other sites, or from your own disk.
- **Your friends** can suggest films too, from a simple web page (with a nick, and optionally with your approval).
- **The world's video player** in VRChat plays the channel link. It never has to be changed.

When the queue is empty, the channel shows a calm waiting screen (with a QR code to the request page), so the player in VRChat never disconnects.

## Getting started

1. **Download** `VRCinema.exe` from the [Releases](https://github.com/wordoftiger-ux/VRCinema/releases) page. No installer: put it anywhere and run it. (Windows 10/11, 64-bit.)
2. **Follow the short setup.** It downloads the helper tools it needs, picks the best video encoder for your computer and starts the channel. You can skip every step, the defaults are fine.
3. **Copy the link for VRChat** at the end of the setup (later: the **Copy link** button at the bottom of the window).
4. **Paste it** into the video player of a VRChat world.
5. **Add films**: paste a link into the window (or press <kbd>Ctrl</kbd>+<kbd>V</kbd>), or drag video files onto it.

> Windows may say "Windows protected your PC" because the file is not code-signed. Click **More info**, then **Run anyway**.

## What you can do

**Run the show**
- A **Preview** of the film you are working with (it reacts at once: pause, jump, skip) and an **On air** picture of what the viewers see, a few seconds later.
- A queue with "play next", cutting the start or the end of a film, starting a film at a given time and a countdown before it begins.
- Remove everything a viewer added, or block them.
- A small player stays with you when you open the other tabs.

**Let friends join**
- A request page for viewers: they add links (or small video and music files) under a nick protected by a PIN.
- Limits, approval, playlists from viewers and a block list: you decide how open it is.

**Make it look good**
- An info band on the picture: the title, who added it, what is next, your own messages, a QR code and a progress line.
- Slates for pauses and waiting, with your own picture if you like. Music gets a cover and moving bars.
- Optional smooth fades, loudness levelling and subtitles.

**Go further**
- A **public link** is built in: no account and no setup. A permanent link on your own domain is optional.
- **OBS** can go live on the channel (and takes priority over the queue).
- Show a window, a screen or a camera from this computer.
- A **remote panel** to run the channel from a phone, and a short "now playing" text in the VRChat chat box.
- Six languages: English, Polish, Spanish, German, French and Japanese (beta).

## Good to know

- Your computer sends the video, so your **upload speed** matters: the more people watch, the more it needs. The program warns you when the connection looks too weak.
- Most videos are only repackaged, not converted, so the load on your computer is small.
- Everything is stored in `%LOCALAPPDATA%\VRCinema`. Deleting that folder resets the program.

## Without a window

On Linux there is also a headless server with the same panel. See the [manual](docs/manual.md#linux-server-without-window).

## Please use it responsibly

VRCinema is meant for watching **privately with friends**. Whoever runs it is responsible for what is streamed. Re-streaming films you have no rights to can break copyright law and the terms of the sites they come from.

## More

- The [full manual](docs/manual.md) has every setting, troubleshooting and the technical details.
- Licence: [MIT](LICENSE).
- [Third-party software and licences](docs/third-party-licenses.md) (FFmpeg, yt-dlp, Deno, cloudflared, the interface and Rust libraries).
