import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const root = fileURLToPath(new URL('../', import.meta.url));
const grouped = process.platform !== 'win32';
const children = [];
let stopping = false;

function stop(code = 0) {
  if (stopping) return;
  stopping = true;
  process.exitCode = code;
  for (const child of children) {
    if (!child.pid) continue;
    try {
      if (grouped) process.kill(-child.pid, 'SIGTERM');
      else child.kill('SIGTERM');
    } catch (error) {
      if (error.code !== 'ESRCH') console.error(error.message);
    }
  }
  setTimeout(() => {
    for (const child of children) {
      if (child.exitCode !== null || !child.pid) continue;
      try {
        if (grouped) process.kill(-child.pid, 'SIGKILL');
        else child.kill('SIGKILL');
      } catch {}
    }
  }, 150000).unref();
}

function start(command, args, cwd) {
  const child = spawn(command, args, {
    cwd,
    stdio: 'inherit',
    detached: grouped,
    env: { ...process.env, APP_MODE: process.env.APP_MODE || 'sandbox' },
  });
  children.push(child);
  child.on('error', (error) => {
    console.error(`Could not start ${command}: ${error.message}`);
    stop(1);
  });
  child.on('exit', (code, signal) => {
    if (!stopping) {
      console.error(`${command} stopped (${signal || code}). Stopping the development stack.`);
      stop(code || 1);
    }
  });
}

process.on('SIGINT', () => stop());
process.on('SIGTERM', () => stop());
console.log('MyFans sandbox: http://127.0.0.1:5173 · Rust API: http://127.0.0.1:8080');
start('cargo', ['run', '--locked', '-p', 'myfans-api'], root);
start(
  resolve(root, 'apps/web/node_modules/.bin/vite'),
  ['dev', '--host', '127.0.0.1', '--port', '5173', '--strictPort'],
  resolve(root, 'apps/web'),
);
