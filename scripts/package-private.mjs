import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, readFile, writeFile, readdir, lstat, mkdtemp, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';

const root = fileURLToPath(new URL('../', import.meta.url));
function execute(args, capture = false) {
  return new Promise((resolvePromise, reject) => {
    let output = '';
    const child = spawn(args[0], args.slice(1), {
      cwd: root,
      stdio: ['ignore', capture ? 'pipe' : 'inherit', 'inherit'],
    });
    if (capture)
      child.stdout.on('data', (value) => {
        output += value;
      });
    child.on('error', () => reject(new Error(`Cannot start ${args[0]}`)));
    child.on('exit', (code) =>
      code === 0 ? resolvePromise(output) : reject(new Error(`${args[0]} failed (${code})`)),
    );
  });
}
async function digest(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}
try {
  const [image, target] = process.argv.slice(2);
  if (!image || !target || !/^(?:sha256:|[a-zA-Z0-9./:_-]+@sha256:)[a-f0-9]{64}$/.test(image))
    throw new Error(
      'Usage: node scripts/package-private.mjs <immutable-image> <new-private-directory>',
    );
  const metadata = JSON.parse(await execute(['docker', 'image', 'inspect', image], true))[0];
  if (
    metadata.Config.User !== 'node' ||
    !metadata.Config.Healthcheck?.Test?.includes('scripts/healthcheck.mjs')
  )
    throw new Error('Image must use the unprivileged MyFans runtime with both health probes');
  if (metadata.Config.Labels?.['io.myfans.runtime-distro'] !== 'alpine:3.24')
    throw new Error('Image must declare the pinned Alpine 3.24 runtime distribution');
  const destination = resolve(target);
  await mkdir(destination, { mode: 0o700 });
  const source = join(destination, 'source');
  await mkdir(source, { mode: 0o700 });
  const files = {};
  async function copy(path) {
    const info = await lstat(join(root, path));
    if (info.isSymbolicLink()) throw new Error(`Source symlinks are not accepted: ${path}`);
    if (info.isDirectory()) {
      await mkdir(join(source, path), { recursive: true, mode: 0o700 });
      for (const entry of (await readdir(join(root, path))).sort()) {
        if (
          entry.startsWith('.') ||
          ['node_modules', 'target', 'build', 'test-results', 'playwright-report'].includes(entry)
        )
          continue;
        await copy(join(path, entry));
      }
    } else {
      if (!info.isFile() || info.size > 16 * 1024 * 1024)
        throw new Error(`Unexpected source artifact: ${path}`);
      const contents = await readFile(join(root, path));
      await mkdir(join(source, path, '..'), { recursive: true, mode: 0o700 });
      await writeFile(join(source, path), contents, {
        mode: info.mode & 0o111 ? 0o700 : 0o600,
        flag: 'wx',
      });
      files[path] = createHash('sha256').update(contents).digest('hex');
    }
  }
  // Explicit roots prevent inclusion of databases, uploaded media, keys, auth state,
  // personal tooling settings, generated output, environment files, or traces.
  for (const path of [
    'Cargo.toml',
    'Cargo.lock',
    'rust-toolchain.toml',
    'package.json',
    'pnpm-lock.yaml',
    'pnpm-workspace.yaml',
    '.npmrc',
    '.gitignore',
    '.dockerignore',
    '.prettierignore',
    '.prettierrc.json',
    '.node-version',
    'Dockerfile',
    'compose.yaml',
    'README.md',
    'TO-DO.md',
    'THIRD_PARTY_NOTICES.md',
    'services',
    'db',
    'deploy',
    'docs',
    'scripts',
    '.github/workflows',
    'apps/web/package.json',
    'apps/web/tsconfig.json',
    'apps/web/vite.config.ts',
    'apps/web/vitest.config.ts',
    'apps/web/playwright.config.ts',
    'apps/web/src',
    'apps/web/static',
    'apps/web/tests',
  ])
    await copy(path);
  await execute(['docker', 'image', 'save', '--output', join(destination, 'image.tar'), image]);
  const isolation = [
    '--rm',
    '--read-only',
    '--user',
    `${process.getuid()}:${process.getgid()}`,
    '--cap-drop',
    'ALL',
    '--security-opt',
    'no-new-privileges',
    '--tmpfs',
    '/tmp:rw,noexec,nosuid,size=256m,mode=1777',
  ];
  const scout =
    'docker/scout-cli:1.25.0@sha256:d385046155c692ff5f4752fc32d612202a9211ae2c7904ae52bb65f9e1dc620a';
  const grype =
    'anchore/grype:v0.120.1@sha256:e4a44ef45d285b829ce6efe2642980329661bd2d18eab5fc539138d4adaebbbe';
  const cache = join(root, '.local', 'image-scan-cache');
  await mkdir(cache, { recursive: true, mode: 0o700 });
  const scratch = await mkdtemp(join(cache, 'inventory-'));
  let sbom;
  try {
    sbom = await execute(
      [
        'docker',
        'run',
        ...isolation,
        '--network',
        'none',
        '-e',
        'DOCKER_SCOUT_OFFLINE=true',
        '-e',
        'DOCKER_SCOUT_CACHE_DIR=/scratch/scout',
        '-e',
        'XDG_CACHE_HOME=/scratch/cache',
        '-e',
        'TMPDIR=/scratch',
        '-v',
        `${scratch}:/scratch`,
        '-v',
        `${destination}:/package:ro`,
        scout,
        'sbom',
        '--format',
        'spdx',
        'archive:///package/image.tar',
      ],
      true,
    );
  } finally {
    await rm(scratch, { recursive: true });
  }
  JSON.parse(sbom);
  await writeFile(join(destination, 'runtime.spdx.json'), sbom, { mode: 0o600, flag: 'wx' });
  const report = await execute(
    [
      'docker',
      'run',
      ...isolation,
      '-e',
      'GRYPE_DB_CACHE_DIR=/cache',
      '-e',
      'GRYPE_CHECK_FOR_APP_UPDATE=false',
      '-v',
      `${cache}:/cache`,
      '-v',
      `${destination}:/package:ro`,
      grype,
      'sbom:/package/runtime.spdx.json',
      '--output',
      'json',
      '--distro',
      'alpine:3.24',
    ],
    true,
  );
  const vulnerabilities = JSON.parse(report);
  await writeFile(join(destination, 'runtime-vulnerabilities.json'), report, {
    mode: 0o600,
    flag: 'wx',
  });
  const severe = vulnerabilities.matches.filter((match) =>
    ['High', 'Critical'].includes(match.vulnerability.severity),
  );
  if (severe.length)
    throw new Error(
      `${severe.length} high/critical vulnerability matches block packaging; inspect runtime-vulnerabilities.json. No successful release manifest was created.`,
    );
  await execute(['tar', '-czf', join(destination, 'source.tar.gz'), '-C', destination, 'source']);
  const manifest = {
    version: 1,
    mode: 'private-sandbox',
    created: new Date().toISOString(),
    imageReference: image,
    imageId: metadata.Id,
    os: metadata.Os,
    architecture: metadata.Architecture,
    webOrigin: metadata.Config.Labels?.['io.myfans.web-origin'],
    runtimeDistribution: 'alpine:3.24',
    imageArchiveSha256: await digest(join(destination, 'image.tar')),
    sourceArchiveSha256: await digest(join(destination, 'source.tar.gz')),
    runtimeSbomSha256: await digest(join(destination, 'runtime.spdx.json')),
    vulnerabilityReportSha256: await digest(join(destination, 'runtime-vulnerabilities.json')),
    scanners: { scout, grype },
    vulnerabilityGate: {
      highCriticalMatches: severe.length,
      totalMatches: vulnerabilities.matches.length,
    },
    sourceFiles: files,
    limitations:
      'Private staging artifact. Hashes and runtime package inventory are not signed build provenance or production approval. Compiled/bundled dependency detection is incomplete; source lockfile audits remain required. Source snapshot is not proof this image was built from these bytes. No credentials, backups, uploads, or test traces are included. Distribution license acceptance remains required.',
  };
  await writeFile(join(destination, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`, {
    mode: 0o600,
    flag: 'wx',
  });
  console.log(`Private offline deployment package created: ${destination}`);
} catch (error) {
  console.error(error instanceof SyntaxError ? 'Invalid image metadata' : error.message);
  process.exitCode = 1;
}
