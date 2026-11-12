import type { RequestEvent } from '@sveltejs/kit';
import { parseIntegrationStatus } from '../integrations.ts';
import { loadResource } from './api.ts';

export async function loadIntegrations(event: RequestEvent) {
  const { value, error } = await loadResource(event, '/v1/integrations/status');
  if (error) return { integrations: null, integrationError: error };
  try {
    return { integrations: parseIntegrationStatus(value), integrationError: null };
  } catch {
    return {
      integrations: null,
      integrationError: 'Provider status is unavailable. Please try again.',
    };
  }
}
