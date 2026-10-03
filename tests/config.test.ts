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

  it('odrzuca nieznany enkoder', () => {
    expect(() => loadConfig({ ADMIN_PASSWORD: 'haslo123', VIDEO_ENCODER: 'foo' })).toThrow(/VIDEO_ENCODER/);
  });
});
