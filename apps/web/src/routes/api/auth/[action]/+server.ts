import {
  apiRequest,
  clearSession,
  failure,
  readBoundedBody,
  responseHeaders,
  setSession,
} from '#lib/server/api.ts';
import type { RequestHandler } from './$types';
export const POST: RequestHandler = async (event) => {
  const action = event.params.action;
  if (!['register', 'login', 'logout', 'recover', 'reset', 'verify'].includes(action))
    return failure(404, 'Endpoint not found.');
  if (event.request.headers.get('origin') !== event.url.origin)
    return failure(403, 'This request must come from MyFans.');
  if (event.request.headers.get('content-type')?.split(';')[0].trim() !== 'application/json')
    return failure(415, 'Send a JSON request.');
  const bytes = await readBoundedBody(event.request, 16384);
  if (bytes === null) return failure(413, 'The request is too large.');
  try {
    const response = await apiRequest(event, `/v1/auth/${action}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: bytes as BodyInit,
    });
    if (action === 'logout') {
      clearSession(event);
      if (!response.ok && response.status !== 401)
        return Response.json(
          { message: 'Signed out locally. The server could not confirm session revocation.' },
          { status: 503, headers: responseHeaders(response) },
        );
      return Response.json({ message: 'Signed out.' }, { headers: responseHeaders(response) });
    }
    const data = await response.json();
    if (response.ok && (action === 'register' || action === 'login')) {
      setSession(event, data.token);
      delete data.token;
    }
    return Response.json(data, {
      status: response.status,
      headers: responseHeaders(response),
    });
  } catch {
    if (action === 'logout') clearSession(event);
    return failure(503, 'The account service is unavailable. Please try again.');
  }
};
