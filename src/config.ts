import path from 'node:path';
import crypto from 'node:crypto';

export const VIDEO_ENCODERS = ['libx264', 'h264_nvenc', 'h264_qsv', 'h264_amf'] as const;
export type VideoEncoder = (typeof VIDEO_ENCODERS)[number];

export interface AppConfig {
  port: number;
  host: string;
  adminPassword: string;
  sessionSecret: string;
  /** Publiczny adres serwera bez końcowego "/", np. https://tv.example.com. Pusty = wyliczany z żądania. */
  publicUrl: string;
  dataDir: string;
  ffmpegPath: string;
  ffprobePath: string;
  ytdlpPath: string;
  videoEncoder: VideoEncoder;
  trustCloudflare: boolean;
  trustProxy: boolean;
  /** Opcjonalny plik cookies.txt dla yt-dlp. */
  ytdlpCookies: string;
  /** Czcionka do plansz. Pusta = wykrywana automatycznie. */
  fontFile: string;
  segmentSeconds: number;
  windowSize: number;
}

type Env = Record<string, string | undefined>;

function bool(v: string | undefined): boolean {
  return v !== undefined && ['1', 'true', 'yes', 'tak'].includes(v.trim().toLowerCase());
}

function defaultFont(platform: NodeJS.Platform): string {
  if (platform === 'win32') return 'C:/Windows/Fonts/arial.ttf';
  if (platform === 'darwin') return '/System/Library/Fonts/Supplemental/Arial.ttf';
  return '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf';
}

export function loadConfig(
  env: Env = process.env,
  rootDir: string = process.cwd(),
  platform: NodeJS.Platform = process.platform,
): AppConfig {
  const adminPassword = env.ADMIN_PASSWORD?.trim();
  if (!adminPassword) {
    throw new Error('Brak ADMIN_PASSWORD w pliku .env — ustaw hasło do panelu administratora.');
  }
  const encoder = (env.VIDEO_ENCODER?.trim() || 'libx264') as VideoEncoder;
  if (!VIDEO_ENCODERS.includes(encoder)) {
    throw new Error(`Nieznany VIDEO_ENCODER "${encoder}". Dozwolone: ${VIDEO_ENCODERS.join(', ')}`);
  }
  const exe = platform === 'win32' ? '.exe' : '';
  const binDir = path.resolve(rootDir, 'bin');
  const port = Number(env.PORT ?? 8080);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) throw new Error(`Niepoprawny PORT: ${env.PORT}`);

  return {
    port,
    host: env.HOST?.trim() || '0.0.0.0',
    adminPassword,
    sessionSecret: env.SESSION_SECRET?.trim() || crypto.randomBytes(32).toString('hex'),
    publicUrl: (env.PUBLIC_URL?.trim() || '').replace(/\/+$/, ''),
    dataDir: path.resolve(rootDir, env.DATA_DIR?.trim() || 'data'),
    ffmpegPath: env.FFMPEG_PATH?.trim() || path.join(binDir, `ffmpeg${exe}`),
    ffprobePath: env.FFPROBE_PATH?.trim() || path.join(binDir, `ffprobe${exe}`),
    ytdlpPath: env.YTDLP_PATH?.trim() || path.join(binDir, `yt-dlp${exe}`),
    videoEncoder: encoder,
    trustCloudflare: bool(env.TRUST_CLOUDFLARE),
    trustProxy: bool(env.TRUST_PROXY),
    ytdlpCookies: env.YTDLP_COOKIES?.trim() || '',
    fontFile: env.FONT_FILE?.trim() || defaultFont(platform),
    segmentSeconds: 4,
    windowSize: 10,
  };
}
