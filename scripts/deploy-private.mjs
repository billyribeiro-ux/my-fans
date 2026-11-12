import { randomBytes, randomUUID, createHash } from 'node:crypto';
import { mkdir, readFile, writeFile, stat, open, rm, mkdtemp } from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import { spawn } from 'node:child_process';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const usage =
  'Usage: node scripts/deploy-private.mjs prepare <name> <sha256:image-id|registry/image@sha256:digest> [port] | up|status|stop|seed|backup <deployment-directory> | verify-backup <deployment-directory> <backup-directory> | verify-upgrade <deployment-directory> <backup-directory> <immutable-candidate-image>';
function validate(config) {
  if (config.version !== 1 || !/^[a-z][a-z0-9-]{2,40}$/.test(config.name))
    throw new Error('Invalid deployment name/version');
  if (!/^(?:sha256:|[a-zA-Z0-9./:_-]+@sha256:)[a-f0-9]{64}$/.test(config.image))
    throw new Error(
      'Use a complete immutable image ID or registry digest; mutable tags are rejected',
    );
  if (!Number.isInteger(config.port) || config.port < 1024 || config.port > 65535)
    throw new Error('Local port must be between 1024 and 65535');
}
function run(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: ['pipe', 'inherit', 'inherit'],
      ...options,
    });
    child.on('error', () => reject(new Error(`Cannot start ${command}`)));
    child.on('exit', (code) =>
      code === 0
        ? resolvePromise()
        : reject(
            new Error(
              `${command} failed (exit ${code}); deployment remains stopped if migration or grants failed`,
            ),
          ),
    );
    child.stdin?.end(options.input);
  });
}
async function capture(command, args, options = {}) {
  let output = '';
  await new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: root,
      stdio: ['ignore', 'pipe', 'inherit'],
      ...options,
    });
    child.stdout.on('data', (chunk) => {
      output += chunk;
      if (output.length > 16 * 1024 * 1024) {
        child.kill();
        reject(new Error('Command output exceeded the safety limit'));
      }
    });
    child.on('error', () => reject(new Error(`Cannot start ${command}`)));
    child.on('exit', (code) =>
      code === 0 ? resolvePromise() : reject(new Error(`${command} failed (exit ${code})`)),
    );
  });
  return output;
}
async function checksum(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

try {
  const [command, location, image, port] = process.argv.slice(2);
  if (command === 'prepare') {
    if (!location || !image) throw new Error(usage);
    const config = { version: 1, name: location, image, port: Number(port || 3100) };
    validate(config);
    const directory = join(root, '.local', 'deployments', config.name);
    await mkdir(join(root, '.local', 'deployments'), { recursive: true, mode: 0o700 });
    // Do not replace credentials for an initialized database.
    await mkdir(directory, { mode: 0o700 });
    await mkdir(join(directory, 'secrets'), { mode: 0o700 });
    const credentials = {
      bootstrap_password: randomBytes(32).toString('hex'),
      owner_password: randomBytes(32).toString('hex'),
      runtime_password: randomBytes(32).toString('hex'),
    };
    credentials.owner_url = `postgresql://myfans_owner:${credentials.owner_password}@postgres:5432/myfans`;
    credentials.runtime_url = `postgresql://myfans_app:${credentials.runtime_password}@postgres:5432/myfans`;
    for (const [name, value] of Object.entries(credentials)) {
      // The private 0700 parents protect host access. Read-only 0444 files permit
      // non-root container UIDs to read individual bind-mounted Compose secrets.
      await writeFile(join(directory, 'secrets', name), `${value}\n`, { mode: 0o444, flag: 'wx' });
    }
    await writeFile(join(directory, 'deployment.json'), `${JSON.stringify(config, null, 2)}\n`, {
      mode: 0o600,
      flag: 'wx',
    });
    console.log(
      `Prepared private staging deployment: ${directory}\nCredentials were not printed. No containers started.`,
    );
  } else {
    if (
      !['up', 'status', 'stop', 'seed', 'backup', 'verify-backup', 'verify-upgrade'].includes(
        command,
      ) ||
      !location ||
      (image && !['verify-backup', 'verify-upgrade'].includes(command)) ||
      (port && command !== 'verify-upgrade')
    )
      throw new Error(usage);
    const directory = resolve(location);
    if ((await stat(directory)).mode & 0o077)
      throw new Error('Deployment directory must be private (mode 0700)');
    const config = JSON.parse(await readFile(join(directory, 'deployment.json'), 'utf8'));
    validate(config);
    const env = {
      ...process.env,
      MYFANS_IMAGE: config.image,
      MYFANS_PORT: String(config.port),
      MYFANS_DEPLOY_ROOT: directory,
    };
    const compose = [
      'compose',
      '--project-name',
      config.name,
      '--file',
      join(root, 'deploy/private.compose.yaml'),
    ];
    const docker = (args, options = {}) =>
      run('docker', [...compose, ...args], { env, ...options });
    const query = (args) => capture('docker', [...compose, ...args], { env });
    if (command === 'up') {
      const metadata = JSON.parse(await capture('docker', ['image', 'inspect', config.image]))[0];
      if (metadata.Config.Labels?.['io.myfans.web-origin'] !== `http://127.0.0.1:${config.port}`)
        throw new Error(
          'The image build-time APP_ORIGIN must match the private staging port; rebuild with --build-arg APP_ORIGIN=http://127.0.0.1:PORT',
        );
      if (
        metadata.Config.User !== 'node' ||
        !metadata.Config.Healthcheck?.Test?.includes('scripts/healthcheck.mjs')
      )
        throw new Error('Use the unprivileged MyFans image with both health probes');
      // Docker's containerd image store can discard an untagged local index after
      // a build tag moves. Retain this exact image for restart/restore; execution
      // still uses the immutable reference, never the retention tag.
      const retentionTag = `myfans-retained:${metadata.Id.replace('sha256:', '')}`;
      await run('docker', ['image', 'tag', config.image, retentionTag]);
      await docker(['config', '--quiet']);
      // Drain writers before migration. Failure never starts the old binary against a new schema.
      await docker(['stop', 'app']);
      await docker(['up', '-d', '--wait', '--wait-timeout', '120', 'postgres']);
      await docker(['run', '--rm', '--no-deps', '-T', 'migrate']);
      await docker(
        [
          'exec',
          '-T',
          'postgres',
          'psql',
          '-U',
          'postgres',
          '-d',
          'myfans',
          '-v',
          'ON_ERROR_STOP=1',
        ],
        { input: await readFile(join(root, 'db/roles/runtime.sql')) },
      );
      await docker(['up', '-d', '--wait', '--wait-timeout', '120', 'app']);
      console.log(
        `Private staging healthy at http://127.0.0.1:${config.port}. Production remains disabled.`,
      );
    } else if (command === 'seed') {
      await docker(['run', '--rm', '--no-deps', '-T', 'migrate', 'seed']);
      console.log(
        'Development fixtures installed, including documented demo credentials. Keep this deployment private.',
      );
    } else if (command === 'stop') {
      await docker(['stop', 'app']);
    } else if (command === 'backup') {
      if ((await query(['ps', '--status', 'running', '-q', 'app'])).trim())
        throw new Error('Stop the staging app before taking a consistent database/media backup');
      const app = (await query(['ps', '--all', '-q', 'app'])).trim();
      if (!app) throw new Error('A stopped staging app is required to back up its media');
      const container = JSON.parse(await capture('docker', ['container', 'inspect', app]))[0];
      if (container.Config.Image !== config.image)
        throw new Error('Back up the current release before changing the deployment image');
      await capture('docker', ['image', 'inspect', config.image]);
      const hasStorageBinding =
        (
          await query([
            'exec',
            '-T',
            'postgres',
            'psql',
            '-U',
            'postgres',
            '-d',
            'myfans',
            '-At',
            '-v',
            'ON_ERROR_STOP=1',
            '-c',
            "SELECT to_regclass('public.platform_storage') IS NOT NULL",
          ])
        ).trim() === 't';
      const storageBinding = hasStorageBinding
        ? (
            await query([
              'exec',
              '-T',
              'postgres',
              'psql',
              '-U',
              'postgres',
              '-d',
              'myfans',
              '-At',
              '-v',
              'ON_ERROR_STOP=1',
              '-c',
              'SELECT media_store_identity FROM platform_storage WHERE id=1',
            ])
          ).trim()
        : 'local:v1';
      if (
        storageBinding !== 'local:v1' ||
        container.Config.Env.some((value) => value.startsWith('MFA_ENCRYPTION_KEY_FILE='))
      )
        throw new Error(
          'This backup workflow requires local media and its local key; S3 and external keys need coordinated versioned recovery',
        );
      const destination = join(
        directory,
        'backups',
        `${new Date().toISOString().replaceAll(':', '-')}-${randomUUID()}`,
      );
      await mkdir(destination, { recursive: true, mode: 0o700 });
      const dump = await open(join(destination, 'database.dump'), 'wx', 0o600);
      try {
        await docker(
          [
            'exec',
            '-T',
            'postgres',
            'pg_dump',
            '-U',
            'postgres',
            '-d',
            'myfans',
            '--format=custom',
          ],
          { stdio: ['pipe', dump.fd, 'inherit'] },
        );
      } finally {
        await dump.close();
      }
      await docker(['cp', 'app:/app/.local/media', join(destination, 'media')]);
      await run('tar', ['-czf', join(destination, 'media.tar.gz'), '-C', destination, 'media']);
      await rm(join(destination, 'media'), { recursive: true });
      const manifest = {
        version: 1,
        image: config.image,
        created: new Date().toISOString(),
        database: await checksum(join(destination, 'database.dump')),
        media: await checksum(join(destination, 'media.tar.gz')),
      };
      await writeFile(join(destination, 'backup.json'), `${JSON.stringify(manifest, null, 2)}\n`, {
        mode: 0o600,
        flag: 'wx',
      });
      console.log(
        `Consistent private backup created: ${destination}. Encrypt it before off-host storage.`,
      );
    } else if (command === 'verify-backup' || command === 'verify-upgrade') {
      if (!image) throw new Error(usage);
      if (command === 'verify-upgrade') {
        if (!port) throw new Error(usage);
        validate({ ...config, image: port });
        const candidate = JSON.parse(await capture('docker', ['image', 'inspect', port]))[0];
        if (
          candidate.Config.User !== 'node' ||
          candidate.Config.Labels?.['io.myfans.web-origin'] !== `http://127.0.0.1:${config.port}` ||
          !candidate.Config.Healthcheck?.Test?.includes('scripts/healthcheck.mjs')
        )
          throw new Error(
            'Upgrade candidate must use the unprivileged runtime, both health probes, and the same private origin',
          );
      }
      const backup = resolve(image);
      const manifest = JSON.parse(await readFile(join(backup, 'backup.json'), 'utf8'));
      if (manifest.version !== 1 || manifest.image !== config.image)
        throw new Error('Restore verification requires the matching immutable release image');
      // Check image availability before creating an isolated restore database.
      await capture('docker', ['image', 'inspect', config.image]);
      if (
        manifest.database !== (await checksum(join(backup, 'database.dump'))) ||
        manifest.media !== (await checksum(join(backup, 'media.tar.gz')))
      )
        throw new Error('Backup checksum verification failed');
      const archive = join(backup, 'media.tar.gz');
      const names = (await capture('tar', ['-tzf', archive])).trim().split('\n');
      if (
        !names.length ||
        names.some(
          (name) =>
            !/^media(?:\/|$)/.test(name) ||
            name.split('/').some((part) => part === '..' || part === '.'),
        )
      )
        throw new Error('Unsafe backup archive path');
      const entries = (await capture('tar', ['-tvzf', archive])).trim().split('\n');
      if (entries.some((line) => !['d', '-'].includes(line[0])))
        throw new Error('Backup archive must contain only regular files and directories');
      const temporary = await mkdtemp(join(directory, '.restore-'));
      const database = `myfans_restore_${randomBytes(12).toString('hex')}`;
      let created = false;
      try {
        await run('tar', ['-xzf', archive, '-C', temporary]);
        const key = await stat(join(temporary, 'media/.mfa-key'));
        if (!key.isFile() || key.size !== 32)
          throw new Error('Backup encryption key is missing or invalid');
        const url = (await readFile(join(directory, 'secrets/owner_url'), 'utf8')).trim();
        const restoreURL = new URL(url);
        restoreURL.pathname = `/${database}`;
        await writeFile(join(temporary, 'restore_url'), `${restoreURL}\n`, {
          mode: 0o444,
          flag: 'wx',
        });
        await docker([
          'exec',
          '-T',
          'postgres',
          'createdb',
          '-U',
          'postgres',
          '--owner=myfans_owner',
          database,
        ]);
        created = true;
        const input = await open(join(backup, 'database.dump'), 'r');
        try {
          await docker(
            [
              'exec',
              '-T',
              'postgres',
              'pg_restore',
              '-U',
              'postgres',
              '-d',
              database,
              '--exit-on-error',
            ],
            { stdio: [input.fd, 'inherit', 'inherit'] },
          );
        } finally {
          await input.close();
        }
        await docker([
          'exec',
          '-T',
          'postgres',
          'psql',
          '-U',
          'postgres',
          '-d',
          database,
          '-v',
          'ON_ERROR_STOP=1',
          '-c',
          "DO $$ BEGIN IF EXISTS(SELECT 1 FROM ledger_transactions t LEFT JOIN ledger_postings p ON p.transaction_id=t.id WHERE t.status='posted' GROUP BY t.id HAVING COALESCE(sum(p.amount_minor),0)<>0 OR count(p.id)<2) THEN RAISE EXCEPTION 'Unbalanced restored ledger'; END IF; END $$;",
        ]);
        const keys = (
          await query([
            'exec',
            '-T',
            'postgres',
            'psql',
            '-U',
            'postgres',
            '-d',
            database,
            '-At',
            '-v',
            'ON_ERROR_STOP=1',
            '-c',
            "SELECT storage_key FROM media_assets WHERE state IN ('processing','quarantine','approved') UNION SELECT preview_key FROM media_assets WHERE state IN ('quarantine','approved') AND preview_key IS NOT NULL",
          ])
        ).trim();
        for (const name of keys ? keys.split('\n') : []) {
          if (
            !/^[a-zA-Z0-9_-]+(?:\.[a-zA-Z0-9_-]+)+$/.test(name) ||
            !(await stat(join(temporary, 'media', name))).isFile()
          )
            throw new Error('A restored media reference is invalid or missing');
        }
        await docker([
          'run',
          '--rm',
          '--no-deps',
          '-T',
          '--user',
          `${key.uid}:${key.gid}`,
          '--entrypoint',
          'myfans-api',
          '-e',
          'DATABASE_URL_FILE=/run/secrets/restore_url',
          '-e',
          'APP_RUN_MIGRATIONS=false',
          '-e',
          'MEDIA_ROOT=/verify/media',
          '-v',
          `${join(temporary, 'restore_url')}:/run/secrets/restore_url:ro`,
          '-v',
          `${join(temporary, 'media')}:/verify/media:ro`,
          'migrate',
          'verify-storage',
        ]);
        console.log(
          'Backup checksums, ledger, media references, migration checksums, and encrypted storage verified in an isolated restore.',
        );
        if (command === 'verify-upgrade') {
          const candidateEnv = { ...env, MYFANS_IMAGE: port };
          const verify = [
            'run',
            '--rm',
            '--no-deps',
            '-T',
            '--user',
            `${key.uid}:${key.gid}`,
            '--entrypoint',
            'myfans-api',
            '-e',
            'MEDIA_ROOT=/verify/media',
            '-v',
            `${join(temporary, 'media')}:/verify/media:ro`,
          ];
          await run(
            'docker',
            [
              ...compose,
              ...verify,
              '-e',
              'DATABASE_URL_FILE=/run/secrets/restore_url',
              '-e',
              'APP_RUN_MIGRATIONS=true',
              '-v',
              `${join(temporary, 'restore_url')}:/run/secrets/restore_url:ro`,
              'migrate',
              'migrate',
            ],
            { env: candidateEnv },
          );
          await docker(
            [
              'exec',
              '-T',
              'postgres',
              'psql',
              '-U',
              'postgres',
              '-d',
              database,
              '-v',
              'ON_ERROR_STOP=1',
            ],
            { input: await readFile(join(root, 'db/roles/runtime.sql')) },
          );
          const runtime = new URL(
            (await readFile(join(directory, 'secrets/runtime_url'), 'utf8')).trim(),
          );
          runtime.pathname = `/${database}`;
          await writeFile(join(temporary, 'restore_runtime_url'), `${runtime}\n`, {
            mode: 0o444,
            flag: 'wx',
          });
          await run(
            'docker',
            [
              ...compose,
              ...verify,
              '-e',
              'DATABASE_URL_FILE=/run/secrets/restore_runtime_url',
              '-e',
              'APP_RUN_MIGRATIONS=false',
              '-v',
              `${join(temporary, 'restore_runtime_url')}:/run/secrets/restore_runtime_url:ro`,
              'migrate',
              'verify-storage',
            ],
            { env: candidateEnv },
          );
          console.log(
            'Candidate forward migrations and restricted-role encrypted storage checks passed against the isolated restored snapshot. Live deployment was not changed.',
          );
        }
      } finally {
        if (created)
          await docker([
            'exec',
            '-T',
            'postgres',
            'dropdb',
            '-U',
            'postgres',
            '--if-exists',
            database,
          ]);
        await rm(temporary, { recursive: true });
      }
    } else {
      await docker(['ps']);
    }
  }
} catch (error) {
  // Do not echo config contents, secret files, or raw JSON parser errors.
  console.error(
    error instanceof SyntaxError ? 'Invalid deployment configuration JSON' : error.message,
  );
  process.exitCode = 1;
}
