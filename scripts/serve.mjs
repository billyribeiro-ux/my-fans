import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
if (process.env.APP_MODE === 'production') {
  const compiledOrigin = (await readFile(`${root}web-origin`, 'utf8')).trim();
  if (
    compiledOrigin !== process.env.PUBLIC_ORIGIN ||
    compiledOrigin !== process.env.MAIL_PUBLIC_ORIGIN
  )
    throw new Error('The image must be rebuilt for the configured production origin.');
}
const children = [];
let stopping = false;
function stop(code = 0) {
  if (stopping) return;
  stopping = true;
  process.exitCode = code;
  for (const child of children) child.kill('SIGTERM');
  setTimeout(() => {
    for (const child of children) if (child.exitCode === null) child.kill('SIGKILL');
  }, 118000).unref();
}
for (const [command, args] of [
  [process.env.API_BINARY || `${root}target/release/myfans-api`, []],
  [process.execPath, [`${root}apps/web/build/index.js`]],
]) {
  const child = spawn(command, args, {
    cwd: root,
    stdio: 'inherit',
    env: {
      ...process.env,
      APP_MODE: process.env.APP_MODE || 'sandbox',
      HOST: process.env.HOST || '127.0.0.1',
      BODY_SIZE_LIMIT: process.env.BODY_SIZE_LIMIT || '12M',
    },
  });
  children.push(child);
  child.on('error', () => stop(1));
  child.on('exit', (code) => {
    if (!stopping) stop(code || 1);
  });
}
process.on('SIGINT', () => stop());
process.on('SIGTERM', () => stop());
