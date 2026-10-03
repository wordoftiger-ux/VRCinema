import { describe, it, expect, beforeEach } from 'vitest';
import { Channel, type ClipFactory, type ClipHandle } from '../src/pipeline/channel.ts';
import { openDatabase, MAIN_CHANNEL_ID } from '../src/db/database.ts';
import { QueueRepo, type QueueItem } from '../src/db/queue.ts';
import { SettingsRepo } from '../src/db/settings.ts';
import type { SegmentInfo, SegmenterState } from '../src/pipeline/segmenter.ts';
import type { Slates } from '../src/pipeline/slates.ts';

const slates: Slates = {
  idle: [
    { path: '/slates/idle/0.ts', duration: 4, index: 0 },
    { path: '/slates/idle/1.ts', duration: 4, index: 1 },
  ],
  buffering: [{ path: '/slates/buf/0.ts', duration: 4, index: 0 }],
};

class FakeHandle implements ClipHandle {
  state: SegmenterState = 'running';
  error: string | null = null;
  durationS: number | null;
  killed = false;
  disposed = false;
  pending: SegmentInfo[] = [];
  private n = 0;
  item: QueueItem;
  startAtS: number;
  constructor(durationS: number | null, item: QueueItem, startAtS: number) {
    this.durationS = durationS;
    this.item = item;
    this.startAtS = startAtS;
  }
  produce(count: number, duration = 4) {
    for (let i = 0; i < count; i++) {
      this.pending.push({ path: `/clips/${this.item.id}/${this.startAtS}-${this.n}.ts`, duration, index: this.n++ });
    }
  }
  poll() {
    const out = this.pending;
    this.pending = [];
    return out;
  }
  kill() {
    this.killed = true;
    if (this.state === 'running') this.state = 'killed';
  }
  dispose() {
    this.kill();
    this.disposed = true;
  }
}

class FakeFactory implements ClipFactory {
  handles: FakeHandle[] = [];
  calls: { itemId: number; startAtS: number; attempt: number }[] = [];
  failNext = 0;
  manual = false;
  resolvers: (() => void)[] = [];
  async create(item: QueueItem, startAtS: number, attempt: number): Promise<ClipHandle> {
    this.calls.push({ itemId: item.id, startAtS, attempt });
    if (this.failNext > 0) {
      this.failNext--;
      throw new Error('nie da się rozpoznać');
    }
    if (this.manual) await new Promise<void>((r) => this.resolvers.push(r));
    const h = new FakeHandle(item.durationS, item, startAtS);
    this.handles.push(h);
    return h;
  }
}

const flush = () => new Promise((r) => setImmediate(r));

let queue: QueueRepo;
let settings: SettingsRepo;
let factory: FakeFactory;
let now: number;
let channel: Channel;

function addItem(title: string, durationS: number | null = 100, isLive = false) {
  return queue.add({
    channelId: MAIN_CHANNEL_ID, sourceUrl: `https://youtu.be/${title}`, title, durationS, isLive,
    status: 'queued', addedBy: 'admin', requesterName: 'Ola', requesterIp: null,
  });
}

beforeEach(() => {
  const db = openDatabase(':memory:');
  queue = new QueueRepo(db);
  settings = new SettingsRepo(db);
  factory = new FakeFactory();
  now = 0;
  channel = new Channel({
    channelId: MAIN_CHANNEL_ID, queue, settings, factory, slates, now: () => now, removeFile: () => {},
  });
  channel.start();
});

