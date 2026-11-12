import { parseEarningsData } from '#lib/route-data.ts';
import { loadDecodedResource, resourcePath } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';
export const load = (async (event) => {
  event.depends('app:earnings');
  const { value, error } = await loadDecodedResource(
    event,
    resourcePath(event.url, '/v1/earnings'),
    parseEarningsData,
  );
  return { earnings: value, error };
}) satisfies PageServerLoad;
