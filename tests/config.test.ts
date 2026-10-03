import { describe, it, expect } from 'vitest';
import path from 'node:path';
import { loadConfig } from '../src/config.ts';

describe('loadConfig', () => {
  it('wymaga ADMIN_PASSWORD', () => {
    expect(() => loadConfig({})).toThrow(/ADMIN_PASSWORD/);
  });

  it('odrzuca domyślne i zbyt krótkie hasło', () => {
    expect(() => loadConfig({ ADMIN_PASSWORD: 'zmien-mnie' })).toThrow(/ADMIN_PASSWORD/);
    expect(() => loadConfig({ ADMIN_PASSWORD: 'abc' })).toThrow(/ADMIN_PASSWORD/);
  });

  it('ustawia wartości domyślne', () => {
    const c = loadConfig({ ADMIN_PASSWORD: 'haslo123' }, '/proj', 'win32');
    expect(c.port).toBe(8080);
    expect(c.host).toBe('0.0.0.0');
    expect(c.segmentSeconds).toBe(4);
    expect(c.windowSize).toBe(10);
    expect(c.videoEncoder).toBe('libx264');
    expect(c.trustCloudflare).toBe(false);
    expect(c.dataDir).toBe(path.resolve('/proj', 'data'));
    expect(c.ffmpegPath).toBe(path.resolve('/proj', 'bin', 'ffmpeg.exe'));
    expect(c.ytdlpPath).toBe(path.resolve('/proj', 'bin', 'yt-dlp.exe'));
    expect(c.sessionSecret.length).toBeGreaterThanOrEqual(32);
  });

  it('na Linuksie używa nazw bez .exe', () => {
    const c = loadConfig({ ADMIN_PASSWORD: 'haslo123' }, '/proj', 'linux');
    expect(c.ffmpegPath).toBe(path.resolve('/proj', 'bin', 'ffmpeg'));
  });

  it('czyta nadpisania z env', () => {
    const c = loadConfig(
      {
        ADMIN_PASSWORD: 'haslo123',
        PORT: '9000',
        VIDEO_ENCODER: 'h264_nvenc',
        TRUST_CLOUDFLARE: 'true',
        FFMPEG_PATH: '/usr/bin/ffmpeg',
        PUBLIC_URL: 'https://tv.example.com/',
      },
      '/proj',
      'linux',
    );
    expect(c.port).toBe(9000);
    expect(c.videoEncoder).toBe('h264_nvenc');
    expect(c.trustCloudflare).toBe(true);
    expect(c.ffmpegPath).toBe('/usr/bin/ffmpeg');
    expect(c.publicUrl).toBe('https://tv.example.com');
  });

  it('MEDIA_DIRS: lista folderów rozdzielona średnikami, domyślnie pusta', () => {
    expect(loadConfig({ ADMIN_PASSWORD: 'haslo123' }, '/proj', 'win32').mediaDirs).toEqual([]);
    const c = loadConfig({ ADMIN_PASSWORD: 'haslo123', MEDIA_DIRS: ' D:/Filmy ; E:/Seriale;;' }, '/proj', 'win32');
    expect(c.mediaDirs).toEqual(['D:/Filmy', 'E:/Seriale']);
  });

  it('odrzuca nieznany enkoder', () => {
    expect(() => loadConfig({ ADMIN_PASSWORD: 'haslo123', VIDEO_ENCODER: 'foo' })).toThrow(/VIDEO_ENCODER/);
  });
});

describe('readEnvFile', () => {
  it('czyta plik ze znacznikiem BOM (Notatnik, PowerShell) i końcami linii CRLF', async () => {
    const { readEnvFile } = await import('../src/config.ts');
    const fs = await import('node:fs');
    const os = await import('node:os');
    const path = await import('node:path');
    const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'env-')), '.env');
    fs.writeFileSync(file, '\uFEFFADMIN_PASSWORD=haslo123\r\nMEDIA_DIRS=D:\\Filmy;E:\\Seriale\r\n# komentarz\r\nPORT=9000\r\n');
    expect(readEnvFile(file)).toEqual({ ADMIN_PASSWORD: 'haslo123', MEDIA_DIRS: 'D:\\Filmy;E:\\Seriale', PORT: '9000' });
  });

  it('nie nadpisuje zmiennych już ustawionych w środowisku', async () => {
    const { applyEnv } = await import('../src/config.ts');
    const env: Record<string, string | undefined> = { PORT: '1' };
    applyEnv(env, { PORT: '2', ADMIN_PASSWORD: 'x' });
    expect(env).toEqual({ PORT: '1', ADMIN_PASSWORD: 'x' });
  });
});
