import { describe, it, expect, beforeEach } from 'vitest';
import { openDatabase, MAIN_CHANNEL_ID } from '../src/db/database.ts';
import { QueueRepo } from '../src/db/queue.ts';
import { SettingsRepo, DEFAULT_SETTINGS } from '../src/db/settings.ts';

function item(title: string, extra: Partial<Parameters<QueueRepo['add']>[0]> = {}) {
  return {
    channelId: MAIN_CHANNEL_ID,
    sourceUrl: `https://youtu.be/${title}`,
    title,
    durationS: 60,
    isLive: false,
    status: 'queued' as const,
    addedBy: 'admin' as const,
    requesterName: null,
    requesterIp: null,
    ...extra,
  };
}

describe('QueueRepo', () => {
  let q: QueueRepo;
  beforeEach(() => {
    q = new QueueRepo(openDatabase(':memory:'));
  });

  it('dodaje na koniec kolejki i zwraca w kolejności', () => {
    q.add(item('a'));
    q.add(item('b'));
    q.add(item('c'));
    expect(q.listByStatus(MAIN_CHANNEL_ID, ['queued']).map((i) => i.title)).toEqual(['a', 'b', 'c']);
    expect(q.nextQueued(MAIN_CHANNEL_ID)?.title).toBe('a');
  });

  it('przechowuje polskie znaki i pola', () => {
    const added = q.add(item('Żółć', { requesterName: 'Łukasz', requesterIp: '1.2.3.4', addedBy: 'public' }));
    const got = q.get(added.id)!;
    expect(got.title).toBe('Żółć');
    expect(got.requesterName).toBe('Łukasz');
    expect(got.addedBy).toBe('public');
    expect(got.isLive).toBe(false);
  });

  it('move przenosi pozycję na wskazany indeks', () => {
    const a = q.add(item('a'));
    q.add(item('b'));
    const c = q.add(item('c'));
    q.move(c.id, 0);
    expect(q.listByStatus(MAIN_CHANNEL_ID, ['queued']).map((i) => i.title)).toEqual(['c', 'a', 'b']);
    q.move(a.id, 2);
    expect(q.listByStatus(MAIN_CHANNEL_ID, ['queued']).map((i) => i.title)).toEqual(['c', 'b', 'a']);
  });

  it('setStatus zapisuje błąd i czasy', () => {
    const a = q.add(item('a'));
    q.setStatus(a.id, 'playing');
    expect(q.get(a.id)!.startedAt).not.toBeNull();
    q.setStatus(a.id, 'failed', 'zepsute');
    const got = q.get(a.id)!;
    expect(got.status).toBe('failed');
    expect(got.error).toBe('zepsute');
    expect(got.finishedAt).not.toBeNull();
    expect(q.nextQueued(MAIN_CHANNEL_ID)).toBeNull();
  });

  it('resetPlaying wstawia przerwany film na początek kolejki', () => {
    q.add(item('a'));
    const b = q.add(item('b'));
    q.setStatus(b.id, 'playing');
    q.resetPlaying(MAIN_CHANNEL_ID);
    expect(q.listByStatus(MAIN_CHANNEL_ID, ['queued']).map((i) => i.title)).toEqual(['b', 'a']);
  });

  it('countActiveByIp liczy oczekujące i zakolejkowane', () => {
    q.add(item('a', { requesterIp: '1.1.1.1' }));
    q.add(item('b', { requesterIp: '1.1.1.1', status: 'pending_approval' }));
    const c = q.add(item('c', { requesterIp: '1.1.1.1' }));
    q.setStatus(c.id, 'played');
    q.add(item('d', { requesterIp: '2.2.2.2' }));
    expect(q.countActiveByIp(MAIN_CHANNEL_ID, '1.1.1.1')).toBe(2);
  });

  it('history zwraca zakończone od najnowszych', () => {
    const a = q.add(item('a'));
    const b = q.add(item('b'));
    q.setStatus(a.id, 'played');
    q.setStatus(b.id, 'failed', 'x');
    const h = q.history(MAIN_CHANNEL_ID, 50);
    expect(h.map((i) => i.title)).toEqual(['b', 'a']);
  });
});

describe('SettingsRepo', () => {
  it('zwraca domyślne i scala zmiany', () => {
    const s = new SettingsRepo(openDatabase(':memory:'));
    expect(s.get(MAIN_CHANNEL_ID)).toEqual(DEFAULT_SETTINGS);
    s.update(MAIN_CHANNEL_ID, { moderation: true, maxDurationS: 600 });
    const got = s.get(MAIN_CHANNEL_ID);
    expect(got.moderation).toBe(true);
    expect(got.maxDurationS).toBe(600);
    expect(got.allowedDomains).toEqual(DEFAULT_SETTINGS.allowedDomains);
  });

  it('domyślne wartości zgodne ze specyfikacją', () => {
    expect(DEFAULT_SETTINGS).toEqual({
      allowedDomains: ['youtube.com', 'youtu.be', 'pixeldrain.com', 'cda.pl'],
      maxDurationS: 7200,
      requestLimitCount: 3,
      requestLimitWindowS: 600,
      maxActivePerRequester: 5,
      moderation: false,
      forceTranscode: false,
      discontinuityTags: true,
    });
  });
});
