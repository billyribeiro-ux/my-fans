import adapter from '@sveltejs/adapter-node';
import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { createLogger, defineConfig } from 'vite';
import { redactRequestLog } from './src/lib/server/logging.ts';

const logger = createLogger();
for (const method of ['info', 'warn', 'warnOnce', 'error'] as const) {
  const write = logger[method].bind(logger);
  logger[method] = (message, options) => write(redactRequestLog(message), options);
}

const appOrigin = new URL(process.env.APP_ORIGIN || 'http://127.0.0.1:5173');
if (
  !['http:', 'https:'].includes(appOrigin.protocol) ||
  appOrigin.username ||
  appOrigin.password ||
  appOrigin.pathname !== '/' ||
  appOrigin.search ||
  appOrigin.hash
)
  throw new Error('APP_ORIGIN must be an HTTP(S) origin without a path or credentials');

export default defineConfig({
  customLogger: logger,
  plugins: [
    tailwindcss(),
    sveltekit({
      adapter: adapter(),
      paths: { origin: appOrigin.origin },
      csrf: { trustedOrigins: [] },
      csp: {
        mode: 'auto',
        directives: {
          'default-src': ['self'],
          'script-src': ['self'],
          'style-src': ['self', 'unsafe-inline'],
          'font-src': ['self'],
          'img-src': ['self', 'https://images.unsplash.com', 'data:'],
          'connect-src': ['self'],
          'media-src': ['self', 'blob:'],
          'frame-ancestors': ['none'],
          'form-action': ['self'],
          'base-uri': ['self'],
          'object-src': ['none'],
        },
      },
    }),
  ],
});
