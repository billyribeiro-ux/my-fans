import { performance } from 'node:perf_hooks';
const origin = new URL(process.env.LOAD_ORIGIN || 'http://127.0.0.1:3200');
if (!['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname))
  throw new Error('This benchmark is restricted to an isolated local deployment.');
const status = await fetch(new URL('/api/platform/integrations/status', origin)).then((r) =>
  r.json(),
);
if (status.mode !== 'sandbox') throw new Error('The benchmark requires sandbox mode.');
const login = await fetch(new URL('/api/auth/login', origin), {
  method: 'POST',
  headers: { origin: origin.origin, 'content-type': 'application/json' },
  body: JSON.stringify({ email: 'demo@myfans.test', password: 'DevelopmentOnly!2026' }),
});
if (!login.ok) throw new Error('Seeded sandbox account is unavailable.');
const cookie = login.headers
  .getSetCookie()
  .map((value) => value.split(';', 1)[0])
  .join('; ');
const paths = [
  '/',
  '/creators',
  '/api/platform/feed',
  '/api/platform/creators?q=art',
  '/api/platform/account',
  '/api/platform/notifications',
];
const samples = Object.fromEntries(paths.map((path) => [path, []]));
let failed = 0,
  completed = 0,
  responseBytes = 0;
const started = performance.now(),
  duration = 15000,
  concurrency = 16;
await Promise.all(
  Array.from({ length: concurrency }, async (_, worker) => {
    let index = worker;
    while (performance.now() - started < duration && completed < 4000) {
      const path = paths[index++ % paths.length],
        before = performance.now();
      try {
        const response = await fetch(new URL(path, origin), {
          headers: { cookie },
          signal: AbortSignal.timeout(5000),
        });
        if (!response.ok) failed++;
        responseBytes += (await response.arrayBuffer()).byteLength;
      } catch {
        failed++;
      }
      samples[path].push(performance.now() - before);
      completed++;
    }
  }),
);
const routes = Object.fromEntries(
  Object.entries(samples).map(([path, values]) => {
    values.sort((a, b) => a - b);
    return [
      path,
      {
        requests: values.length,
        p50_ms: Math.round(values[Math.floor(values.length * 0.5)] || 0),
        p95_ms: Math.round(values[Math.floor(values.length * 0.95)] || 0),
        p99_ms: Math.round(values[Math.floor(values.length * 0.99)] || 0),
      },
    ];
  }),
);
const result = {
  measured_at: new Date().toISOString(),
  concurrency,
  completed,
  failed,
  responseBytes,
  elapsed_ms: Math.round(performance.now() - started),
  routes,
  scope:
    'Local fixture workload; production capacity requires measurements in the selected account and region.',
};
console.log(JSON.stringify(result, null, 2));
if (failed || Object.values(routes).some((route) => route.p95_ms > 1500)) process.exitCode = 1;
