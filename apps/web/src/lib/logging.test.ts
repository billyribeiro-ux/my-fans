import { describe, expect, it } from 'vitest';
import { redactRequestLog } from './server/logging.ts';

describe('development access log privacy', () => {
  it('removes authentication and private search query values', () => {
    for (const path of [
      '/auth?verify=private-verification-token',
      '/auth?reset=private-reset-token',
      '/auth?%76erify=encoded-verification-token',
      '/?q=private-search&creator=private-creator',
    ]) {
      expect(redactRequestLog(`200 GET ${path}`)).toBe(`200 GET ${path.split('?')[0]}`);
    }
  });
  it('redacts colored error and data-request logs', () => {
    expect(redactRequestLog('\x1b[1;31m400\x1b[0m GET /auth\x1b[2m?reset=private\x1b[0m')).toBe(
      '400 GET /auth',
    );
    expect(
      redactRequestLog('200 GET /auth/__data.json?verify=private&x-sveltekit-invalidated=11'),
    ).toBe('200 GET /auth/__data.json');
  });
  it('preserves compiler diagnostics and request method/status/path', () => {
    const diagnostic = 'Build error: invalid TypeScript declaration';
    expect(redactRequestLog(diagnostic)).toBe(diagnostic);
    expect(redactRequestLog('403 POST /api/platform/checkout')).toBe(
      '403 POST /api/platform/checkout',
    );
  });
});
