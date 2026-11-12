import { dev } from '$app/env';
import type { Handle, HandleServerError } from '@sveltejs/kit/hooks';

export const handle: Handle = async ({ event, resolve }) => {
  const response = await resolve(event);
  response.headers.set('cache-control', 'private, no-store');
  response.headers.set('x-content-type-options', 'nosniff');
  response.headers.set(
    'referrer-policy',
    event.url.pathname === '/auth' ? 'no-referrer' : 'strict-origin-when-cross-origin',
  );
  response.headers.set('x-frame-options', 'DENY');
  response.headers.set('permissions-policy', 'camera=(), microphone=(), geolocation=()');
  if (!dev && event.url.protocol === 'https:')
    response.headers.set('strict-transport-security', 'max-age=31536000');
  return response;
};

export const handleError: HandleServerError = ({ kind, error }) => {
  if (kind === 'app' || kind === 'framework') return error;
  const errorId = crypto.randomUUID();
  // Deliberately do not log request headers, cookies, or arbitrary thrown values.
  console.error(JSON.stringify({ event: 'unexpected_server_error', errorId }));
  return { message: `Something went wrong. Reference: ${errorId}` };
};
