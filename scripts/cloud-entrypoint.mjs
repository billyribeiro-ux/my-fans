import { spawn } from 'node:child_process';
import { chmod, chown, mkdir, open, readFile, stat } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = '/run/myfans';
const files = {
  CLOUD_STRIPE_SECRET: 'STRIPE_SECRET_KEY_FILE',
  CLOUD_STRIPE_WEBHOOK_SECRET: 'STRIPE_WEBHOOK_SECRET_FILE',
  CLOUD_SUMSUB_TOKEN: 'SUMSUB_APP_TOKEN_FILE',
  CLOUD_SUMSUB_SECRET: 'SUMSUB_SECRET_KEY_FILE',
  CLOUD_SUMSUB_WEBHOOK_SECRET: 'SUMSUB_WEBHOOK_SECRET_FILE',
  CLOUD_SMTP_PASSWORD: 'SMTP_PASSWORD_FILE',
};
function required(env, name, limit = 4096) {
  const value = env[name];
  if (
    typeof value !== 'string' ||
    !value ||
    Buffer.byteLength(value) > limit ||
    /[\x00-\x1f\x7f]/u.test(value)
  ) {
    throw new Error(`Invalid or missing ${name}`);
  }
  return value;
}
async function privateFile(directory, name, bytes) {
  const path = resolve(directory, name);
  // An existing file or symlink is never followed. A fresh task volume is required.
  const file = await open(path, 'wx', 0o600);
  try {
    await file.writeFile(bytes);
    await file.sync();
  } finally {
    await file.close();
  }
  return path;
}
export async function prepareEnvironment(source, mode, directory = ROOT) {
  if (
    ![
      'web',
      'worker',
      'migrate',
      'doctor',
      'verify-storage',
      'bootstrap-admin',
      'rebind-storage',
      'rotate-key',
      'privacy-purge',
      'privacy-retention',
      'privacy-export',
      'privacy-replay',
    ].includes(mode)
  )
    throw new Error('Invalid cloud command');
  const env = { ...source };
  const host = required(env, 'CLOUD_DATABASE_HOST');
  const database = required(env, 'CLOUD_DATABASE_NAME');
  const username = required(env, 'CLOUD_DATABASE_USERNAME');
  if (
    !/^[a-z0-9.-]+\.rds\.amazonaws\.com$/u.test(host) ||
    !/^[a-z][a-z0-9_]{0,62}$/u.test(database)
  )
    throw new Error('Invalid cloud database identity');
  const administration = [
    'migrate',
    'bootstrap-admin',
    'rebind-storage',
    'rotate-key',
    'privacy-purge',
    'privacy-retention',
    'privacy-export',
    'privacy-replay',
  ].includes(mode);
  if (username !== (administration ? 'myfans_owner' : 'myfans_app'))
    throw new Error('Cloud database role does not match command');
  const password = required(env, 'CLOUD_DATABASE_PASSWORD');
  const dsn = new URL(`postgresql://${host}:5432/${database}`);
  dsn.username = username;
  dsn.password = password;
  dsn.searchParams.set('sslmode', 'verify-full');
  dsn.searchParams.set('sslrootcert', '/app/certs/global-bundle.pem');
  env.DATABASE_URL_FILE = await privateFile(directory, 'database-url', dsn.toString());
  delete env.DATABASE_URL;
  if (mode === 'migrate') {
    env.DATABASE_RUNTIME_PASSWORD_FILE = await privateFile(
      directory,
      'runtime-password',
      required(env, 'CLOUD_RUNTIME_PASSWORD'),
    );
    env.APP_RUN_MIGRATIONS = 'true';
  } else if (mode === 'rotate-key') {
    for (const [input, output, name] of [
      ['CLOUD_MFA_KEY_HEX', 'MFA_ENCRYPTION_KEY_FILE', 'old-key'],
      ['CLOUD_NEXT_MFA_KEY_HEX', 'NEXT_MFA_ENCRYPTION_KEY_FILE', 'next-key'],
    ]) {
      const hex = required(env, input);
      if (!/^[0-9a-f]{64}$/u.test(hex)) throw new Error('Invalid rotation key encoding');
      env[output] = await privateFile(directory, name, Buffer.from(hex, 'hex'));
    }
    env.APP_RUN_MIGRATIONS = 'false';
  } else if (!administration) {
    for (const [input, output] of Object.entries(files)) {
      env[output] = await privateFile(directory, output.toLowerCase(), required(env, input));
    }
    const hex = required(env, 'CLOUD_MFA_KEY_HEX');
    if (!/^[0-9a-f]{64}$/u.test(hex))
      throw new Error('Encryption key must be 64 lowercase hexadecimal characters');
    env.MFA_ENCRYPTION_KEY_FILE = await privateFile(directory, 'mfa-key', Buffer.from(hex, 'hex'));
    const policy = env.CLOUD_OPERATOR_POLICY;
    if (typeof policy !== 'string' || Buffer.byteLength(policy) > 131072)
      throw new Error('Invalid operator policy document');
    try {
      JSON.parse(policy);
    } catch {
      throw new Error('Invalid operator policy JSON');
    }
    env.OPERATOR_POLICY_FILE = await privateFile(directory, 'operator-policy.json', policy);
    env.SMTP_USERNAME = required(env, 'CLOUD_SMTP_USERNAME', 512);
    env.APP_RUN_MIGRATIONS = 'false';
    env.APP_EMBEDDED_WORKER = 'false';
    env.WORKER_HEARTBEAT_FILE = `${directory}/worker-heartbeat`;
  }
  for (const key of Object.keys(env)) if (key.startsWith('CLOUD_')) delete env[key];
  return env;
}

