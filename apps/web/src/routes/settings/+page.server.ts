import { parseAccountData } from '#lib/route-data.ts';
import { loadDecodedResource, resourcePath } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';
export const load = (async (event) => {
  event.depends('app:account');
  const { value, error } = await loadDecodedResource(
    event,
    resourcePath(event.url, '/v1/account'),
    parseAccountData,
  );
  return { account: value, error };
}) satisfies PageServerLoad;
