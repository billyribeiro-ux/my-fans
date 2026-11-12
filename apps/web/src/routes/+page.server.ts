import { emptyFeed, isFeedResponse } from '#lib/contracts.ts';
import { loadResource } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';
export const load = (async (event) => {
  event.depends('app:feed');
  const params = new URLSearchParams();
  for (const key of ['q', 'category', 'cursor', 'tab', 'creator', 'post']) {
    const value = event.url.searchParams.get(key);
    if (value) params.set(key, value);
  }
  const { value, error } = await loadResource(event, `/v1/feed${params.size ? '?' + params : ''}`);
  return isFeedResponse(value)
    ? { feed: value, error: null }
    : { feed: emptyFeed, error: error ?? 'The creator feed is unavailable.' };
}) satisfies PageServerLoad;
