import { parseNotificationsData } from '#lib/route-data.ts';
import { loadDecodedResource, resourcePath } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';
export const load = (async (event) => {
  event.depends('app:notifications');
  const { value, error } = await loadDecodedResource(
    event,
    resourcePath(event.url, '/v1/notifications'),
    parseNotificationsData,
  );
  return {
    next_cursor: value?.next_cursor ?? null,
    notifications: value?.notifications ?? [],
    error,
  };
}) satisfies PageServerLoad;
