import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { loadConfig } from '../src/config.ts';
import { openDatabase, MAIN_CHANNEL_ID } from '../src/db/database.ts';
import { QueueRepo } from '../src/db/queue.ts';
import { SettingsRepo } from '../src/db/settings.ts';
import { Channel } from '../src/pipeline/channel.ts';
import { createClipFactory } from '../src/pipeline/clipFactory.ts';
import { Broadcaster } from '../src/pipeline/broadcaster.ts';
import { generateSlates } from '../src/pipeline/slates.ts';
import type { MediaResolver } from '../src/resolver/types.ts';

const cfg = loadConfig({ ADMIN_PASSWORD: 'haslo123' });
const hasTools = fs.existsSync(cfg.ffmpegPath) && fs.existsSync(cfg.ffprobePath);

let work: string;

function makeClip(file: string, size: string, rate: number, withAudio: boolean, seconds: number) {
  const args = ['-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=${size}:rate=${rate}:duration=${seconds}`];
  if (withAudio) args.push('-f', 'lavfi', '-i', `sine=frequency=440:duration=${seconds}`);
  args.push('-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', '-g', String(rate * 2));
  if (withAudio) args.push('-c:a', 'aac');
  args.push(file);
  const r = spawnSync(cfg.ffmpegPath, args);
  if (r.status !== 0) throw new Error(String(r.stderr));
}

function packetDts(file: string, stream: 'v' | 'a'): number[] {
  const r = spawnSync(
    cfg.ffprobePath,
    ['-v', 'error', '-select_streams', stream, '-show_entries', 'packet=dts', '-of', 'csv=p=0', file],
    { encoding: 'utf8', maxBuffer: 1e8 },
  );
  return r.stdout.split(/\r?\n/).map((l) => parseFloat(l)).filter((n) => Number.isFinite(n));
}

beforeAll(() => {
  if (!hasTools) return;
  work = fs.mkdtempSync(path.join(os.tmpdir(), 'integ-'));
  makeClip(path.join(work, 'a.mp4'), '1280x720', 25, true, 6);
  makeClip(path.join(work, 'b.mp4'), '1920x1080', 30, false, 6);
});

afterAll(() => {
  if (work) fs.rmSync(work, { recursive: true, force: true });
});

describe.skipIf(!hasTools)('kanał od początku do końca (prawdziwy ffmpeg)', () => {
  it('dwa różne filmy i plansze tworzą jeden strumień bez cofania znaczników czasu', async () => {
    const db = openDatabase(':memory:');
    const queue = new QueueRepo(db);
    const settings = new SettingsRepo(db);
    const resolver: MediaResolver = {
      fetchMeta: async () => {
        throw new Error('nieużywane');
      },
      resolve: async (url) => ({ inputs: [{ url, headers: {} }], title: url, durationS: 6, isLive: false }),
    };
    for (const name of ['a.mp4', 'b.mp4']) {
      queue.add({
        channelId: MAIN_CHANNEL_ID, sourceUrl: path.join(work, name), title: name, durationS: 6, isLive: false,
        status: 'queued', addedBy: 'admin', requesterName: null, requesterIp: null,
      });
    }
    const slates = await generateSlates({
      ffmpegPath: cfg.ffmpegPath, fontFile: cfg.fontFile, dir: path.join(work, 'slates'), requestUrl: '', segmentSeconds: 4,
    });
    const channel = new Channel({
      channelId: MAIN_CHANNEL_ID, queue, settings, slates,
      factory: createClipFactory({
        resolver, ffmpegPath: cfg.ffmpegPath, ffprobePath: cfg.ffprobePath, encoder: 'libx264',
        segmentSeconds: 4, clipsDir: path.join(work, 'clips'), settings, channelId: MAIN_CHANNEL_ID,
      }),
    });
    channel.start();

    let now = 0;
    const published: string[] = [];
    const liveDir = path.join(work, 'live');
    const broadcaster = new Broadcaster({
      dir: liveDir, windowSize: 10, segmentSeconds: 4, pathPrefix: '', source: channel,
      discontinuityTags: () => true, now: () => now, retainS: 100_000,
      onPublished: (s) => {
        published.push(`${s.seq}.ts`);
        channel.onPublished(s);
      },
    });
    broadcaster.start(false);

    // Przyspieszony zegar nadajnika; ffmpeg pracuje w swoim tempie.
    const deadline = Date.now() + 90_000;
    while (Date.now() < deadline) {
      const done = queue.listByStatus(MAIN_CHANNEL_ID, ['queued', 'playing']).length === 0;
      if (done && published.length > 8) break;
      now += 1000;
      broadcaster.tick();
      await new Promise((r) => setTimeout(r, 150));
    }
    broadcaster.stop();
    channel.stop();

    const statuses = queue.history(MAIN_CHANNEL_ID, 10).map((i) => [i.title, i.status, i.error]);
    expect(statuses).toEqual([
      ['b.mp4', 'played', null],
      ['a.mp4', 'played', null],
    ]);

    const joined = path.join(work, 'joined.ts');
    fs.writeFileSync(joined, Buffer.concat(published.map((n) => fs.readFileSync(path.join(liveDir, n)))));
    for (const stream of ['v', 'a'] as const) {
      const dts = packetDts(joined, stream);
      expect(dts.length).toBeGreaterThan(100);
      const backwards = dts.filter((d, i) => i > 0 && d <= dts[i - 1]);
      expect(backwards, `cofnięcia DTS w strumieniu ${stream}`).toEqual([]);
      const maxGap = Math.max(...dts.slice(1).map((d, i) => d - dts[i]));
      expect(maxGap, `największa luka w strumieniu ${stream}`).toBeLessThan(0.5 * 90_000);
    }
  }, 120_000);
});
