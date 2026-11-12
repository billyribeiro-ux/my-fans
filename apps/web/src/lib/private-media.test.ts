import { describe, expect, it } from 'vitest';
import { privateHlsUrl, privateHlsMediaId } from './private-media.ts';
const id = 'aabbccdd-1234-5678-9000-aabbccddeeff';
const origin = 'https://myfans.example';
const path = `/api/platform/media/${id}/hls/${id}`;
describe('private adaptive asset boundary', () => {
  it('allows generated same-item playlists and bounded segments', () => {
    expect(privateHlsMediaId(`${path}.m3u8`, origin)).toBe(id);
    for (const suffix of ['.720p.m3u8', '.360p.m3u8', '.720p-00151.ts', '.360p-00000.ts'])
      expect(privateHlsUrl(path + suffix, origin, id).origin).toBe(origin);
  });
  it('rejects other origins, queries, redirects to other media and unexpected assets', () => {
    for (const value of [
      `https://other.example${path}.m3u8`,
      `${path}.m3u8?token=secret`,
      `${path}.m3u8#x`,
      path.replace(/aabbccddeeff$/, 'aabbccddee00') + '.m3u8',
      `${path}.720p-00152.ts`,
      `${path}.1080p.m3u8`,
      `${path}.ts`,
      `${path}.720p-00001.m3u8`,
      `${path}/../../full`,
      `https://user:password@myfans.example${path}.m3u8`,
    ])
      expect(() => privateHlsUrl(value, origin, id)).toThrow();
  });
});
