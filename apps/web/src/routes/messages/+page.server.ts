import { parseConversationsData } from '#lib/route-data.ts';
import { isFeedResponse } from '#lib/contracts.ts';
import { loadDecodedResource, loadResource, resourcePath } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';
export const load = (async (event) => {
  event.depends('app:messages');
  const [messages, feed] = await Promise.all([
    loadDecodedResource(
      event,
      resourcePath(event.url, '/v1/conversations'),
      parseConversationsData,
    ),
    loadResource(event, '/v1/feed'),
  ]);
  return {
    next_cursor: messages.value?.next_cursor ?? null,
    conversations: messages.value?.conversations ?? [],
    creators: isFeedResponse(feed.value) ? feed.value.creators : [],
    error: messages.error,
  };
}) satisfies PageServerLoad;
