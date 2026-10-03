import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { loadConfig, type AppConfig } from './config.ts';
import { openDatabase, MAIN_CHANNEL_ID } from './db/database.ts';
import { QueueRepo } from './db/queue.ts';
import { SettingsRepo } from './db/settings.ts';
import { YtDlpResolver } from './resolver/ytdlp.ts';
import { PixeldrainResolver } from './resolver/pixeldrain.ts';
import { CompositeResolver } from './resolver/index.ts';
import { LocalResolver } from './resolver/local.ts';
import { MediaLibrary } from './library/library.ts';
import { probeDuration } from './pipeline/probe.ts';
import { generateSlates } from './pipeline/slates.ts';
import { Channel } from './pipeline/channel.ts';
import { createClipFactory } from './pipeline/clipFactory.ts';
import { Broadcaster } from './pipeline/broadcaster.ts';
import { buildApp } from './http/app.ts';
import { runProcess } from './util/process.ts';

const root = path.resolve(import.meta.dirname, '..');

function log(msg: string): void {
  const time = new Date().toLocaleTimeString('pl-PL');
  console.log(`[${time}] ${msg}`);
}

function loadEnv(): AppConfig {
  const envFile = path.join(root, '.env');
  if (fs.existsSync(envFile)) process.loadEnvFile(envFile);
  try {
    return loadConfig(process.env, root);
  } catch (e) {
    console.error(`\nBłąd konfiguracji: ${(e as Error).message}\n`);
    process.exit(1);
  }
}

function checkTools(cfg: AppConfig): void {
  const missing = [cfg.ffmpegPath, cfg.ffprobePath, cfg.ytdlpPath].filter((p) => !fs.existsSync(p));
  if (missing.length > 0) {
    console.error(`\nBrak narzędzi: ${missing.join(', ')}\nUruchom najpierw: npm run setup\n`);
    process.exit(1);
  }
}

async function updateYtDlp(cfg: AppConfig): Promise<void> {
  try {
    const res = await runProcess(cfg.ytdlpPath, ['-U'], { timeoutMs: 120_000 });
    const line = `${res.stdout}\n${res.stderr}`.split(/\r?\n/).filter((l) => l.trim()).at(-1) ?? '';
    log(`yt-dlp: ${line}`);
  } catch (e) {
    log(`Nie udało się zaktualizować yt-dlp: ${(e as Error).message}`);
  }
}

function lanAddresses(): string[] {
  return Object.values(os.networkInterfaces())
    .flat()
    .filter((a) => a && a.family === 'IPv4' && !a.internal)
    .map((a) => a!.address);
}

async function main(): Promise<void> {
  const cfg = loadEnv();
  checkTools(cfg);
  if (!process.env.SESSION_SECRET) {
    log('Uwaga: brak SESSION_SECRET w .env — po restarcie trzeba będzie zalogować się ponownie.');
  }

  const db = openDatabase(path.join(cfg.dataDir, 'vrcinema.db'));
  const queue = new QueueRepo(db);
  const settings = new SettingsRepo(db);

  const library = new MediaLibrary(cfg.mediaDirs);
  for (const r of library.roots()) {
    if (!fs.existsSync(r.path)) log(`Uwaga: folder z MEDIA_DIRS nie istnieje: ${r.path}`);
  }
  const resolver = new CompositeResolver(
    new PixeldrainResolver({
      probeDuration: (url, headers) => probeDuration(cfg.ffprobePath, url, headers),
      apiKey: cfg.pixeldrainApiKey || undefined,
    }),
    new YtDlpResolver({
      ytdlpPath: cfg.ytdlpPath,
      jsRuntime: process.execPath,
      cookiesFile: cfg.ytdlpCookies || undefined,
    }),
    library.enabled ? new LocalResolver(library, (file) => probeDuration(cfg.ffprobePath, file, {})) : null,
  );

  const clipsDir = path.join(cfg.dataDir, 'clips');
  fs.rmSync(clipsDir, { recursive: true, force: true });
  fs.mkdirSync(clipsDir, { recursive: true });

  log('Generowanie plansz...');
  const slates = await generateSlates({
    ffmpegPath: cfg.ffmpegPath,
    fontFile: cfg.fontFile,
    dir: path.join(cfg.dataDir, 'slates'),
    requestUrl: cfg.publicUrl ? `${cfg.publicUrl}/` : '',
    segmentSeconds: cfg.segmentSeconds,
  });

  const channel = new Channel({
    channelId: MAIN_CHANNEL_ID,
    queue,
    settings,
    slates,
    log,
    factory: createClipFactory({
      resolver,
      ffmpegPath: cfg.ffmpegPath,
      ffprobePath: cfg.ffprobePath,
      encoder: cfg.videoEncoder,
      segmentSeconds: cfg.segmentSeconds,
      clipsDir,
      settings,
      channelId: MAIN_CHANNEL_ID,
      log,
    }),
  });
  channel.start();

  const broadcaster = new Broadcaster({
    dir: path.join(cfg.dataDir, 'live', 'main'),
    windowSize: cfg.windowSize,
    segmentSeconds: cfg.segmentSeconds,
    pathPrefix: 'main/',
    source: channel,
    discontinuityTags: () => settings.get(MAIN_CHANNEL_ID).discontinuityTags,
    onPublished: (seg) => channel.onPublished(seg),
    log,
  });
  broadcaster.start();

  const app = await buildApp({
    config: cfg,
    channelId: MAIN_CHANNEL_ID,
    queue,
    settings,
    resolver,
    channel,
    broadcaster,
    publicDir: path.join(root, 'public'),
    library,
    log,
  });
  await app.listen({ port: cfg.port, host: cfg.host });

  const local = `http://localhost:${cfg.port}`;
  const lines = [
    '',
    '  Kanał działa.',
    '',
    `  Panel administratora:  ${local}/admin`,
    `  Strona zgłoszeń:       ${local}/`,
    `  Link kanału (lokalnie): ${local}/live/main.m3u8`,
    ...lanAddresses().map((ip) => `  W sieci domowej:        http://${ip}:${cfg.port}/live/main.m3u8`),
    ...(cfg.publicUrl ? [`  Link dla VRChat:        ${cfg.publicUrl}/live/main.m3u8`] : [
      '',
      '  Dla VRChat potrzebny jest adres publiczny, np.:',
      `    cloudflared tunnel --url ${local}`,
    ]),
    '',
  ];
  console.log(lines.join('\n'));

  void updateYtDlp(cfg);
  const updater = setInterval(() => void updateYtDlp(cfg), 24 * 3600 * 1000);

  let closing = false;
  const shutdown = async () => {
    if (closing) return;
    closing = true;
    log('Zamykanie...');
    clearInterval(updater);
    broadcaster.stop();
    channel.stop();
    await app.close().catch(() => {});
    db.close();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
