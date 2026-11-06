// A short real HLS stream for the browser checks of the preview: 40 s of a test picture and a
// tone cut into 2 s segments by the repo's bin/ffmpeg.exe, written to a temporary folder.
//
//   const stream = await makeTestStream();   // {dir, cleanup()}; makeTestStream({color: 'red'}): plain red
//
// The fake engine serves it as /live/main.m3u8 (without EXT-X-ENDLIST, so the player treats
// it as live).

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const FFMPEG = join(fileURLToPath(new URL('.', import.meta.url)), '..', '..', 'bin', 'ffmpeg.exe');

/** A 4-minute MP4 film (test picture with its own clock, a tone; fast start) for the Preview. */
export async function makeTestFilm() {
  if (!existsSync(FFMPEG)) throw new Error(`bin/ffmpeg.exe not found (${FFMPEG})`);
  const dir = mkdtempSync(join(tmpdir(), 'ui-check-film-'));
  const file = join(dir, 'film.mp4');
  const args = [
    '-hide_banner', '-loglevel', 'error',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=25',
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
    '-t', '240',
    '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '25', '-pix_fmt', 'yuv420p',
    '-c:a', 'aac', '-movflags', '+faststart',
    file,
  ];
  await new Promise((resolve, reject) => {
    const child = spawn(FFMPEG, args, { stdio: ['ignore', 'ignore', 'pipe'] });
    let err = '';
    child.stderr.on('data', (d) => (err += d));
    child.on('error', reject);
    child.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`ffmpeg exited ${code}: ${err}`))));
  });
  return { file, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

export async function makeTestStream({ color = null } = {}) {
  if (!existsSync(FFMPEG)) throw new Error(`bin/ffmpeg.exe not found (${FFMPEG})`);
  const dir = mkdtempSync(join(tmpdir(), 'ui-check-hls-'));
  // `color` (e.g. 'red'): a plain picture of that colour instead of the test pattern.
  const picture = color ? `color=c=${color}:size=320x180:rate=25` : 'testsrc2=size=320x180:rate=25';
  const args = [
    '-hide_banner', '-loglevel', 'error',
    '-f', 'lavfi', '-i', picture,
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
    '-t', '40',
    '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '50', '-pix_fmt', 'yuv420p',
    '-c:a', 'aac',
    '-f', 'hls', '-hls_time', '2', '-hls_list_size', '0',
    '-hls_segment_filename', join(dir, '%d.ts'),
    join(dir, 'main.m3u8'),
  ];
  await new Promise((resolve, reject) => {
    const child = spawn(FFMPEG, args, { stdio: ['ignore', 'ignore', 'pipe'] });
    let err = '';
    child.stderr.on('data', (d) => (err += d));
    child.on('error', reject);
    child.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`ffmpeg exited ${code}: ${err}`))));
  });
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}
