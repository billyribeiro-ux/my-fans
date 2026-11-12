import { stat } from 'node:fs/promises';
try {
  const info = await stat('/run/myfans/worker-heartbeat');
  const age = Date.now() - info.mtimeMs;
  process.exitCode = info.isFile() && age >= -5000 && age < 180000 ? 0 : 1;
} catch {
  process.exitCode = 1;
}
