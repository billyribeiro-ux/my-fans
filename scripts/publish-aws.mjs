import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { aws, identity, outputs, validateAccount } from './deploy-aws.mjs';

async function docker(args, inherit = false) {
  return new Promise((accept, reject) => {
    const child = spawn('docker', args, {
      stdio: inherit ? 'inherit' : ['ignore', 'pipe', 'pipe'],
    });
    let text = '';
    if (!inherit)
      child.stdout.on('data', (b) => {
        text += b;
      });
    if (!inherit) child.stderr.resume();
    child.once('error', () => reject(new Error('Docker is unavailable')));
    child.once('close', (code) =>
      code ? reject(new Error(`Docker ${args[0]} failed`)) : accept(text),
    );
  });
}
async function login(c) {
  // The short-lived registry token flows directly between processes, never into
  // arguments, a file, logs, or this program's result.
  return new Promise((accept, reject) => {
    const credential = spawn('aws', ['ecr', 'get-login-password', '--region', c.region], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const consume = spawn(
      'docker',
      [
        'login',
        '--username',
        'AWS',
        '--password-stdin',
        `${c.accountId}.dkr.ecr.${c.region}.amazonaws.com`,
      ],
      { stdio: ['pipe', 'ignore', 'pipe'] },
    );
    credential.stdout.pipe(consume.stdin);
    credential.stderr.resume();
    consume.stderr.resume();
    let credentialCode;
    let loginCode;
    const done = () => {
      if (credentialCode !== undefined && loginCode !== undefined)
        credentialCode === 0 && loginCode === 0
          ? accept()
          : reject(new Error('ECR authentication failed'));
    };
    credential.once('error', () => reject(new Error('AWS CLI is unavailable')));
    consume.once('error', () => reject(new Error('Docker is unavailable')));
    consume.stdin.on('error', () => {});
    credential.once('close', (code) => {
      credentialCode = code;
      done();
    });
    consume.once('close', (code) => {
      loginCode = code;
      done();
    });
  });
}
async function main() {
  const [configPath, appId, scannerId] = process.argv.slice(2);
  const c = validateAccount(JSON.parse(await readFile(configPath, 'utf8')));
  for (const id of [appId, scannerId])
    if (!/^sha256:[a-f0-9]{64}$/u.test(id))
      throw new Error('Supply exact locally tested application and scanner image IDs');
  if (!/^[a-z0-9][a-z0-9.-]+\.[a-z]{2,63}$/u.test(c.domainName))
    throw new Error('Supply the application domain before publication');
  await identity(c);
  const registry = await outputs(c, 'registry');
  const inspected = JSON.parse(await docker(['image', 'inspect', appId, scannerId]));
  if (
    inspected.length !== 2 ||
    inspected.some((v) => v.Os !== 'linux' || v.Architecture !== 'arm64')
  )
    throw new Error('Cloud publication requires Linux ARM64 images');
  if (inspected[0].Config.Labels?.['io.myfans.web-origin'] !== `https://${c.domainName}`)
    throw new Error('Application was built for a different origin');
  await login(c);
  const published = {};
  for (const [id, repository, key] of [
    [appId, registry.ApplicationRepositoryUri, 'imageUri'],
    [scannerId, registry.ScannerRepositoryUri, 'scannerImageUri'],
  ]) {
    const tag = `release-${id.slice(7)}`;
    await docker(['tag', id, `${repository}:${tag}`]);
    await docker(['push', `${repository}:${tag}`], true);
    const name = repository.split('.amazonaws.com/')[1];
    const result = await aws(c, [
      'ecr',
      'describe-images',
      '--repository-name',
      name,
      '--image-ids',
      `imageTag=${tag}`,
    ]);
    if (
      result.imageDetails.length !== 1 ||
      !/^sha256:[a-f0-9]{64}$/u.test(result.imageDetails[0].imageDigest)
    )
      throw new Error('Published image digest unavailable');
    published[key] = `${repository}@${result.imageDetails[0].imageDigest}`;
  }
  console.log(
    JSON.stringify(
      {
        ...published,
        next: 'Record these immutable digests in account configuration. Confirm ECR and retained local image vulnerability findings before runtime deployment.',
      },
      null,
      2,
    ),
  );
}
main().catch((e) => {
  console.error(e.message);
  process.exitCode = 1;
});
