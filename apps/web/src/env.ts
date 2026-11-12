import { defineEnvVars } from '@sveltejs/kit/env';

export const variables = defineEnvVars({
  API_BASE_URL: {
    description: 'Private Rust API origin. Never exposed to the browser.',
    schema: (value) => {
      const url = new URL(value || 'http://127.0.0.1:8080');
      if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password)
        throw new Error('API_BASE_URL must be an HTTP(S) origin without credentials');
      if (url.pathname !== '/' || url.search || url.hash)
        throw new Error('API_BASE_URL must contain an origin only');
      return url.origin;
    },
  },
});