async function prepareVolume() {
  if (process.getuid?.() !== 0)
    throw new Error('Volume preparation requires the isolated init container');
  for (const path of [ROOT, `${ROOT}/tmp`, `${ROOT}/media`]) {
    await mkdir(path, { recursive: true, mode: 0o700 });
    await chmod(path, 0o700);
    await chown(path, 1000, 1000);
  }
  // Scanner receives only its own signature/scratch volume, never application secrets.
  try {
    if ((await stat('/scan')).isDirectory()) {
      await chmod('/scan', 0o700);
      await chown('/scan', 1000, 1000);
    }
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
}
export function runChild(command, args, env) {
  return new Promise((accept, reject) => {
    const child = spawn(command, args, { env, stdio: 'inherit', cwd: '/app' });
    const forward = () => child.kill('SIGTERM');
    process.on('SIGTERM', forward);
    process.on('SIGINT', forward);
    child.once('error', () => reject(new Error('Cloud process could not start')));
    child.once('exit', (code) => {
      process.off('SIGTERM', forward);
      process.off('SIGINT', forward);
      code === 0 ? accept() : reject(new Error('Cloud process exited unsuccessfully'));
    });
  });
}
async function main() {
  const mode = process.argv[2];
  if (mode === 'prepare') return prepareVolume();
  const env = await prepareEnvironment(process.env, mode);
  for (const key of Object.keys(process.env)) if (key.startsWith('CLOUD_')) delete process.env[key];
  if (
    env.APP_MODE === 'production' &&
    ['web', 'worker', 'doctor', 'verify-storage'].includes(mode)
  ) {
    const origin = (await readFile('/app/web-origin', 'utf8')).trim();
    if (
      !origin.startsWith('https://') ||
      origin !== env.PUBLIC_ORIGIN ||
      origin !== env.MAIL_PUBLIC_ORIGIN
    )
      throw new Error('Compiled web origin differs from the configured production origin');
  }
  if (mode === 'web') return runChild(process.execPath, ['/app/scripts/serve.mjs'], env);
  if (mode === 'migrate') {
    await runChild('/usr/local/bin/myfans-api', ['migrate'], env);
    return runChild('/usr/local/bin/myfans-api', ['provision-runtime'], env);
  }
  return runChild('/usr/local/bin/myfans-api', [mode], env);
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(() => {
    // Do not expose parser details, environment values or credential-bearing URLs.
    console.error(
      'Cloud startup failed; inspect configuration names, role, file permissions and dependency health. Secret values were withheld.',
    );
    process.exitCode = 1;
  });
}
