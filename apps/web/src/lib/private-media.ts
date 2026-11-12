const uuid = '[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}';
const hlsPath = new RegExp(
  `^/api/platform/media/(${uuid})/hls/(${uuid})(?:\\.(720p|360p)(?:-[0-9]{5})?)?\\.(m3u8|ts)$`,
);

/** Accept only generated assets for the same private media item. */
export function privateHlsUrl(value: string, origin: string, expectedId?: string): URL {
  const url = new URL(value, origin);
  const match = hlsPath.exec(url.pathname);
  if (
    url.origin !== origin ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    !match ||
    match[1] !== match[2] ||
    (expectedId && match[1] !== expectedId) ||
    (match[4] === 'ts' && !/\.(720p|360p)-[0-9]{5}\.ts$/.test(url.pathname)) ||
    (match[4] === 'm3u8' && /-[0-9]{5}\.m3u8$/.test(url.pathname)) ||
    (match[4] === 'ts' && Number(url.pathname.match(/-([0-9]{5})\.ts$/)?.[1]) > 151)
  )
    throw new Error('Adaptive playback is unavailable.');
  return url;
}

export function privateHlsMediaId(value: string, origin: string): string {
  const url = privateHlsUrl(value, origin);
  if (!url.pathname.endsWith('.m3u8')) throw new Error('Adaptive playback is unavailable.');
  return hlsPath.exec(url.pathname)![1];
}
