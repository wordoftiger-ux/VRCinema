// Pobiera ffmpeg, ffprobe i yt-dlp do ./bin. Użycie: npm run setup [-- --force]
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const root = path.resolve(import.meta.dirname, '..');
const bin = path.join(root, 'bin');
const force = process.argv.includes('--force');
const win = process.platform === 'win32';
const exe = win ? '.exe' : '';

const YTDLP_URL = win
  ? 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe'
  : process.platform === 'darwin'
    ? 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos'
    : 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux';
const FFMPEG_URL = win
  ? 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip'
  : 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz';

async function download(url: string, dest: string): Promise<void> {
  console.log(`Pobieranie ${url}`);
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok || !res.body) throw new Error(`Pobieranie nie powiodło się: ${res.status} ${url}`);
  await pipeline(Readable.fromWeb(res.body as never), fs.createWriteStream(dest));
}

async function findFile(dir: string, name: string): Promise<string | null> {
  for (const entry of await fsp.readdir(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = await findFile(p, name);
      if (found) return found;
    } else if (entry.name === name) {
      return p;
    }
  }
  return null;
}

async function setupYtDlp(): Promise<void> {
  const dest = path.join(bin, `yt-dlp${exe}`);
  if (fs.existsSync(dest) && !force) return console.log('yt-dlp już jest.');
  await download(YTDLP_URL, dest);
  if (!win) await fsp.chmod(dest, 0o755);
}

async function setupFfmpeg(): Promise<void> {
  const ffmpeg = path.join(bin, `ffmpeg${exe}`);
  const ffprobe = path.join(bin, `ffprobe${exe}`);
  if (fs.existsSync(ffmpeg) && fs.existsSync(ffprobe) && !force) return console.log('ffmpeg już jest.');
  if (process.platform === 'darwin') {
    throw new Error('Na macOS zainstaluj ffmpeg przez Homebrew i ustaw FFMPEG_PATH/FFPROBE_PATH w .env.');
  }
  const tmp = path.join(bin, 'ffmpeg-tmp');
  await fsp.rm(tmp, { recursive: true, force: true });
  await fsp.mkdir(tmp, { recursive: true });
  const archive = path.join(tmp, win ? 'ffmpeg.zip' : 'ffmpeg.tar.xz');
  await download(FFMPEG_URL, archive);
  console.log('Rozpakowywanie ffmpeg...');
  // Na Windows systemowy bsdtar rozpakowuje zip; tar z Git Bash tego nie potrafi.
  const tar = win ? path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe') : 'tar';
  const r = spawnSync(tar, ['-xf', archive, '-C', tmp], { stdio: 'inherit' });
  if (r.status !== 0) throw new Error('Rozpakowanie ffmpeg nie powiodło się (wymagany program tar).');
  for (const [name, dest] of [[`ffmpeg${exe}`, ffmpeg], [`ffprobe${exe}`, ffprobe]] as const) {
    const found = await findFile(tmp, name);
    if (!found) throw new Error(`W archiwum brak ${name}`);
    await fsp.copyFile(found, dest);
    if (!win) await fsp.chmod(dest, 0o755);
  }
  await fsp.rm(tmp, { recursive: true, force: true });
}

await fsp.mkdir(bin, { recursive: true });
await setupYtDlp();
await setupFfmpeg();
for (const tool of ['ffmpeg', 'yt-dlp']) {
  const args = tool === 'ffmpeg' ? ['-hide_banner', '-version'] : ['--version'];
  const r = spawnSync(path.join(bin, `${tool}${exe}`), args, { encoding: 'utf8' });
  console.log(`${tool}: ${(r.stdout || r.stderr || 'błąd').split('\n')[0]}`);
}
if (!process.argv.includes('--no-env') && !fs.existsSync(path.join(root, '.env'))) {
  await fsp.copyFile(path.join(root, '.env.example'), path.join(root, '.env'));
  console.log('Utworzono .env z .env.example — ustaw w nim ADMIN_PASSWORD.');
}
console.log('Gotowe.');
