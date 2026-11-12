import { loadResource, resourcePath } from '#lib/server/api.ts';
import { parseCreatorDirectory } from '#lib/public-site.ts';
import type { PageServerLoad } from './$types';

export const load = (async (event) => {
  event.depends('app:creators');
  const { value, error } = await loadResource(event, resourcePath(event.url, '/v1/creators'));
  try {
    return { directory: parseCreatorDirectory(value), error: null };
  } catch {
    return {
      directory: null,
      error: error ?? 'The creator directory is unavailable. Please try again.',
    };
  }
}) satisfies PageServerLoad;
