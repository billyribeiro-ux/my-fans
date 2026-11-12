// Routes supply the private backend origin. This relay never reads sessions or uses
// SvelteKit's credential-inheriting event.fetch.
export type WebhookProvider = 'stripe' | 'sumsub';
const bodyLimit = 256 * 1024;
const signatureHeaders: Record<WebhookProvider, string[]> = {
  stripe: ['stripe-signature'],
  sumsub: ['x-payload-digest', 'x-payload-digest-alg'],
};
const failure = (status: number, message: string) =>
  Response.json({ message }, { status, headers: { 'cache-control': 'no-store' } });

async function boundedBytes(request: Request): Promise<Uint8Array | null> {
  if (Number(request.headers.get('content-length')) > bodyLimit) return null;
  if (!request.body) return new Uint8Array();
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > bodyLimit) {
        await reader.cancel();
        return null;
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

export async function relayWebhook(
  request: Request,
  provider: WebhookProvider,
  backendOrigin: string,
  transport: typeof fetch = globalThis.fetch,
): Promise<Response> {
  try {
    const bytes = await boundedBytes(request);
    if (bytes === null) return failure(413, 'The webhook payload is too large.');
    const target = new URL(`/v1/webhooks/${provider}`, backendOrigin);
    if (!['http:', 'https:'].includes(target.protocol) || target.username || target.password)
      return failure(503, 'Webhook delivery is unavailable.');
    const headers = new Headers({ 'content-type': 'application/json' });
    for (const key of signatureHeaders[provider]) {
      const value = request.headers.get(key);
      if (value) headers.set(key, value);
    }
    const response = await transport(target.href, {
      method: 'POST',
      headers,
      body: bytes as BodyInit,
      credentials: 'omit',
      redirect: 'error',
      cache: 'no-store',
      signal: AbortSignal.any([request.signal, AbortSignal.timeout(8000)]),
    });
    // Provider signatures and replay protection are verified by Rust. Do not
    // forward upstream cookies, redirects, debug headers, or error bodies.
    if (!response.ok) return failure(response.status, 'The webhook was not accepted.');
    return Response.json(
      { received: true },
      {
        status: response.status === 204 ? 200 : response.status,
        headers: { 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' },
      },
    );
  } catch {
    return failure(503, 'Webhook delivery is unavailable.');
  }
}
