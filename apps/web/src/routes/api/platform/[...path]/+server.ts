import {
  apiRequest,
  clearSession,
  failure,
  readBoundedBody,
  responseHeaders,
  resourcePath,
} from '#lib/server/api.ts';
import { privateHlsUrl } from '#lib/private-media.ts';
import type { RequestHandler } from './$types';
const routes: Record<string, RegExp[]> = {
  GET: [
    /^(privacy\/erasure|admin\/privacy)$/,
    /^admin\/finance$/,
    /^admin\/posts\/[a-f0-9-]{36}$/,
    /^(site|creators)$/,
    /^integrations\/status$/,
    /^(feed|account|account\/mail|account\/safety|admin\/appeals|account\/export|studio|earnings|conversations|notifications|admin\/reviews|admin\/operations)$/,
    /^posts\/[a-f0-9-]{36}\/comments$/,
    /^conversations\/[a-f0-9-]{36}\/messages$/,
    /^media\/[a-f0-9-]{36}\/(full|preview|captions)$/,
    /^media\/uploads\/[a-f0-9-]{36}$/,
    /^media\/[a-f0-9-]{36}\/hls\/[a-f0-9.-]+$/,
  ],
  POST: [
    /^privacy\/erasure$/,
    /^admin\/privacy\/[a-f0-9-]{36}\/(review|complete)$/,
    /^media\/uploads(?:\/[a-f0-9-]{36}\/complete)?$/,
    /^admin\/finance\/(checkout|dispute|issues\/acknowledge)$/,
    /^creators\/(payout-account|payout-dashboard|verification|resubmit)$/,
    /^account\/verification\/resend$/,
    /^appeals$/,
    /^admin\/appeals\/[a-f0-9-]{36}$/,
    /^admin\/jobs\/[a-f0-9-]{36}\/retry$/,
    /^admin\/accounts\/[a-f0-9-]{36}$/,
    /^admin\/posts\/[a-f0-9-]{36}\/restore$/,
    /^media\/[a-f0-9-]{36}\/captions$/,
    /^(creators\/profile|media\/upload|posts|checkout|payouts|conversations|reports|account\/(password|mfa|deactivate))$/,
    /^checkout\/[a-f0-9-]{36}\/confirm$/,
    /^posts\/[a-f0-9-]{36}\/comments$/,
    /^conversations\/[a-f0-9-]{36}\/messages$/,
    /^admin\/(reviews|refunds)\/[a-f0-9-]{36}$/,
  ],
  PATCH: [
    /^media\/uploads\/[a-f0-9-]{36}$/,
    /^account$/,
    /^posts\/[a-f0-9-]{36}$/,
    /^notifications\/[a-f0-9-]{36}\/read$/,
  ],
  PUT: [/^users\/[a-f0-9-]{36}\/block$/, /^posts\/[a-f0-9-]{36}\/(like|save)$/],
  DELETE: [
    /^media\/uploads\/[a-f0-9-]{36}$/,
    /^users\/[a-f0-9-]{36}\/block$/,
    /^posts\/[a-f0-9-]{36}\/(like|save)$/,
    /^subscriptions\/[a-f0-9-]{36}$/,
  ],
};
const handle: RequestHandler = async (event) => {
  const method = event.request.method,
    path = event.params.path;
  if (!path || !routes[method]?.some((pattern) => pattern.test(path)))
    return failure(404, 'Endpoint not found.');
  if (path.includes('/hls/')) {
    try {
      privateHlsUrl(`/api/platform/${path}`, event.url.origin);
    } catch {
      return failure(404, 'Endpoint not found.');
    }
  }
  const headers = new Headers();
  let body: Uint8Array | undefined;
  if (path.startsWith('media/') && event.request.headers.has('range'))
    headers.set('range', event.request.headers.get('range')!);
  if (method !== 'GET') {
    if (event.request.headers.get('origin') !== event.url.origin)
      return failure(403, 'This request must come from MyFans.');
    const contentType = event.request.headers.get('content-type') || '',
      upload = path === 'media/upload',
      chunk = method === 'PATCH' && /^media\/uploads\/[a-f0-9-]{36}$/.test(path);
    if (
      upload
        ? !contentType.startsWith('multipart/form-data;')
        : contentType.split(';')[0].trim() !==
          (chunk ? 'application/octet-stream' : 'application/json')
    )
      return failure(415, upload || chunk ? 'Send a file upload.' : 'Send a JSON request.');
    if (chunk) {
      const offset = event.request.headers.get('upload-offset');
      if (!offset || !/^(0|[1-9][0-9]{0,8})$/.test(offset) || Number(offset) > 256 * 1024 * 1024)
        return failure(400, 'Send a valid upload offset.');
      headers.set('upload-offset', offset);
    }
    const bytes = await readBoundedBody(
      event.request,
      chunk ? 4 * 1024 * 1024 : upload ? 12 * 1024 * 1024 : 16384,
    );
    if (bytes === null) return failure(413, 'The request is too large.');
    if (chunk && bytes.byteLength === 0) return failure(400, 'Send a nonempty upload chunk.');
    body = bytes;
    headers.set('content-type', contentType);
  }
  try {
    const response = await apiRequest(
      event,
      method === 'GET' ? resourcePath(event.url, `/v1/${path}`) : `/v1/${path}`,
      {
        method,
        headers,
        body: body as BodyInit | undefined,
      },
    );
    if ((path === 'account/deactivate' || path === 'privacy/erasure') && response.ok)
      clearSession(event);
    const outgoing = responseHeaders(response);
    outgoing.set('content-type', response.headers.get('content-type') || 'application/json');
    if (path.startsWith('media/')) {
      outgoing.set('content-disposition', 'inline');
      for (const key of ['content-range', 'accept-ranges', 'content-length']) {
        const v = response.headers.get(key);
        if (v) outgoing.set(key, v);
      }
    }
    return new Response(response.body, { status: response.status, headers: outgoing });
  } catch {
    return failure(503, 'This service is unavailable. Please try again.');
  }
};
export const GET = handle,
  POST = handle,
  PATCH = handle,
  PUT = handle,
  DELETE = handle;
