import { DatabaseSync } from 'node:sqlite';
import fs from 'node:fs';
import path from 'node:path';

export const MAIN_CHANNEL_ID = 1;
export const MAIN_CHANNEL_SLUG = 'main';

const SCHEMA = `
CREATE TABLE IF NOT EXISTS channels (
  id INTEGER PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
  channel_id INTEGER NOT NULL REFERENCES channels(id),
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (channel_id, key)
);
CREATE TABLE IF NOT EXISTS queue_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_id INTEGER NOT NULL REFERENCES channels(id),
  position INTEGER NOT NULL,
  source_url TEXT NOT NULL,
  title TEXT NOT NULL,
  duration_s REAL,
  is_live INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,
  added_by TEXT NOT NULL,
  requester_name TEXT,
  requester_ip TEXT,
  error TEXT,
  created_at INTEGER NOT NULL,
  started_at INTEGER,
  finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS queue_items_channel_status ON queue_items(channel_id, status, position);
`;

/** Otwiera bazę (":memory:" w testach), tworzy schemat i kanał "main". */
export function openDatabase(file: string): DatabaseSync {
  if (file !== ':memory:') fs.mkdirSync(path.dirname(file), { recursive: true });
  const db = new DatabaseSync(file);
  db.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;');
  db.exec(SCHEMA);
  db.prepare('INSERT OR IGNORE INTO channels (id, slug, name, created_at) VALUES (?, ?, ?, ?)').run(
    MAIN_CHANNEL_ID,
    MAIN_CHANNEL_SLUG,
    'Kanał główny',
    Date.now(),
  );
  return db;
}
