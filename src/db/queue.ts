import type { DatabaseSync } from 'node:sqlite';

export type ItemStatus = 'pending_approval' | 'queued' | 'playing' | 'played' | 'failed' | 'removed' | 'rejected';
export type AddedBy = 'admin' | 'public';

export const ACTIVE_STATUSES: ItemStatus[] = ['pending_approval', 'queued', 'playing'];
const FINISHED_STATUSES: ItemStatus[] = ['played', 'failed'];

export interface QueueItem {
  id: number;
  channelId: number;
  position: number;
  sourceUrl: string;
  title: string;
  durationS: number | null;
  isLive: boolean;
  status: ItemStatus;
  addedBy: AddedBy;
  requesterName: string | null;
  requesterIp: string | null;
  error: string | null;
  createdAt: number;
  startedAt: number | null;
  finishedAt: number | null;
}

export interface NewItem {
  channelId: number;
  sourceUrl: string;
  title: string;
  durationS: number | null;
  isLive: boolean;
  status: ItemStatus;
  addedBy: AddedBy;
  requesterName: string | null;
  requesterIp: string | null;
}

type Row = Record<string, unknown>;

function toItem(r: Row): QueueItem {
  return {
    id: Number(r.id),
    channelId: Number(r.channel_id),
    position: Number(r.position),
    sourceUrl: String(r.source_url),
    title: String(r.title),
    durationS: r.duration_s === null ? null : Number(r.duration_s),
    isLive: Number(r.is_live) === 1,
    status: r.status as ItemStatus,
    addedBy: r.added_by as AddedBy,
    requesterName: (r.requester_name as string | null) ?? null,
    requesterIp: (r.requester_ip as string | null) ?? null,
    error: (r.error as string | null) ?? null,
    createdAt: Number(r.created_at),
    startedAt: r.started_at === null ? null : Number(r.started_at),
    finishedAt: r.finished_at === null ? null : Number(r.finished_at),
  };
}

function placeholders(n: number): string {
  return Array.from({ length: n }, () => '?').join(', ');
}

export class QueueRepo {
  private db: DatabaseSync;

  constructor(db: DatabaseSync) {
    this.db = db;
  }

  add(item: NewItem): QueueItem {
    const max = this.db
      .prepare('SELECT COALESCE(MAX(position), 0) AS m FROM queue_items WHERE channel_id = ?')
      .get(item.channelId) as Row;
    const res = this.db
      .prepare(
        `INSERT INTO queue_items (channel_id, position, source_url, title, duration_s, is_live, status,
           added_by, requester_name, requester_ip, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        item.channelId,
        Number(max.m) + 1,
        item.sourceUrl,
        item.title,
        item.durationS,
        item.isLive ? 1 : 0,
        item.status,
        item.addedBy,
        item.requesterName,
        item.requesterIp,
        Date.now(),
      );
    return this.get(Number(res.lastInsertRowid))!;
  }

  get(id: number): QueueItem | null {
    const r = this.db.prepare('SELECT * FROM queue_items WHERE id = ?').get(id) as Row | undefined;
    return r ? toItem(r) : null;
  }

  listByStatus(channelId: number, statuses: ItemStatus[]): QueueItem[] {
    const rows = this.db
      .prepare(
        `SELECT * FROM queue_items WHERE channel_id = ? AND status IN (${placeholders(statuses.length)})
         ORDER BY position, id`,
      )
      .all(channelId, ...statuses) as Row[];
    return rows.map(toItem);
  }

  nextQueued(channelId: number): QueueItem | null {
    return this.listByStatus(channelId, ['queued'])[0] ?? null;
  }

  setStatus(id: number, status: ItemStatus, error: string | null = null): void {
    const now = Date.now();
    const started = status === 'playing' ? now : null;
    const finished = FINISHED_STATUSES.includes(status) || status === 'removed' || status === 'rejected' ? now : null;
    this.db
      .prepare(
        `UPDATE queue_items SET status = ?, error = ?,
           started_at = COALESCE(?, started_at), finished_at = COALESCE(?, finished_at)
         WHERE id = ?`,
      )
      .run(status, error, started, finished, id);
  }

  /** Przenosi pozycję w obrębie aktywnej kolejki (queued) na indeks toIndex. */
  move(id: number, toIndex: number): void {
    const item = this.get(id);
    if (!item) return;
    const list = this.listByStatus(item.channelId, ['queued']).filter((i) => i.id !== id);
    const index = Math.max(0, Math.min(toIndex, list.length));
    list.splice(index, 0, item);
    this.renumber(list.map((i) => i.id));
  }

  /** Pozycja "playing" (przerwana restartem) wraca na początek kolejki. */
  resetPlaying(channelId: number): void {
    const playing = this.listByStatus(channelId, ['playing']);
    if (playing.length === 0) return;
    const queued = this.listByStatus(channelId, ['queued']);
    for (const p of playing) this.db.prepare("UPDATE queue_items SET status = 'queued' WHERE id = ?").run(p.id);
    this.renumber([...playing, ...queued].map((i) => i.id));
  }

  countActiveByIp(channelId: number, ip: string): number {
    const r = this.db
      .prepare(
        `SELECT COUNT(*) AS n FROM queue_items WHERE channel_id = ? AND requester_ip = ?
         AND status IN (${placeholders(ACTIVE_STATUSES.length)})`,
      )
      .get(channelId, ip, ...ACTIVE_STATUSES) as Row;
    return Number(r.n);
  }

  history(channelId: number, limit: number): QueueItem[] {
    const rows = this.db
      .prepare(
        `SELECT * FROM queue_items WHERE channel_id = ? AND status IN (${placeholders(FINISHED_STATUSES.length)})
         ORDER BY finished_at DESC, id DESC LIMIT ?`,
      )
      .all(channelId, ...FINISHED_STATUSES, limit) as Row[];
    return rows.map(toItem);
  }

  private renumber(ids: number[]): void {
    const stmt = this.db.prepare('UPDATE queue_items SET position = ? WHERE id = ?');
    // Nowe pozycje dostają MAX(position)+1, więc zawsze trafią za przenumerowaną listę.
    ids.forEach((id, i) => stmt.run(i + 1, id));
  }
}
