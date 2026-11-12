import { parseAccountData } from '#lib/route-data.ts';
import { loadDecodedResource } from '#lib/server/api.ts';
import type { LayoutServerLoad } from './$types';
import { loadIntegrations } from '#lib/server/integrations.ts';
import { loadPublicSite } from '#lib/server/public-site.ts';
export const load = (async (event) => {
  event.depends('app:account');
  event.setHeaders({ 'cache-control': 'private, no-store' });
  const [{ value }, integrations, site] = await Promise.all([
    loadDecodedResource(event, '/v1/account', parseAccountData),
    loadIntegrations(event),
    loadPublicSite(event),
  ]);
  return { user: value?.user ?? null, ...integrations, ...site };
}) satisfies LayoutServerLoad;