describe('Channel', () => {
  it('pusta kolejka: plansza oczekiwania w pętli', () => {
    const a = channel.next();
    const b = channel.next();
    const c = channel.next();
    expect([a.sourceKey, b.sourceKey, c.sourceKey]).toEqual(['idle', 'idle', 'idle']);
    expect([a.index, b.index, c.index]).toEqual([0, 1, 0]);
    expect(channel.state().status).toBe('idle');
  });

  it('nowy film: najpierw buforowanie, potem fragmenty klipu i status playing', async () => {
    const item = addItem('a');
    expect(channel.next().sourceKey).toBe('buffering');
    await flush();
    factory.handles[0].produce(2);
    const s1 = channel.next();
    expect(s1.sourceKey).toBe(`clip-${item.id}-0`);
    expect(s1.index).toBe(0);
    expect(channel.next().index).toBe(1);
    expect(queue.get(item.id)!.status).toBe('playing');
  });

  it('koniec filmu: status played i plansza oczekiwania', async () => {
    const item = addItem('a');
    channel.next();
    await flush();
    const h = factory.handles[0];
    h.produce(1);
    channel.next();
    h.state = 'done';
    expect(channel.next().sourceKey).toBe('idle');
    expect(queue.get(item.id)!.status).toBe('played');
    expect(h.disposed).toBe(true);
  });

  it('błąd ffmpeg: jedno wznowienie od miejsca przerwania, potem failed', async () => {
    const item = addItem('a');
    channel.next();
    await flush();
    factory.handles[0].produce(3);
    channel.next();
    channel.next();
    channel.next();
    factory.handles[0].state = 'failed';
    factory.handles[0].error = 'Connection reset';
    expect(channel.next().sourceKey).toBe('buffering');
    await flush();
    expect(factory.calls[1]).toEqual({ itemId: item.id, startAtS: 12, attempt: 1 });
    factory.handles[1].produce(1);
    expect(channel.next().sourceKey).toBe(`clip-${item.id}-1`);
    factory.handles[1].state = 'failed';
    factory.handles[1].error = 'Connection reset';
    expect(channel.next().sourceKey).toBe('idle');
    expect(queue.get(item.id)!.status).toBe('failed');
    expect(queue.get(item.id)!.error).toContain('Connection reset');
  });

  it('nieudane rozpoznanie dwa razy: failed i następny film', async () => {
    const a = addItem('a');
    const b = addItem('b');
    factory.failNext = 2;
    channel.next();
    await flush();
    channel.next();
    await flush();
    channel.next();
    await flush();
    expect(queue.get(a.id)!.status).toBe('failed');
    expect(queue.get(a.id)!.error).toContain('nie da się rozpoznać');
    expect(factory.calls.at(-1)!.itemId).toBe(b.id);
  });

  it('brak postępu przez 30 s: film pominięty jako failed', async () => {
    const item = addItem('a');
    channel.next();
    await flush();
    const h = factory.handles[0];
    h.produce(1);
    channel.next();
    expect(channel.next().sourceKey).toBe('buffering');
    now += 29_000;
    expect(channel.next().sourceKey).toBe('buffering');
    now += 2_000;
    expect(channel.next().sourceKey).toBe('idle');
    expect(queue.get(item.id)!.status).toBe('failed');
    expect(h.killed).toBe(true);
  });

  it('wyprzedzanie: następny film startuje 60 s przed końcem i gra bez buforowania', async () => {
    const a = addItem('a', 80);
    const b = addItem('b', 100);
    channel.next();
    await flush();
    const ha = factory.handles[0];
    ha.produce(20);
    for (let i = 0; i < 4; i++) channel.next(); // 16 s
    expect(factory.calls).toHaveLength(1);
    channel.next(); // 20 s, zostaje 60 s
    await flush();
    expect(factory.calls.map((c) => c.itemId)).toEqual([a.id, b.id]);
    factory.handles[1].produce(1);
    for (let i = 0; i < 15; i++) channel.next();
    ha.state = 'done';
    const s = channel.next();
    expect(s.sourceKey).toBe(`clip-${b.id}-0`);
    expect(queue.get(a.id)!.status).toBe('played');
    expect(queue.get(b.id)!.status).toBe('playing');
  });

  it('zmiana kolejności: wyprzedzony klip jest zabijany, gra nowa pierwsza pozycja', async () => {
    addItem('a', 40);
    const b = addItem('b');
    const c = addItem('c');
    channel.next();
    await flush();
    factory.handles[0].produce(10);
    channel.next();
    await flush();
    const prefetched = factory.handles[1];
    expect(prefetched.item.id).toBe(b.id);
    queue.move(c.id, 0);
    channel.onQueueChanged();
    expect(prefetched.disposed).toBe(true);
    for (let i = 0; i < 9; i++) channel.next();
    factory.handles[0].state = 'done';
    channel.next();
    await flush();
    expect(factory.calls.at(-1)!.itemId).toBe(c.id);
  });

  it('usunięcie bieżącego filmu przerywa go', async () => {
    const a = addItem('a');
    channel.next();
    await flush();
    factory.handles[0].produce(5);
    channel.next();
    queue.setStatus(a.id, 'removed');
    channel.onQueueChanged();
    expect(factory.handles[0].disposed).toBe(true);
    expect(channel.next().sourceKey).toBe('idle');
  });

  it('skip kończy bieżący film i przechodzi dalej', async () => {
    const a = addItem('a');
    const b = addItem('b');
    channel.next();
    await flush();
    factory.handles[0].produce(5);
    channel.next();
    channel.skip();
    expect(queue.get(a.id)!.status).toBe('played');
    expect(queue.get(a.id)!.error).toMatch(/[Pp]omini/);
    expect(channel.next().sourceKey).toBe('buffering');
    await flush();
    expect(factory.calls.at(-1)!.itemId).toBe(b.id);
  });

  it('transmisja na żywo nie uruchamia wyprzedzania', async () => {
    addItem('live', null, true);
    addItem('b');
    channel.next();
    await flush();
    factory.handles[0].produce(30);
    for (let i = 0; i < 30; i++) channel.next();
    await flush();
    expect(factory.calls).toHaveLength(1);
  });

  it('state() pokazuje film widziany przez widzów (z opóźnieniem 3 fragmentów)', async () => {
    const a = addItem('a', 100);
    const publish = () => {
      const s = channel.next();
      channel.onPublished({ ...s, seq: 0, channelTime: 0 });
    };
    publish(); // buforowanie
    await flush();
    factory.handles[0].produce(10);
    for (let i = 0; i < 5; i++) publish();
    const st = channel.state();
    expect(st.status).toBe('playing');
    expect(st.current).toMatchObject({ itemId: a.id, title: 'a', durationS: 100, requesterName: 'Ola' });
    expect(st.current!.positionS).toBe(8);
  });

  it('restart: przerwany film wraca na początek kolejki', () => {
    const a = addItem('a');
    addItem('b');
    queue.setStatus(a.id, 'playing');
    const c2 = new Channel({ channelId: MAIN_CHANNEL_ID, queue, settings, factory, slates, now: () => now, removeFile: () => {} });
    c2.start();
    expect(queue.nextQueued(MAIN_CHANNEL_ID)!.id).toBe(a.id);
  });
});
