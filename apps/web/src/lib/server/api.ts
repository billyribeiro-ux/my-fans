import '$app/server';
import { API_BASE_URL } from '$app/env/private';
import type { RequestEvent } from '@sveltejs/kit';
import { privateHlsUrl } from '#lib/private-media.ts';
const cookieName = 'myfans_session';
const tokenPattern = /^[a-f0-9]{64}$/;
export function queryKeys(path: string): readonly string[] {
  const keys: Record<string, readonly string[]> = {
    '/v1/feed': ['q', 'category', 'cursor', 'tab', 'creator', 'post'],
    '/v1/creators': ['q', 'category', 'cursor'],
    '/v1/account': ['subscriptions_cursor', 'payments_cursor'],
    '/v1/studio': ['posts_cursor', 'media_cursor'],
    '/v1/earnings': ['transactions_cursor', 'payouts_cursor'],
    '/v1/account/safety': ['blocks_cursor', 'appeals_cursor', 'blocked_posts_cursor'],
    '/v1/admin/reviews': [
      'creators_cursor',
      'media_cursor',
      'reports_cursor',
      'payouts_cursor',
      'checkouts_cursor',
    ],
    '/v1/admin/operations': ['jobs_cursor', 'audit_cursor', 'users_cursor'],
    '/v1/admin/finance': [
      'accounts_cursor',
      'issues_cursor',
      'checkouts_cursor',
      'disputes_cursor',
      'payouts_cursor',
    ],
  };
  if (keys[path]) return keys[path];
  if (
    /^\/v1\/(notifications|conversations|account\/mail|admin\/appeals|admin\/privacy)$/.test(
      path,
    ) ||
    /^\/v1\/(posts\/[a-f0-9-]{36}\/comments|conversations\/[a-f0-9-]{36}\/messages)$/.test(path)
  )
    return ['cursor'];
  return [];
}
export function resourcePath(url: URL, path: string): string {
  const params = new URLSearchParams();
  for (const key of queryKeys(path)) {
    const value = url.searchParams.get(key);
    if (value !== null) params.set(key, value);
  }
  return `${path}${params.size ? '?' + params : ''}`;
}
export function getSession(event: Pick<RequestEvent, 'cookies'>): string | undefined {
  const token = event.cookies.get(cookieName);
  return token && tokenPattern.test(token) ? token : undefined;
}
export function setSession(event: RequestEvent, token: string): void {
  if (!tokenPattern.test(token)) throw new Error('Invalid session');
  event.cookies.set(cookieName, token, {
    path: '/',
    httpOnly: true,
    secure: event.url.protocol === 'https:',
    sameSite: 'lax',
    maxAge: 86400,
  });
}
export function clearSession(event: Pick<RequestEvent, 'cookies'>): void {
  event.cookies.delete(cookieName, { path: '/' });
}
export async function apiRequest(
  event: Pick<RequestEvent, 'fetch' | 'cookies' | 'request'>,
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const target = new URL(path, API_BASE_URL);
  let generatedHls = false;
  if (target.pathname.includes('/hls/')) {
    try {
      privateHlsUrl(target.pathname.replace(/^\/v1\//, '/api/platform/'), target.origin);
      generatedHls = true;
    } catch {
      /* Invalid generated asset paths are rejected below. */
    }
  }
  if (
    target.origin !== new URL(API_BASE_URL).origin ||
    (!/^\/v1\/[a-zA-Z0-9/_-]+$/.test(target.pathname) && !generatedHls) ||
    target.hash
  )
    throw new Error('Invalid API path');
  if (
    target.search &&
    [...target.searchParams.keys()].some((key) => !queryKeys(target.pathname).includes(key))
  )
    throw new Error('Invalid API query');
  const headers = new Headers(init.headers),
    token = getSession(event);
  headers.delete('authorization');
  if (token) headers.set('authorization', `Bearer ${token}`);
  const response = await event.fetch(target.href, {
    ...init,
    headers,
    signal: AbortSignal.any([
      event.request.signal,
      ...(init.signal ? [init.signal] : []),
      AbortSignal.timeout(30000),
    ]),
    redirect: 'error',
    cache: 'no-store',
  });
  if (response.status === 401) {
    clearSession(event);
    if (
      token &&
      ['/v1/feed', '/v1/account'].includes(target.pathname) &&
      (!init.method || init.method === 'GET')
    )
      return apiRequest(event, path, init);
  }
  return response;
}
export function responseHeaders(response: Response): Headers {
  const headers = new Headers({
    'cache-control': 'private, no-store',
    'x-content-type-options': 'nosniff',
  });
  const id = response.headers.get('x-request-id');
  if (id && /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(id))
    headers.set('x-request-id', id);
  return headers;
}
export function failure(status: number, message: string): Response {
  return Response.json({ message }, { status, headers: { 'cache-control': 'private, no-store' } });
}
export async function readBoundedBody(request: Request, limit: number): Promise<Uint8Array | null> {
  const length = request.headers.get('content-length');
  if (length && Number(length) > limit) return null;
  if (!request.body) return new Uint8Array();
  const reader = request.body.getReader(),
    chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      size += part.value.byteLength;
      if (size > limit) {
        await reader.cancel();
        return null;
      }
      chunks.push(part.value);
    }
  } finally {
    reader.releaseLock();
  }
  const result = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}
export async function loadResource(
  event: RequestEvent,
  path: string,
): Promise<{ value: unknown; error: string | null }> {
  try {
    const response = await apiRequest(event, path),
      value: unknown = await response.json();
    if (!response.ok)
      return {
        value: null,
        error:
          value !== null &&
          typeof value === 'object' &&
          'message' in value &&
          typeof value.message === 'string'
            ? value.message
            : 'This service is unavailable.',
      };
    return { value, error: null };
  } catch {
    return { value: null, error: 'This service is unavailable. Please try again.' };
  }
}

// A parser, rather than a generic assertion, establishes the serialized page contract.
export async function loadDecodedResource<T>(
  event: RequestEvent,
  path: string,
  parse: (value: unknown) => T,
): Promise<{ value: T | null; error: string | null }> {
  const result = await loadResource(event, path);
  if (result.error) return { value: null, error: result.error };
  try {
    return { value: parse(result.value), error: null };
  } catch {
    return { value: null, error: 'This service returned invalid data. Please try again.' };
  }
}
