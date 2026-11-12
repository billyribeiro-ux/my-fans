import { describe, expect, it, vi } from 'vitest';
import { relayWebhook } from './webhook-relay.ts';

describe('signed webhook relay', () => {
  it.each(['stripe', 'sumsub'] as const)(
    'preserves %s raw bytes and only its signature headers',
    async (provider) => {
      const bytes = new TextEncoder().encode('{ "unicode": "é", "spacing": 1 }\n');
      const request = new Request('https://web.test/api/webhooks/' + provider, {
        method: 'POST',
        body: bytes,
        headers: {
          'stripe-signature': 't=1,v1=signature',
          'x-payload-digest': 'digest',
          'x-payload-digest-alg': 'HMAC_SHA256_HEX',
          cookie: 'myfans_session=private',
          authorization: 'Bearer private',
          origin: 'https://external-provider.test',
          'x-debug': 'private',
        },
      });
      const transport = vi.fn<typeof fetch>(
        async () =>
          new Response('private debug body', {
            status: 200,
            headers: { 'set-cookie': 'private=1', location: 'https://evil.test' },
          }),
      );
      const response = await relayWebhook(request, provider, 'http://backend.test', transport);
      expect(response.status).toBe(200);
      expect(transport.mock.calls[0][0]).toBe('http://backend.test/v1/webhooks/' + provider);
      const init = transport.mock.calls[0][1]!;
      expect(init.body).toEqual(bytes);
      expect(init.credentials).toBe('omit');
      expect(init.redirect).toBe('error');
      const headers = new Headers(init.headers);
      expect([...headers.keys()].sort()).toEqual(
        provider === 'stripe'
          ? ['content-type', 'stripe-signature']
          : ['content-type', 'x-payload-digest', 'x-payload-digest-alg'],
      );
      expect(response.headers.has('set-cookie')).toBe(false);
      expect(response.headers.has('location')).toBe(false);
      expect(await response.text()).not.toContain('private');
    },
  );
  it('bounds streamed bodies even when content length is omitted', async () => {
    const request = new Request('https://web.test/api/webhooks/stripe', {
      method: 'POST',
      body: new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array(256 * 1024));
          controller.enqueue(new Uint8Array(1));
          controller.close();
        },
      }),
      duplex: 'half',
    } as RequestInit);
    const transport = vi.fn<typeof fetch>();
    expect((await relayWebhook(request, 'stripe', 'http://backend.test', transport)).status).toBe(
      413,
    );
    expect(transport).not.toHaveBeenCalled();
  });
  it('accepts the exact limit and rejects oversized content length before forwarding', async () => {
    const transport = vi.fn<typeof fetch>(async () => new Response(null, { status: 204 }));
    const exact = new Request('https://web.test', {
      method: 'POST',
      body: new Uint8Array(256 * 1024),
    });
    expect((await relayWebhook(exact, 'stripe', 'http://backend.test', transport)).status).toBe(
      200,
    );
    const oversized = new Request('https://web.test', {
      method: 'POST',
      body: '{}',
      headers: { 'content-length': String(256 * 1024 + 1) },
    });
    expect((await relayWebhook(oversized, 'stripe', 'http://backend.test', transport)).status).toBe(
      413,
    );
    expect(transport).toHaveBeenCalledTimes(1);
  });
  it('preserves signature rejection without exposing upstream errors', async () => {
    const response = await relayWebhook(
      new Request('https://web.test', { method: 'POST', body: '{}' }),
      'stripe',
      'http://backend.test',
      async () => new Response('secret', { status: 400 }),
    );
    expect(response.status).toBe(400);
    expect(await response.text()).not.toContain('secret');
  });
  it('does not claim acceptance when transport fails', async () => {
    const response = await relayWebhook(
      new Request('https://web.test', { method: 'POST', body: '{}' }),
      'sumsub',
      'http://backend.test',
      async () => {
        throw new Error('private upstream URL');
      },
    );
    expect(response.status).toBe(503);
    expect(await response.text()).not.toContain('private');
  });
});
