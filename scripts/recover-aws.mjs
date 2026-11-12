import { createHash, randomUUID } from 'node:crypto';
import { mkdtemp, readFile, writeFile, rm, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { aws, identity, outputs, pause, validateConfiguration } from './deploy-aws.mjs';
import { exportPrivacyJournal, loadPrivacyJournal } from './privacy-journal-aws.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const digest = (bytes) => createHash('sha256').update(bytes).digest();
export function checkpointManifestBytes(manifest) {
  const bytes = Buffer.from(JSON.stringify(manifest));
  // Leave room for the envelope fields and bounded ECDSA signature.
  if (bytes.length > 33554432 - 1024)
    throw new Error(
      'Checkpoint manifest exceeds the 32 MiB envelope limit; nothing was signed or published',
    );
  return bytes;
}
export const storageIdentity = (region, owner, bucket, prefix) =>
  `s3:${digest(`${region}\n${owner}\n${bucket}\n${prefix.replace(/\/$/u, '')}/`).toString('hex')}`;
export function validateManifest(m, c, f, media, { allowHistoricalImages = false } = {}) {
  if (
    m.version !== 1 ||
    m.accountId !== c.accountId ||
    m.region !== c.region ||
    m.deploymentName !== c.deploymentName ||
    m.databaseIdentifier !== f.DatabaseIdentifier ||
    m.mediaBucket !== media.MediaBucketName ||
    m.recoveryBucket !== media.RecoveryBucketName ||
    m.signingKeyArn !== f.RecoverySigningKeyArn ||
    m.mediaPrefix !== c.mediaPrefix ||
    !/^checkpoint-[a-f0-9-]{36}$/u.test(m.id)
  )
    throw new Error('Checkpoint is not bound to this deployment');
  for (const key of ['imageUri', 'scannerImageUri'])
    if (
      !new RegExp(
        `^${c.accountId}\\.dkr\\.ecr\\.${c.region}\\.amazonaws\\.com/[a-z0-9][a-z0-9/_-]*@sha256:[a-f0-9]{64}$`,
        'u',
      ).test(m[key])
    )
      throw new Error('Checkpoint requires immutable same-account image references');
  if (
    !allowHistoricalImages &&
    (m.imageUri !== c.imageUri || m.scannerImageUri !== c.scannerImageUri)
  )
    throw new Error('Restore must use the exact checkpoint image digests');
  if (
    m.taskDefinitions &&
    (Object.keys(m.taskDefinitions).sort().join(',') !== 'MigrationTask,WorkerTask' ||
      Object.values(m.taskDefinitions).some(
        (arn) =>
          !new RegExp(
            `^arn:aws:ecs:${c.region}:${c.accountId}:task-definition/[A-Za-z0-9_-]+:[1-9][0-9]*$`,
            'u',
          ).test(arn),
      ))
  )
    throw new Error('Checkpoint task definitions must be exact same-account revisions');
  if (
    !Array.isArray(m.objects) ||
    m.objects.length > 100000 ||
    !/^[a-z][a-z0-9-]{0,62}$/u.test(m.snapshotId) ||
    !/^[a-zA-Z0-9-]{32,64}$/u.test(m.mfaVersion) ||
    !/^privacy-[a-f0-9-]{36}$/u.test(m.privacyJournalId) ||
    !Number.isFinite(Date.parse(m.privacyJournalExportedAt))
  )
    throw new Error('Invalid checkpoint manifest');
  const seen = new Set();
  for (const item of m.objects) {
    if (
      !item.key.startsWith(`${c.mediaPrefix}/`) ||
      item.key.length > 1024 ||
      item.checkpointKey !== `${m.id}/objects/${item.key}` ||
      typeof item.checkpointVersion !== 'string' ||
      item.checkpointVersion === 'null' ||
      !/^[A-Za-z0-9+/]{43}=$/u.test(item.sha256) ||
      !Number.isSafeInteger(item.size) ||
      item.size < 0 ||
      item.size > 536870912 ||
      seen.has(item.key)
    )
      throw new Error('Invalid or duplicate checkpoint object');
    seen.add(item.key);
  }
  return m;
}
// Registered task revisions are immutable. New checkpoints sign their identities
// so recovery never borrows a newer launch contract for an older application.
export async function resolveCheckpointTasks(c, m, runtime, call = aws) {
  const references = m.taskDefinitions || {
    WorkerTask: runtime.WorkerTask,
    MigrationTask: runtime.MigrationTask,
  };
  const definitions = {};
  for (const key of ['WorkerTask', 'MigrationTask']) {
    const task = (
      await call(c, ['ecs', 'describe-task-definition', '--task-definition', references[key]])
    ).taskDefinition;
    const app = task?.containerDefinitions?.find((v) => v.name === 'app');
    const scanner = task?.containerDefinitions?.find((v) => v.name === 'scanner');
    const env = Object.fromEntries((app?.environment || []).map((v) => [v.name, v.value]));
    const secrets = Object.fromEntries((app?.secrets || []).map((v) => [v.name, v.valueFrom]));
    if (
      task?.taskDefinitionArn !== references[key] ||
      app?.image !== m.imageUri ||
      app.entryPoint?.length !== 2 ||
      app.entryPoint[0] !== 'node' ||
      app.entryPoint[1] !== '/app/scripts/cloud-entrypoint.mjs' ||
      app.command?.length !== 1 ||
      app.command[0] !== (key === 'WorkerTask' ? 'worker' : 'migrate') ||
      task.networkMode !== 'awsvpc' ||
      task.runtimePlatform?.cpuArchitecture !== 'ARM64' ||
      task.containerDefinitions.some(
        (v) =>
          !['app', 'prepare', 'scanner'].includes(v.name) ||
          v.environmentFiles?.length ||
          v.image !== (v.name === 'scanner' ? m.scannerImageUri : m.imageUri),
      ) ||
      (key === 'WorkerTask' &&
        (!scanner ||
          env.PUBLIC_ORIGIN !== `https://${c.domainName}` ||
          env.MAIL_PUBLIC_ORIGIN !== `https://${c.domainName}` ||
          secrets.CLOUD_OPERATOR_POLICY !== c.policySecretArn)) ||
      !secrets.CLOUD_DATABASE_PASSWORD ||
      (key === 'WorkerTask' && !secrets.CLOUD_MFA_KEY_HEX) ||
      (key === 'MigrationTask' && (scanner || !secrets.CLOUD_RUNTIME_PASSWORD))
    )
      throw new Error(
        m.taskDefinitions
          ? 'Retained checkpoint task definition no longer matches its signed image and launch contract'
          : 'Legacy checkpoint has no signed task revisions and current tasks differ; use the documented paused historical-definition procedure',
      );
    if (key === 'WorkerTask') {
      for (const [name, value] of Object.entries(secrets))
        if (
          [
            'CLOUD_STRIPE_SECRET',
            'CLOUD_STRIPE_WEBHOOK_SECRET',
            'CLOUD_SUMSUB_TOKEN',
            'CLOUD_SUMSUB_SECRET',
            'CLOUD_SUMSUB_WEBHOOK_SECRET',
            'CLOUD_SMTP_USERNAME',
            'CLOUD_SMTP_PASSWORD',
          ].includes(name) &&
          !value.startsWith(`${c.providerSecretArn}:`)
        )
          throw new Error(
            'Historical task references a different provider secret; review the historical configuration',
          );
    }
    definitions[key] = structuredClone(task);
  }
  // Fail before restoring a database if retained images were removed from ECR.
  for (const uri of [m.imageUri, m.scannerImageUri]) {
    const [repository, imageDigest] = uri.split('.amazonaws.com/')[1].split('@');
    const images = await call(c, [
      'ecr',
      'describe-images',
      '--repository-name',
      repository,
      '--image-ids',
      `imageDigest=${imageDigest}`,
    ]);
    if (images.imageDetails?.length !== 1 || images.imageDetails[0].imageDigest !== imageDigest)
      throw new Error('Exact checkpoint image is no longer available in ECR');
  }
  return definitions;
}
async function wait(check, label, timeout = 90 * 60000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    console.log(`${label}: waiting`);
    await new Promise((accept) => setTimeout(accept, 20000));
  }
  throw new Error(`${label} timed out; resources were retained for investigation`);
}
async function currentVersion(c, arn) {
  if (c.mfaSecretVersion) return c.mfaSecretVersion;
  const metadata = await aws(c, ['secretsmanager', 'describe-secret', '--secret-id', arn]);
  const current = Object.entries(metadata.VersionIdsToStages || {}).filter(([, stages]) =>
    stages.includes('AWSCURRENT'),
  );
  if (current.length !== 1) throw new Error('Expected one current encryption-key version');
  return current[0][0];
}
const copySource = (bucket, key, version) =>
  `${bucket}/${key.split('/').map(encodeURIComponent).join('/')}?versionId=${encodeURIComponent(version)}`;
