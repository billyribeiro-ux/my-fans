import { performance } from 'node:perf_hooks';
const origin = new URL(process.env.LOAD_ORIGIN || 'http://127.0.0.1:8080');
if (!['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname))
  throw new Error('This smoke test only targets local services.');
const samples = [];
let failed = 0;
await Promise.all(
  Array.from({ length: 8 }, async () => {
    for (let i = 0; i < 25; i++) {
      const start = performance.now();
      try {
        const response = await fetch(new URL('/v1/feed', origin), {
          signal: AbortSignal.timeout(5000),
        });
        if (!response.ok) failed++;
        await response.arrayBuffer();
      } catch {
        failed++;
      }
      samples.push(performance.now() - start);
    }
  }),
);
samples.sort((a, b) => a - b);
const result = {
  requests: samples.length,
  concurrency: 8,
  failed,
  p50_ms: Math.round(samples[Math.floor(samples.length * 0.5)]),
  p95_ms: Math.round(samples[Math.floor(samples.length * 0.95)]),
  p99_ms: Math.round(samples[Math.floor(samples.length * 0.99)]),
};
console.log(JSON.stringify(result, null, 2));
if (failed > 0 || result.p95_ms > 1000) process.exitCode = 1;
