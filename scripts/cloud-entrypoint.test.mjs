import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { prepareEnvironment } from './cloud-entrypoint.mjs';

const config = {
  CLOUD_DATABASE_HOST: 'example.abcdef.us-east-1.rds.amazonaws.com',
  CLOUD_DATABASE_NAME: 'myfans',
  CLOUD_DATABASE_USERNAME: 'myfans_app',
  CLOUD_DATABASE_PASSWORD: 'never-expose:/?#@test',
  CLOUD_MFA_KEY_HEX: 'ab'.repeat(32),
  CLOUD_OPERATOR_POLICY: JSON.stringify({ operator: { legal_name: 'Fixture' } }),
  CLOUD_SMTP_USERNAME: 'smtp-fixture',
  CLOUD_SMTP_PASSWORD: 'fixture-password',
  CLOUD_STRIPE_SECRET: 'sk_live_fixture',
  CLOUD_STRIPE_WEBHOOK_SECRET: 'whsec_fixture',
  CLOUD_SUMSUB_TOKEN: 'fixture-token',
  CLOUD_SUMSUB_SECRET: 'fixture-secret',
  CLOUD_SUMSUB_WEBHOOK_SECRET: 'fixture-webhook',
  APP_MODE: 'production',
  DATABASE_URL: 'ignored-unsafe-url',
  PUBLIC_ORIGIN: 'https://example.com',
};
test('cloud bootstrap encodes DSN, creates private files and scrubs injected values', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'myfans-cloud-'));
  try {
    const env = await prepareEnvironment(config, 'worker', directory);
    assert.equal(
      Object.keys(env).some((key) => key.startsWith('CLOUD_')),
      false,
    );
    assert.equal(env.DATABASE_URL, undefined);
    assert.equal(env.APP_RUN_MIGRATIONS, 'false');
    assert.equal(env.PUBLIC_ORIGIN, config.PUBLIC_ORIGIN);
    const dsn = new URL(await readFile(env.DATABASE_URL_FILE, 'utf8'));
    assert.equal(decodeURIComponent(dsn.password), config.CLOUD_DATABASE_PASSWORD);
    assert.equal(dsn.searchParams.get('sslmode'), 'verify-full');
    assert.equal(dsn.searchParams.get('sslrootcert'), '/app/certs/global-bundle.pem');
    assert.deepEqual(await readFile(env.MFA_ENCRYPTION_KEY_FILE), Buffer.alloc(32, 171));
    for (const path of Object.entries(env)
      .filter(([key]) => key.endsWith('_FILE') && key !== 'WORKER_HEARTBEAT_FILE')
      .map(([, value]) => value)) {
      assert.equal((await stat(path)).mode & 0o777, 0o600);
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
test('bootstrap fails closed for privilege mismatch, malformed key, and symlink overwrite', async () => {
  for (const changed of [
    { CLOUD_DATABASE_USERNAME: 'myfans_owner' },
    { CLOUD_MFA_KEY_HEX: 'g'.repeat(64) },
    { CLOUD_DATABASE_HOST: 'localhost' },
  ]) {
    const directory = await mkdtemp(join(tmpdir(), 'myfans-cloud-'));
    try {
      await assert.rejects(prepareEnvironment({ ...config, ...changed }, 'web', directory));
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  }
  const directory = await mkdtemp(join(tmpdir(), 'myfans-cloud-'));
  try {
    await symlink(join(directory, 'target'), join(directory, 'database-url'));
    await assert.rejects(prepareEnvironment(config, 'web', directory));
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
test('migration requires only owner and runtime credentials, never provider secrets', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'myfans-cloud-'));
  try {
    const env = await prepareEnvironment(
      {
        CLOUD_DATABASE_HOST: config.CLOUD_DATABASE_HOST,
        CLOUD_DATABASE_NAME: 'myfans',
        CLOUD_DATABASE_USERNAME: 'myfans_owner',
        CLOUD_DATABASE_PASSWORD: 'owner-fixture',
        CLOUD_RUNTIME_PASSWORD: 'runtime-fixture',
      },
      'migrate',
      directory,
    );
    assert.equal(env.APP_RUN_MIGRATIONS, 'true');
    assert.equal(env.MFA_ENCRYPTION_KEY_FILE, undefined);
    assert.equal(await readFile(env.DATABASE_RUNTIME_PASSWORD_FILE, 'utf8'), 'runtime-fixture');
    assert.equal(
      Object.keys(env).some((key) => key.startsWith('CLOUD_')),
      false,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
test('key rotation provides only private key files to the owner command', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'myfans-cloud-'));
  try {
    const env = await prepareEnvironment(
      {
        CLOUD_DATABASE_HOST: config.CLOUD_DATABASE_HOST,
        CLOUD_DATABASE_NAME: 'myfans',
        CLOUD_DATABASE_USERNAME: 'myfans_owner',
        CLOUD_DATABASE_PASSWORD: 'owner-fixture',
        CLOUD_MFA_KEY_HEX: 'ab'.repeat(32),
        CLOUD_NEXT_MFA_KEY_HEX: 'cd'.repeat(32),
        KEY_ROTATION_ID: '12345678-1234-1234-1234-123456789abc',
      },
      'rotate-key',
      directory,
    );
    assert.equal(env.APP_RUN_MIGRATIONS, 'false');
    assert.deepEqual(await readFile(env.MFA_ENCRYPTION_KEY_FILE), Buffer.alloc(32, 171));
    assert.deepEqual(await readFile(env.NEXT_MFA_ENCRYPTION_KEY_FILE), Buffer.alloc(32, 205));
    assert.equal((await stat(env.NEXT_MFA_ENCRYPTION_KEY_FILE)).mode & 0o777, 0o600);
    assert.equal(
      Object.keys(env).some((key) => key.startsWith('CLOUD_')),
      false,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
test('historical erasure needs only an owner DSN and preserves the bounded request identity', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'myfans-cloud-'));
  try {
    const env = await prepareEnvironment(
      {
        CLOUD_DATABASE_HOST: config.CLOUD_DATABASE_HOST,
        CLOUD_DATABASE_NAME: 'myfans',
        CLOUD_DATABASE_USERNAME: 'myfans_owner',
        CLOUD_DATABASE_PASSWORD: 'owner-fixture',
        PRIVACY_REQUEST_ID: '12345678-1234-1234-1234-123456789abc',
        APP_RUN_MIGRATIONS: 'false',
      },
      'privacy-purge',
      directory,
    );
    assert.equal(env.MFA_ENCRYPTION_KEY_FILE, undefined);
    assert.equal(env.DATABASE_RUNTIME_PASSWORD_FILE, undefined);
    assert.equal(env.PRIVACY_REQUEST_ID, '12345678-1234-1234-1234-123456789abc');
    assert.equal(
      Object.keys(env).some((key) => key.startsWith('CLOUD_')),
      false,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