export async function checkpoint(c, directory) {
  const [f, media] = await Promise.all([outputs(c, 'foundation'), outputs(c, 'media')]);
  f.MfaKeyArn = c.mfaSecretArn || f.MfaKeyArn;
  const previousCounts = await pause(c);
  const journal = await exportPrivacyJournal(c, directory);
  const runtime = await outputs(c, 'runtime');
  const taskDefinitions = { WorkerTask: runtime.WorkerTask, MigrationTask: runtime.MigrationTask };
  await resolveCheckpointTasks(c, { ...c, taskDefinitions }, runtime);
  const id = `checkpoint-${randomUUID()}`;
  const snapshotId = `${c.deploymentName}-${randomUUID()}`;
  const m = {
    version: 1,
    id,
    createdAt: new Date().toISOString(),
    accountId: c.accountId,
    region: c.region,
    deploymentName: c.deploymentName,
    databaseIdentifier: f.DatabaseIdentifier,
    snapshotId,
    mediaBucket: media.MediaBucketName,
    recoveryBucket: media.RecoveryBucketName,
    mediaPrefix: c.mediaPrefix,
    imageUri: c.imageUri,
    scannerImageUri: c.scannerImageUri,
    taskDefinitions,
    signingKeyArn: f.RecoverySigningKeyArn,
    mfaSecretArn: f.MfaKeyArn,
    mfaVersion: await currentVersion(c, f.MfaKeyArn),
    previousCounts,
    privacyJournalId: journal.manifest.id,
    privacyJournalExportedAt: journal.manifest.exportedAt,
    objects: [],
  };
  const created = await aws(c, [
    'rds',
    'create-db-snapshot',
    '--db-instance-identifier',
    f.DatabaseIdentifier,
    '--db-snapshot-identifier',
    snapshotId,
    '--tags',
    `Key=MyFansCheckpoint,Value=${id}`,
  ]);
  m.snapshotArn = created.DBSnapshot.DBSnapshotArn;
  let keyMarker;
  let versionMarker;
  do {
    const args = [
      's3api',
      'list-object-versions',
      '--bucket',
      media.MediaBucketName,
      '--prefix',
      `${c.mediaPrefix}/`,
      '--expected-bucket-owner',
      c.accountId,
      '--max-keys',
      '1000',
      '--no-paginate',
    ];
    if (keyMarker) args.push('--key-marker', keyMarker);
    if (versionMarker) args.push('--version-id-marker', versionMarker);
    const page = await aws(c, args);
    for (const object of (page.Versions || []).filter((v) => v.IsLatest)) {
      if (m.objects.length >= 100000 || object.Size > 536870912 || object.VersionId === 'null')
        throw new Error(
          'Checkpoint size/version bounds exceeded; no unsigned manifest will be accepted',
        );
      const checkpointKey = `${id}/objects/${object.Key}`;
      const result = await aws(c, [
        's3api',
        'copy-object',
        '--bucket',
        media.RecoveryBucketName,
        '--key',
        checkpointKey,
        '--copy-source',
        copySource(media.MediaBucketName, object.Key, object.VersionId),
        '--expected-bucket-owner',
        c.accountId,
        '--expected-source-bucket-owner',
        c.accountId,
        '--server-side-encryption',
        'aws:kms',
        '--ssekms-key-id',
        media.RecoveryEncryptionKeyArn,
        '--checksum-algorithm',
        'SHA256',
      ]);
      if (!result.VersionId || !result.CopyObjectResult.ChecksumSHA256)
        throw new Error('Recovery copy did not return an immutable version and SHA256 checksum');
      m.objects.push({
        key: object.Key,
        sourceVersion: object.VersionId,
        checkpointKey,
        checkpointVersion: result.VersionId,
        size: object.Size,
        sha256: result.CopyObjectResult.ChecksumSHA256,
      });
    }
    keyMarker = page.IsTruncated ? page.NextKeyMarker : undefined;
    versionMarker = page.IsTruncated ? page.NextVersionIdMarker : undefined;
    if (page.IsTruncated && !keyMarker) throw new Error('Invalid S3 pagination cursor');
  } while (keyMarker);
  await wait(async () => {
    const snap = (
      await aws(c, ['rds', 'describe-db-snapshots', '--db-snapshot-identifier', snapshotId])
    ).DBSnapshots[0];
    if (snap.Status === 'failed') throw new Error('Database snapshot failed');
    return snap.Status === 'available';
  }, 'database checkpoint');
  if (m.mfaVersion !== (await currentVersion(c, f.MfaKeyArn)))
    throw new Error(
      'Encryption key rotated during checkpoint; retry without concurrent key rotation',
    );
  // Keep the exact key version from becoming an unlabelled, deprecated secret
  // version while an off-host checkpoint depends on it.
  await aws(c, [
    'secretsmanager',
    'update-secret-version-stage',
    '--secret-id',
    f.MfaKeyArn,
    '--version-stage',
    `RECOVERY-${m.mfaVersion}`,
    '--move-to-version-id',
    m.mfaVersion,
  ]);
  const bytes = checkpointManifestBytes(m);
  const signed = await aws(c, [
    'kms',
    'sign',
    '--key-id',
    f.RecoverySigningKeyArn,
    '--message',
    digest(bytes).toString('base64'),
    '--message-type',
    'DIGEST',
    '--signing-algorithm',
    'ECDSA_SHA_256',
    '--cli-binary-format',
    'base64',
  ]);
  const envelope = { manifest: m, signature: signed.Signature };
  const path = join(directory, 'checkpoint.json');
  await writeFile(path, JSON.stringify(envelope), { mode: 0o600 });
  const uploaded = await aws(c, [
    's3api',
    'put-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    `${id}/manifest.json`,
    '--body',
    path,
    '--expected-bucket-owner',
    c.accountId,
    '--server-side-encryption',
    'aws:kms',
    '--ssekms-key-id',
    media.RecoveryEncryptionKeyArn,
    '--checksum-algorithm',
    'SHA256',
    '--if-none-match',
    '*',
  ]);
  console.log(
    JSON.stringify({
      checkpoint: id,
      manifestVersion: uploaded.VersionId,
      snapshotArn: m.snapshotArn,
      objects: m.objects.length,
      services: 'paused; resume explicitly after verification',
    }),
  );
  return m;
}
export async function loadCheckpoint(
  c,
  id,
  directory,
  erasedKeys = new Set(),
  { verifyObjects = true, allowHistoricalImages = false } = {},
) {
  if (!/^checkpoint-[a-f0-9-]{36}$/u.test(id)) throw new Error('Invalid checkpoint id');
  const [f, media] = await Promise.all([outputs(c, 'foundation'), outputs(c, 'media')]);
  const path = join(directory, 'checkpoint.json');
  const head = await aws(c, [
    's3api',
    'head-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    `${id}/manifest.json`,
    '--expected-bucket-owner',
    c.accountId,
  ]);
  if (!head.VersionId || head.ContentLength > 33554432)
    throw new Error('Invalid checkpoint envelope');
  await aws(c, [
    's3api',
    'get-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    `${id}/manifest.json`,
    '--version-id',
    head.VersionId,
    '--expected-bucket-owner',
    c.accountId,
    path,
  ]);
  const envelope = JSON.parse(await readFile(path, 'utf8'));
  const m = validateManifest(envelope.manifest, c, f, media, { allowHistoricalImages });
  if (
    m.id !== id ||
    !m.mfaSecretArn.startsWith(`arn:aws:secretsmanager:${c.region}:${c.accountId}:secret:`)
  )
    throw new Error('Checkpoint identity mismatch');
  const result = await aws(c, [
    'kms',
    'verify',
    '--key-id',
    f.RecoverySigningKeyArn,
    '--message',
    digest(JSON.stringify(m)).toString('base64'),
    '--message-type',
    'DIGEST',
    '--signature',
    envelope.signature,
    '--signing-algorithm',
    'ECDSA_SHA_256',
    '--cli-binary-format',
    'base64',
  ]);
  if (!result.SignatureValid) throw new Error('Checkpoint signature is invalid');
  const snapshot = (
    await aws(c, ['rds', 'describe-db-snapshots', '--db-snapshot-identifier', m.snapshotId])
  ).DBSnapshots[0];
  if (
    snapshot.DBSnapshotArn !== m.snapshotArn ||
    snapshot.DBInstanceIdentifier !== f.DatabaseIdentifier ||
    !snapshot.Encrypted ||
    snapshot.Status !== 'available'
  )
    throw new Error('Checkpoint database snapshot identity or state mismatch');
  // Promotion checks every destination object against this signed manifest instead
  // of rereading all source copies before verifying short-lived ECS task status.
  for (const object of verifyObjects ? m.objects : []) {
    // Only an independently verified signed privacy journal can exempt a key.
    // Historical erasure intentionally removes these exact checkpoint versions.
    if (erasedKeys.has(object.key.slice(`${c.mediaPrefix}/`.length))) continue;
    const copy = await aws(c, [
      's3api',
      'head-object',
      '--bucket',
      media.RecoveryBucketName,
      '--key',
      object.checkpointKey,
      '--version-id',
      object.checkpointVersion,
      '--expected-bucket-owner',
      c.accountId,
      '--checksum-mode',
      'ENABLED',
    ]);
    if (copy.ContentLength !== object.size || copy.ChecksumSHA256 !== object.sha256)
      throw new Error('Checkpoint object checksum or size mismatch');
  }
  return { m, f, media };
}
export async function createStack(c, name, templateName, parameters) {
  const text = await readFile(resolve(ROOT, `deploy/aws/${templateName}`), 'utf8');
  const body = templateName.endsWith('.json') ? JSON.stringify(JSON.parse(text)) : text;
  await aws(c, [
    'cloudformation',
    'create-stack',
    '--stack-name',
    name,
    '--template-body',
    body,
    '--capabilities',
    'CAPABILITY_IAM',
    '--enable-termination-protection',
    '--parameters',
    JSON.stringify(
      Object.entries(parameters).map(([ParameterKey, v]) => ({
        ParameterKey,
        ParameterValue: String(v),
      })),
    ),
  ]);
  let stack;
  await wait(async () => {
    stack = (await aws(c, ['cloudformation', 'describe-stacks', '--stack-name', name])).Stacks[0];
    if (stack.StackStatus.includes('FAILED') || stack.StackStatus.includes('ROLLBACK'))
      throw new Error(`Recovery stack failed: ${name}; resources retained for review`);
    return stack.StackStatus === 'CREATE_COMPLETE';
  }, 'isolated recovery infrastructure');
  return Object.fromEntries(stack.Outputs.map((v) => [v.OutputKey, v.OutputValue]));
}
export async function isolatedTask(c, task, f, cluster) {
  const runtime = await outputs(c, 'runtime');
  if (runtime.ClusterArn !== cluster) throw new Error('Isolated task cluster mismatch');
  const allowed = [
    'family',
    'taskRoleArn',
    'executionRoleArn',
    'networkMode',
    'containerDefinitions',
    'volumes',
    'placementConstraints',
    'requiresCompatibilities',
    'cpu',
    'memory',
    'pidMode',
    'ipcMode',
    'proxyConfiguration',
    'inferenceAccelerators',
    'ephemeralStorage',
    'runtimePlatform',
  ];
  const request = Object.fromEntries(Object.entries(task).filter(([key]) => allowed.includes(key)));
  const registered = (
    await aws(c, ['ecs', 'register-task-definition', '--cli-input-json', JSON.stringify(request)])
  ).taskDefinition;
  const launched = await aws(c, [
    'ecs',
    'run-task',
    '--cluster',
    cluster,
    '--task-definition',
    registered.taskDefinitionArn,
    '--capacity-provider-strategy',
    JSON.stringify([{ capacityProvider: runtime.AdminCompute, weight: 1 }]),
    '--network-configuration',
    JSON.stringify({
      awsvpcConfiguration: {
        subnets: f.PrivateSubnets.split(','),
        securityGroups: [f.AppSecurityGroup],
        assignPublicIp: 'DISABLED',
      },
    }),
  ]);
  if (launched.failures?.length || launched.tasks?.length !== 1)
    throw new Error('Recovery task failed to schedule');
  const arn = launched.tasks[0].taskArn;
  await wait(async () => {
    const state = (await aws(c, ['ecs', 'describe-tasks', '--cluster', cluster, '--tasks', arn]))
      .tasks[0];
    if (state.lastStatus !== 'STOPPED') return false;
    if (state.containers.find((v) => v.name === 'app')?.exitCode !== 0)
      throw new Error(`Recovery validation task failed: ${arn}`);
    return true;
  }, 'isolated recovery verification');
  return arn;
}
async function restore(c, id, directory, journalId, acceptedRpo) {
  if (!/^checkpoint-[a-f0-9-]{36}$/u.test(id)) throw new Error('Invalid checkpoint id');
  let journal;
  if (journalId) {
    journal = await loadPrivacyJournal(c, journalId, directory);
    if (acceptedRpo !== journal.manifest.exportedAt)
      throw new Error(
        `A disaster journal has a finite recovery point. Rerun with its exact exportedAt as the final argument: ${journal.manifest.exportedAt}`,
      );
  }
  await pause(c);
  journal ??= await exportPrivacyJournal(c, directory);
  const erasedKeys = new Set(journal.payload.erased_storage_keys);
  const { m, f } = await loadCheckpoint(c, id, directory, erasedKeys, {
    allowHistoricalImages: true,
  });
  if (Date.parse(journal.manifest.exportedAt) < Date.parse(m.privacyJournalExportedAt))
    throw new Error('Privacy journal predates the checkpoint privacy watermark');
  const r = await outputs(c, 'runtime');
  const definitions = await resolveCheckpointTasks(c, m, r);
  const [provider, policy, historicalKey] = await Promise.all([
    aws(c, ['secretsmanager', 'describe-secret', '--secret-id', c.providerSecretArn]),
    aws(c, ['secretsmanager', 'describe-secret', '--secret-id', c.policySecretArn]),
    aws(c, ['secretsmanager', 'describe-secret', '--secret-id', m.mfaSecretArn]),
  ]);
  if (
    provider.DeletedDate ||
    policy.DeletedDate ||
    historicalKey.DeletedDate ||
    !provider.KmsKeyId ||
    !policy.KmsKeyId ||
    !historicalKey.VersionIdsToStages?.[m.mfaVersion]
  )
    throw new Error(
      'Required provider/policy metadata or historical encryption-key version is unavailable',
    );
  const suffix = randomUUID().slice(0, 8);
  const restoreName = `restore-${suffix}`;
  const restoreDb = `${c.deploymentName}-restore-${suffix}`;
  // New private database only; there is no endpoint/DNS/service switch in this tool.
  await aws(c, [
    'rds',
    'restore-db-instance-from-db-snapshot',
    '--db-instance-identifier',
    restoreDb,
    '--db-snapshot-identifier',
    m.snapshotArn,
    '--db-instance-class',
    c.dbInstanceClass,
    '--db-subnet-group-name',
    f.DatabaseSubnetGroup,
    '--db-parameter-group-name',
    f.DatabaseParameterGroup,
    '--ca-certificate-identifier',
    'rds-ca-rsa2048-g1',
    '--vpc-security-group-ids',
    f.DatabaseSecurityGroup,
    '--no-publicly-accessible',
    '--multi-az',
    '--deletion-protection',
    '--copy-tags-to-snapshot',
    '--backup-retention-period',
    '35',
    '--tags',
    `Key=MyFansRestore,Value=${id}`,
  ]);
  let restored;
  await wait(async () => {
    restored = (
      await aws(c, ['rds', 'describe-db-instances', '--db-instance-identifier', restoreDb])
    ).DBInstances[0];
    return restored.DBInstanceStatus === 'available';
  }, 'isolated database restore');
  if (
    restored.DBInstanceIdentifier === f.DatabaseIdentifier ||
    restored.PubliclyAccessible ||
    !restored.StorageEncrypted ||
    restored.Endpoint.Address === f.DatabaseHost
  )
    throw new Error('Restored database isolation check failed');
  // PostgreSQL snapshot restore cannot enable managed credentials in the restore call.
  // Enable them afterwards on the new instance; RDS generates and rotates the value.
  await aws(c, [
    'rds',
    'modify-db-instance',
    '--db-instance-identifier',
    restoreDb,
    '--manage-master-user-password',
    '--master-user-secret-kms-key-id',
    f.DataKeyArn,
    '--apply-immediately',
  ]);
  await wait(async () => {
    restored = (
      await aws(c, ['rds', 'describe-db-instances', '--db-instance-identifier', restoreDb])
    ).DBInstances[0];
    return (
      restored.DBInstanceStatus === 'available' &&
      restored.MasterUserSecret?.SecretStatus === 'active'
    );
  }, 'restored owner credentials');
  const media = await createStack(c, `${c.deploymentName}-${restoreName}-media`, 'media.yaml', {
    DeploymentName: restoreName,
    MediaPrefix: c.mediaPrefix,
    AlertTopicArn: f.AlarmTopicArn,
  });
  if (media.MediaBucketName === m.mediaBucket)
    throw new Error('Recovery destination must differ from the original bucket');
  const occupied = await aws(c, [
    's3api',
    'list-objects-v2',
    '--bucket',
    media.MediaBucketName,
    '--max-keys',
    '1',
    '--expected-bucket-owner',
    c.accountId,
  ]);
  if (occupied.KeyCount) throw new Error('Recovery destination must be empty');
  for (const object of m.objects) {
    if (erasedKeys.has(object.key.slice(`${c.mediaPrefix}/`.length))) continue;
    const copied = await aws(c, [
      's3api',
      'copy-object',
      '--bucket',
      media.MediaBucketName,
      '--key',
      object.key,
      '--copy-source',
      copySource(m.recoveryBucket, object.checkpointKey, object.checkpointVersion),
      '--expected-bucket-owner',
      c.accountId,
      '--expected-source-bucket-owner',
      c.accountId,
      '--server-side-encryption',
      'aws:kms',
      '--ssekms-key-id',
      media.MediaEncryptionKeyArn,
      '--checksum-algorithm',
      'SHA256',
    ]);
    if (copied.CopyObjectResult.ChecksumSHA256 !== object.sha256)
      throw new Error('Restored object checksum mismatch');
  }
  const runtimeDefinition = definitions.WorkerTask;
  const migrationDefinition = definitions.MigrationTask;
  const repo = (uri) =>
    `arn:aws:ecr:${c.region}:${c.accountId}:repository/${uri.split('.amazonaws.com/')[1].split('@')[0]}`;
  const access = await createStack(
    c,
    `${c.deploymentName}-${restoreName}-access`,
    'recovery-access.json',
    {
      DeploymentName: c.deploymentName,
      RestoreName: restoreName,
      DataKeyArn: f.DataKeyArn,
      SecretArns: [
        restored.MasterUserSecret.SecretArn,
        f.RuntimePasswordArn,
        m.mfaSecretArn,
        c.providerSecretArn,
        c.policySecretArn,
      ].join(','),
      SecretKeyArns: [...new Set([f.DataKeyArn, provider.KmsKeyId, policy.KmsKeyId])].join(','),
      ImageRepositoryArn: repo(m.imageUri),
      ScannerRepositoryArn: repo(m.scannerImageUri),
      PrivacyJournalBucket: journal.manifest.bucket,
      PrivacyJournalObjectKey: journal.manifest.key,
      PrivacyJournalVersion: journal.manifest.objectVersion,
      PrivacyJournalKeyArn: (await outputs(c, 'media')).RecoveryEncryptionKeyArn,
    },
  );
  const prepare = (base, mode) => {
    const task = structuredClone(base);
    task.family = `${restoreName}-${mode}`;
    task.executionRoleArn = access.ExecutionRoleArn;
    for (const container of task.containerDefinitions)
      container.logConfiguration.options['awslogs-group'] = access.LogGroup;
    const app = task.containerDefinitions.find((v) => v.name === 'app');
    app.command = [mode];
    delete app.healthCheck;
    const env = Object.fromEntries(app.environment.map((v) => [v.name, v.value]));
    env.CLOUD_DATABASE_HOST = restored.Endpoint.Address;
    env.S3_BUCKET = media.MediaBucketName;
    env.S3_KMS_KEY_ID = media.MediaEncryptionKeyArn;
    env.APP_RUN_MIGRATIONS = 'false';
    app.environment = Object.entries(env).map(([name, value]) => ({ name, value }));
    return { task, app, env };
  };
  const credentials = prepare(migrationDefinition, 'migrate');
  credentials.app.secrets.find((v) => v.name === 'CLOUD_DATABASE_PASSWORD').valueFrom =
    `${restored.MasterUserSecret.SecretArn}:password::`;
  credentials.app.secrets.find((v) => v.name === 'CLOUD_RUNTIME_PASSWORD').valueFrom =
    f.RuntimePasswordArn;
  const migrationTask = await isolatedTask(c, credentials.task, f, r.ClusterArn);
  const administrative = prepare(migrationDefinition, 'rebind-storage');
  administrative.app.secrets.find((v) => v.name === 'CLOUD_DATABASE_PASSWORD').valueFrom =
    `${restored.MasterUserSecret.SecretArn}:password::`;
  administrative.app.environment.push(
    ...Object.entries({
      RECOVERY_REBIND: 'true',
      RECOVERY_DATABASE_HOST: restored.Endpoint.Address,
      EXPECTED_PREVIOUS_STORAGE_ID: storageIdentity(
        c.region,
        c.accountId,
        m.mediaBucket,
        c.mediaPrefix,
      ),
      RESTORE_STORAGE_ID: storageIdentity(
        c.region,
        c.accountId,
        media.MediaBucketName,
        c.mediaPrefix,
      ),
    }).map(([name, value]) => ({ name, value })),
  );
  const rebindTask = await isolatedTask(c, administrative.task, f, r.ClusterArn);
  const replay = prepare(migrationDefinition, 'privacy-replay');
  replay.task.taskRoleArn = access.JournalReplayRoleArn;
  replay.app.secrets = replay.app.secrets.filter((v) => v.name === 'CLOUD_DATABASE_PASSWORD');
  replay.app.secrets[0].valueFrom = `${restored.MasterUserSecret.SecretArn}:password::`;
  replay.app.environment.push(
    ...Object.entries({
      PRIVACY_JOURNAL_BUCKET: journal.manifest.bucket,
      PRIVACY_JOURNAL_KEY: journal.manifest.key,
      PRIVACY_JOURNAL_VERSION: journal.manifest.objectVersion,
      PRIVACY_JOURNAL_SHA256: journal.manifest.sha256,
      PRIVACY_SOURCE_STORAGE_ID: journal.manifest.sourceStorageId,
      PRIVACY_SOURCE_DATABASE_ID: journal.manifest.databaseIdentifier,
      RECOVERY_DATABASE_HOST: restored.Endpoint.Address,
      S3_EXPECTED_BUCKET_OWNER: c.accountId,
      AWS_REGION: c.region,
    }).map(([name, value]) => ({ name, value })),
  );
  const privacyReplayTask = await isolatedTask(c, replay.task, f, r.ClusterArn);
  const verify = prepare(runtimeDefinition, 'verify-storage');
  verify.task.taskRoleArn = media.WorkloadRoleArn;
  verify.app.secrets.find((v) => v.name === 'CLOUD_DATABASE_PASSWORD').valueFrom =
    f.RuntimePasswordArn;
  verify.app.secrets.find((v) => v.name === 'CLOUD_MFA_KEY_HEX').valueFrom =
    `${m.mfaSecretArn}:::${m.mfaVersion}`;
  const verificationTask = await isolatedTask(c, verify.task, f, r.ClusterArn);
  const evidence = {
    checkpoint: id,
    databaseIdentifier: restoreDb,
    databaseHost: restored.Endpoint.Address,
    ownerSecretArn: restored.MasterUserSecret.SecretArn,
    media,
    rebindTask,
    migrationTask,
    privacyReplayTask,
    privacyJournal: journal.manifest,
    excludedErasedObjects: m.objects.filter((v) =>
      erasedKeys.has(v.key.slice(`${c.mediaPrefix}/`.length)),
    ).length,
    privacyRecoveryPointExplicitlyAccepted: Boolean(journalId),
    verificationTask,
    verifiedAt: new Date().toISOString(),
    liveServicesModified: true,
    sourceObjectsModified: false,
  };
  const evidencePath = resolve(ROOT, `.local/aws/${c.deploymentName}/${restoreName}.json`);
  await mkdir(dirname(evidencePath), { recursive: true, mode: 0o700 });
  await writeFile(evidencePath, JSON.stringify(evidence, null, 2), { mode: 0o600 });
  console.log(
    JSON.stringify({
      evidencePath,
      databaseIdentifier: restoreDb,
      mediaBucket: media.MediaBucketName,
      verificationTask,
      note: 'Isolated recovery and signed privacy replay verified. Source services remain paused. Resources are retained; no traffic was switched.',
    }),
  );
}
async function main() {
  const [command, configPath, id, journalId, acceptedRpo] = process.argv.slice(2);
  const c = validateConfiguration(JSON.parse(await readFile(configPath, 'utf8')));
  await identity(c);
  const directory = await mkdtemp(join(tmpdir(), 'myfans-recovery-'));
  try {
    if (command === 'checkpoint') return await checkpoint(c, directory);
    if (command === 'privacy-journal') {
      await pause(c);
      return await exportPrivacyJournal(c, directory);
    }
    if (command === 'verify') {
      const journal = journalId ? await loadPrivacyJournal(c, journalId, directory) : undefined;
      const { m } = await loadCheckpoint(
        c,
        id,
        directory,
        new Set(journal?.payload.erased_storage_keys ?? []),
        { allowHistoricalImages: true },
      );
      console.log(
        JSON.stringify({
          checkpoint: id,
          objects: m.objects.length,
          signatureValid: true,
          checksumsValid: true,
          imageUri: m.imageUri,
          scannerImageUri: m.scannerImageUri,
          taskDefinitions: m.taskDefinitions || null,
        }),
      );
      return;
    }
    if (command === 'restore') return await restore(c, id, directory, journalId, acceptedRpo);
    throw new Error(
      'Usage: node scripts/recover-aws.mjs checkpoint|privacy-journal|verify|restore CONFIG [CHECKPOINT_ID] [SIGNED_PRIVACY_JOURNAL_ID] [EXACT_JOURNAL_EXPORTED_AT]',
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((e) => {
    console.error(e.message);
    process.exitCode = 1;
  });
