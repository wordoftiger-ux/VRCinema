export interface PlaylistEntry {
  url: string;
  title: string;
  durationS: number | null;
}

/** Metadane pobierane przy dodawaniu do kolejki (bez adresów strumieni, które szybko wygasają). */
export interface MediaMeta {
  title: string;
  durationS: number | null;
  isLive: boolean;
  isPlaylist: boolean;
  entries?: PlaylistEntry[];
}

export interface StreamInput {
  url: string;
  headers: Record<string, string>;
}

/** Bezpośrednie adresy strumieni gotowe dla ffmpeg. inputs[0] zawiera wideo, opcjonalnie inputs[1] dźwięk. */
export interface ResolvedStream {
  inputs: StreamInput[];
  title: string;
  durationS: number | null;
  isLive: boolean;
}

export interface MediaResolver {
  fetchMeta(url: string): Promise<MediaMeta>;
  resolve(url: string): Promise<ResolvedStream>;
}
