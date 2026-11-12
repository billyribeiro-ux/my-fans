// Probe each listener directly, bypassing external URLs and reverse proxies.
// Never print response bodies or authentication data into Docker health logs.
try {
  const results = await Promise.all([
    fetch('http://127.0.0.1:8080/health/ready', { signal: AbortSignal.timeout(3000) }),
    fetch(`http://127.0.0.1:${process.env.PORT || '3000'}/auth`, {
      method: 'HEAD',
      redirect: 'manual',
      signal: AbortSignal.timeout(3000),
    }),
  ]);
  process.exitCode = results.every((response) => response.ok) ? 0 : 1;
} catch {
  process.exitCode = 1;
}
