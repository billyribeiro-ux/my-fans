import { parseAdminReviews } from '#lib/route-data.ts';
import { loadDecodedResource, resourcePath } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';
export const load = (async (event) => {
  event.depends('app:reviews');
  const { value, error } = await loadDecodedResource(
    event,
    resourcePath(event.url, '/v1/admin/reviews'),
    parseAdminReviews,
  );
  return { reviews: value, error };
}) satisfies PageServerLoad;
