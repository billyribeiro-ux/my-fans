import { parseStudioData } from '#lib/route-data.ts';
import { loadDecodedResource, resourcePath } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';
export const load = (async (event) => {
  event.depends('app:studio');
  const { value, error } = await loadDecodedResource(
    event,
    resourcePath(event.url, '/v1/studio'),
    parseStudioData,
  );
  return { studio: value, error };
}) satisfies PageServerLoad;
