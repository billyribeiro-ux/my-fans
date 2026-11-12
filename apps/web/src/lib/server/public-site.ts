import type { RequestEvent } from '@sveltejs/kit';
import { parsePublicSite } from '../public-site.ts';
import { loadResource } from './api.ts';

export async function loadPublicSite(event: RequestEvent) {
  const { value, error } = await loadResource(event, '/v1/site');
  if (error) return { site: null, siteError: error };
  try {
    return { site: parsePublicSite(value), siteError: null };
  } catch {
    return { site: null, siteError: 'Site information is unavailable. Please try again.' };
  }
}
