export interface PlaylistSegment {
  seq: number;
  name: string;
  duration: number;
  /** Fragment zaczyna nowe źródło (inny film lub plansza). */
  discontinuity: boolean;
}

export interface LivePlaylistInput {
  segments: PlaylistSegment[];
  targetDuration: number;
  /** Liczba znaczników nieciągłości, które wypadły już z okna listy. */
  discontinuitySequence: number;
  /** Prefiks ścieżek fragmentów względem adresu listy, np. "main/". */
  pathPrefix: string;
}

export function buildLivePlaylist(input: LivePlaylistInput): string {
  const lines = [
    '#EXTM3U',
    '#EXT-X-VERSION:3',
    `#EXT-X-TARGETDURATION:${Math.ceil(input.targetDuration)}`,
    `#EXT-X-MEDIA-SEQUENCE:${input.segments[0]?.seq ?? 0}`,
    `#EXT-X-DISCONTINUITY-SEQUENCE:${input.discontinuitySequence}`,
  ];
  for (const s of input.segments) {
    if (s.discontinuity) lines.push('#EXT-X-DISCONTINUITY');
    lines.push(`#EXTINF:${s.duration.toFixed(6)},`, `${input.pathPrefix}${s.name}`);
  }
  return `${lines.join('\n')}\n`;
}
