import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RequestEvent } from '@sveltejs/kit';

vi.mock('$app/server', () => ({}));
vi.mock('$app/env/private', () => ({ API_BASE_URL: 'http://backend.test' }));
import { apiRequest, loadDecodedResource, loadResource } from './api.ts';
import { parseNotificationsData } from '../route-data.ts';

function event(transport: typeof fetch, signal?: AbortSignal, token?: string): RequestEvent {
  return {
    request: new Request('https://web.test/settings', { signal }),
    fetch: transport,
    cookies: { get: () => token, delete: vi.fn() },
  } as unknown as RequestEvent;
}
afterEach(() => vi.restoreAllMocks());
describe('API request isolation and boundary', () => {
  it.each(['request', 'caller', 'timeout'] as const)(
    'preserves %s cancellation',
    async (source) => {
      const request = new AbortController(),
        caller = new AbortController(),
        timeout = new AbortController();
      vi.spyOn(AbortSignal, 'timeout').mockReturnValue(timeout.signal);
      const transport = vi.fn<typeof fetch>(async (_, init) => {
        const signal = init!.signal!;
        return new Promise<Response>((_, reject) => {
          signal.addEventListener('abort', () => reject(signal.reason), { once: true });
        });
      });
      const pending = apiRequest(event(transport, request.signal), '/v1/account', {
        signal: caller.signal,
      });
      const reason = new Error(`${source} canceled`);
      ({ request, caller, timeout })[source].abort(reason);
      await expect(pending).rejects.toBe(reason);
      expect(AbortSignal.timeout).toHaveBeenCalledWith(30000);
    },
  );
  it('keeps session credentials scoped to each request and preserves streamed responses', async () => {
    const controller = new AbortController();
    const transport = vi.fn<typeof fetch>(
      async (_, init) =>
        new Response(
          new ReadableStream({
            start(stream) {
              stream.enqueue(
                new TextEncoder().encode(
                  new Headers(init?.headers).get('authorization') ?? 'anonymous',
                ),
              );
              stream.close();
            },
          }),
        ),
    );
    const [first, second] = await Promise.all([
      apiRequest(event(transport, controller.signal, 'a'.repeat(64)), '/v1/account', {
        headers: { authorization: 'forged' },
      }),
      apiRequest(event(transport), '/v1/account'),
    ]);
    expect(await first.text()).toBe(`Bearer ${'a'.repeat(64)}`);
    expect(await second.text()).toBe('anonymous');
    expect(transport.mock.calls[0][1]?.signal).not.toBe(transport.mock.calls[1][1]?.signal);
  });
  it('returns checked DTOs and rejects a malformed successful upstream body', async () => {
    const body = {
      viewer_id: '10000000-0000-4000-8000-000000000001',
      notifications: [],
      next_cursor: null,
    };
    const valid = await loadDecodedResource(
      event(async () => Response.json(body)),
      '/v1/notifications',
      parseNotificationsData,
    );
    expect(valid).toEqual({ value: body, error: null });
    const invalid = await loadDecodedResource(
      event(async () => Response.json({ ...body, notifications: 'wrong' })),
      '/v1/notifications',
      parseNotificationsData,
    );
    expect(invalid.value).toBeNull();
    expect(invalid.error).toContain('invalid data');
  });
  it('handles nullable error payloads without leaking response details', async () => {
    expect(
      await loadResource(
        event(async () => Response.json(null, { status: 503 })),
        '/v1/account',
      ),
    ).toEqual({ value: null, error: 'This service is unavailable.' });
    const parse = vi.fn();
    expect(
      await loadDecodedResource(
        event(async () => Response.json({ message: 'Sign in required.' }, { status: 403 })),
        '/v1/account',
        parse,
      ),
    ).toEqual({ value: null, error: 'Sign in required.' });
    expect(parse).not.toHaveBeenCalled();
  });
});
