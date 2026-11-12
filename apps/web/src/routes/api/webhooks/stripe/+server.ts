import { API_BASE_URL } from '$app/env/private';
import { relayWebhook } from '#lib/server/webhook-relay.ts';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = ({ request }) => relayWebhook(request, 'stripe', API_BASE_URL);
