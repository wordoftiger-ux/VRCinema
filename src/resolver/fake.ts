import fs from 'node:fs';
import type { MediaMeta, MediaResolver, ResolvedStream, StreamInput } from './types.ts';

/**
 * Resolver testowy (VRCINEMA_FAKE_RESOLVER=<plik.json>), ten sam format co FakeResolver w silniku Rust:
 * URL -> {title, durationS, isLive, isPlaylist, entries?, inputs?} albo {error}.
 */
export class FakeResolver implements MediaResolver {
  private map: Record<string, Record<string, unknown>>;

  constructor(file: string) {
    this.map = JSON.parse(fs.readFileSync(file, 'utf8').replace(/^﻿/, ''));
  }

  private entry(url: string): Record<string, unknown> {
    const e = Object.hasOwn(this.map, url) ? this.map[url] : undefined;
    if (!e) throw new Error('Unsupported URL');
    if (typeof e.error === 'string') throw new Error(e.error);
    return e;
  }

  async fetchMeta(url: string): Promise<MediaMeta> {
    const e = this.entry(url);
    const num = (v: unknown) => (typeof v === 'number' ? v : null);
    const entries = Array.isArray(e.entries)
      ? e.entries.map((x: Record<string, unknown>) => ({
          url: typeof x.url === 'string' ? x.url : '',
          title: typeof x.title === 'string' ? x.title : '',
          durationS: num(x.durationS),
        }))
      : undefined;
    return {
      title: typeof e.title === 'string' ? e.title : '',
      durationS: num(e.durationS),
      isLive: e.isLive === true,
      isPlaylist: e.isPlaylist === true,
      ...(entries ? { entries } : {}),
    };
  }

  async resolve(url: string): Promise<ResolvedStream> {
    const e = this.entry(url);
    if (!Array.isArray(e.inputs)) throw new Error('fałszywy resolver nie odtwarza');
    const meta = await this.fetchMeta(url);
    return {
      inputs: e.inputs.map((i: { url?: string; headers?: Record<string, string> }): StreamInput => ({
        url: i.url ?? '',
        headers: i.headers ?? {},
      })),
      title: meta.title,
      durationS: meta.durationS,
      isLive: meta.isLive,
    };
  }
}
