import { describe, it, expect } from 'vitest';
import { isAllowedDomain, validateMeta, RateLimiter, parseSourceUrl } from '../src/queue/rules.ts';
import { DEFAULT_SETTINGS } from '../src/db/settings.ts';

const domains = DEFAULT_SETTINGS.allowedDomains;

describe('parseSourceUrl', () => {
  it('akceptuje http(s) i przycina spacje', () => {
    expect(parseSourceUrl('  https://youtu.be/abc ')?.href).toBe('https://youtu.be/abc');
  });
  it('odrzuca inne schematy i śmieci', () => {
    expect(parseSourceUrl('file:///etc/passwd')).toBeNull();
    expect(parseSourceUrl('nie link')).toBeNull();
    expect(parseSourceUrl('')).toBeNull();
  });
});

describe('isAllowedDomain', () => {
  it('przepuszcza domenę i poddomeny', () => {
    expect(isAllowedDomain(new URL('https://www.youtube.com/watch?v=1'), domains)).toBe(true);
    expect(isAllowedDomain(new URL('https://m.youtube.com/watch?v=1'), domains)).toBe(true);
    expect(isAllowedDomain(new URL('https://youtu.be/1'), domains)).toBe(true);
    expect(isAllowedDomain(new URL('https://www.cda.pl/video/1'), domains)).toBe(true);
  });
  it('blokuje domeny podobne i spoza listy', () => {
    expect(isAllowedDomain(new URL('https://evilyoutube.com/x'), domains)).toBe(false);
    expect(isAllowedDomain(new URL('https://youtube.com.evil.net/x'), domains)).toBe(false);
    expect(isAllowedDomain(new URL('https://vimeo.com/1'), domains)).toBe(false);
  });
});

describe('validateMeta', () => {
  const meta = { title: 't', durationS: 100, isLive: false, isPlaylist: false };
  it('przepuszcza poprawny film', () => {
    expect(validateMeta(meta, DEFAULT_SETTINGS, false)).toBeNull();
  });
  it('odrzuca za długi film dla publicznych, ale nie dla admina', () => {
    const long = { ...meta, durationS: 7201 };
    expect(validateMeta(long, DEFAULT_SETTINGS, false)).toMatch(/za długi/);
    expect(validateMeta(long, DEFAULT_SETTINGS, true)).toBeNull();
  });
  it('odrzuca transmisje i playlisty od publicznych', () => {
    expect(validateMeta({ ...meta, isLive: true, durationS: null }, DEFAULT_SETTINGS, false)).toMatch(/na żywo/);
    expect(validateMeta({ ...meta, isPlaylist: true }, DEFAULT_SETTINGS, false)).toMatch(/[Pp]laylist/);
    expect(validateMeta({ ...meta, isLive: true, durationS: null }, DEFAULT_SETTINGS, true)).toBeNull();
    expect(validateMeta({ ...meta, isPlaylist: true }, DEFAULT_SETTINGS, true)).toBeNull();
  });
});

describe('RateLimiter', () => {
  it('pozwala na limit zgłoszeń w oknie, potem blokuje, a po oknie znów pozwala', () => {
    let t = 0;
    const rl = new RateLimiter(3, 600_000, () => t);
    expect(rl.hit('ip')).toBe(true);
    expect(rl.hit('ip')).toBe(true);
    expect(rl.hit('ip')).toBe(true);
    expect(rl.hit('ip')).toBe(false);
    expect(rl.hit('inny')).toBe(true);
    t = 600_001;
    expect(rl.hit('ip')).toBe(true);
  });
  it('setLimits zmienia limit w locie', () => {
    const rl = new RateLimiter(1, 1000, () => 0);
    expect(rl.hit('a')).toBe(true);
    expect(rl.hit('a')).toBe(false);
    rl.setLimits(5, 1000);
    expect(rl.hit('a')).toBe(true);
  });
});
