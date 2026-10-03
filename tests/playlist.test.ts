import { describe, it, expect } from 'vitest';
import { buildLivePlaylist } from '../src/pipeline/playlist.ts';

describe('buildLivePlaylist', () => {
  it('buduje listę na żywo z numeracją i nieciągłościami', () => {
    const text = buildLivePlaylist({
      targetDuration: 4,
      discontinuitySequence: 2,
      segments: [
        { seq: 41, name: '41.ts', duration: 4.004, discontinuity: false },
        { seq: 42, name: '42.ts', duration: 3.5, discontinuity: true },
      ],
      pathPrefix: 'main/',
    });
    expect(text.split('\n')).toEqual([
      '#EXTM3U',
      '#EXT-X-VERSION:3',
      '#EXT-X-TARGETDURATION:4',
      '#EXT-X-MEDIA-SEQUENCE:41',
      '#EXT-X-DISCONTINUITY-SEQUENCE:2',
      '#EXTINF:4.004000,',
      'main/41.ts',
      '#EXT-X-DISCONTINUITY',
      '#EXTINF:3.500000,',
      'main/42.ts',
      '',
    ]);
  });

  it('zaokrągla target duration w górę do liczby całkowitej', () => {
    const text = buildLivePlaylist({ targetDuration: 6.2, discontinuitySequence: 0, segments: [], pathPrefix: '' });
    expect(text).toContain('#EXT-X-TARGETDURATION:7');
    expect(text).toContain('#EXT-X-MEDIA-SEQUENCE:0');
  });
});
