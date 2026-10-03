import type { DatabaseSync } from 'node:sqlite';

export interface ChannelSettings {
  allowedDomains: string[];
  maxDurationS: number;
  requestLimitCount: number;
  requestLimitWindowS: number;
  maxActivePerRequester: number;
  moderation: boolean;
  forceTranscode: boolean;
  discontinuityTags: boolean;
}

export const DEFAULT_SETTINGS: ChannelSettings = {
  allowedDomains: ['youtube.com', 'youtu.be', 'pixeldrain.com', 'cda.pl'],
  maxDurationS: 7200,
  requestLimitCount: 3,
  requestLimitWindowS: 600,
  maxActivePerRequester: 5,
  moderation: false,
  forceTranscode: false,
  discontinuityTags: true,
};

/** Sprawdza i normalizuje częściowe ustawienia (np. z panelu). Rzuca błąd przy złych typach. */
export function sanitizeSettingsPatch(patch: Record<string, unknown>): Partial<ChannelSettings> {
  const out: Partial<ChannelSettings> = {};
  for (const [key, value] of Object.entries(patch)) {
    if (!(key in DEFAULT_SETTINGS)) throw new Error(`Nieznane ustawienie: ${key}`);
    const def = DEFAULT_SETTINGS[key as keyof ChannelSettings];
    if (Array.isArray(def)) {
      if (!Array.isArray(value) || !value.every((v) => typeof v === 'string')) throw new Error(`${key}: oczekiwano listy`);
      (out as Record<string, unknown>)[key] = value
        .map((v) => v.trim().toLowerCase().replace(/^https?:\/\//, '').replace(/^www\./, '').replace(/\/.*$/, ''))
        .filter(Boolean);
    } else if (typeof def === 'number') {
      if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) throw new Error(`${key}: oczekiwano liczby ≥ 0`);
      (out as Record<string, unknown>)[key] = value;
    } else {
      if (typeof value !== 'boolean') throw new Error(`${key}: oczekiwano true/false`);
      (out as Record<string, unknown>)[key] = value;
    }
  }
  return out;
}

export class SettingsRepo {
  private db: DatabaseSync;

  constructor(db: DatabaseSync) {
    this.db = db;
  }

  get(channelId: number): ChannelSettings {
    const rows = this.db.prepare('SELECT key, value FROM settings WHERE channel_id = ?').all(channelId) as {
      key: string;
      value: string;
    }[];
    const result: ChannelSettings = structuredClone(DEFAULT_SETTINGS);
    for (const { key, value } of rows) {
      if (key in result) (result as unknown as Record<string, unknown>)[key] = JSON.parse(value);
    }
    return result;
  }

  update(channelId: number, patch: Partial<ChannelSettings>): ChannelSettings {
    const stmt = this.db.prepare(
      'INSERT INTO settings (channel_id, key, value) VALUES (?, ?, ?) ON CONFLICT(channel_id, key) DO UPDATE SET value = excluded.value',
    );
    for (const [key, value] of Object.entries(patch)) stmt.run(channelId, key, JSON.stringify(value));
    return this.get(channelId);
  }
}
