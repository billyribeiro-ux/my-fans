import { createHash, randomUUID } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { aws, outputs, assertRuntimeBinding } from './deploy-aws.mjs';
import { isolatedTask, storageIdentity } from './recover-aws.mjs';

const hash = (bytes) => createHash('sha256').update(bytes).digest();
const journalId = /^privacy-[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/u;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/u;
const key =
  /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}\.(?:upload|chunk|jpg|preview\.jpg|mp4|m3u8|720p\.m3u8|360p\.m3u8|(?:720p|360p)-(?:000\d\d|001[0-4]\d|0015[01])\.ts)$/u;

export function validatePrivacyJournal(document, c, f, media) {
  if (
    document.version !== 1 ||
    document.source_database_identifier !== f.DatabaseIdentifier ||
    document.source_storage_id !==
      storageIdentity(c.region, c.accountId, media.MediaBucketName, c.mediaPrefix) ||
    !Number.isFinite(Date.parse(document.exported_at)) ||
    Date.parse(document.exported_at) > Date.now() + 60000 ||
    !Array.isArray(document.requests) ||
    document.requests.length > 100000 ||
    !Array.isArray(document.erased_storage_keys) ||
    document.erased_storage_keys.length > 100000
  )
    throw new Error('Privacy journal source, time or bounds are invalid');
  const users = new Set();
  const requests = new Set();
  for (const r of document.requests) {
    const erased = ['application_erased', 'completed'].includes(r.status);
    if (
      !uuid.test(r.id) ||
      !uuid.test(r.user_id) ||
      users.has(r.user_id) ||
      requests.has(r.id) ||
      !['pending', 'approved', 'purging', 'application_erased', 'completed'].includes(r.status) ||
      typeof r.legal_hold !== 'boolean' ||
      !r.retention ||
      typeof r.privacy_version !== 'string' ||
      !r.privacy_version ||
      r.privacy_version.length > 100 ||
      /[\x00-\x1f\x7f]/u.test(r.privacy_version) ||
      !Number.isFinite(Date.parse(r.created_at)) ||
      !Number.isFinite(Date.parse(r.not_before)) ||
      Date.parse(r.created_at) > Date.parse(document.exported_at) ||
      Date.parse(r.not_before) < Date.parse(r.created_at) ||
      erased !== (r.application_erased_at != null) ||
      (r.status === 'completed') !== (r.completed_at != null) ||
      (erased &&
        (!Number.isFinite(Date.parse(r.application_erased_at)) ||
          Date.parse(r.application_erased_at) < Date.parse(r.created_at) ||
          Date.parse(r.application_erased_at) > Date.parse(document.exported_at))) ||
      (r.completed_at != null &&
        (!Number.isFinite(Date.parse(r.completed_at)) ||
          Date.parse(r.completed_at) < Date.parse(r.application_erased_at) ||
          Date.parse(r.completed_at) > Date.parse(document.exported_at)))
    )
      throw new Error('Privacy journal contains an invalid or duplicate request');
    for (const name of [
      'security_days',
      'financial_days',
      'policy_evidence_days',
      'moderation_evidence_days',
    ])
      if (
        !Number.isSafeInteger(r.retention[name]) ||
        r.retention[name] < 1 ||
        r.retention[name] > 36500
      )
        throw new Error('Privacy journal evidence schedule is invalid');
    if (
      !Number.isSafeInteger(r.retention.erasure_delay_days) ||
      r.retention.erasure_delay_days < 0 ||
      r.retention.erasure_delay_days > 365 ||
      !Number.isSafeInteger(r.retention.backup_days) ||
      r.retention.backup_days < 35 ||
      r.retention.backup_days > 3650
    )
      throw new Error('Privacy journal erasure schedule is invalid');
    users.add(r.user_id);
    requests.add(r.id);
  }
  if (
    new Set(document.erased_storage_keys).size !== document.erased_storage_keys.length ||
    document.erased_storage_keys.some((v) => typeof v !== 'string' || !key.test(v))
  )
    throw new Error('Privacy journal contains an unsafe or duplicate media key');
  return document;
}

// Call only after draining all services. ECS injects the owner credential; this
// operator process reads only the deliberately exported, encrypted journal.
export async function exportPrivacyJournal(c, directory) {
  await assertRuntimeBinding(c);
  const [f, r, media, access] = await Promise.all([
    outputs(c, 'foundation'),
    outputs(c, 'runtime'),
    outputs(c, 'media'),
    outputs(c, 'privacy'),
  ]);
  const id = `privacy-${randomUUID()}`;
  const objectKey = `privacy-journals/${id}/journal.json`;
  const task = (
    await aws(c, ['ecs', 'describe-task-definition', '--task-definition', r.MigrationTask])
  ).taskDefinition;
  const app = task.containerDefinitions.find((v) => v.name === 'app');
  if (app?.image !== c.imageUri)
    throw new Error('Privacy export image differs from the configured release');
  task.family = `${c.deploymentName}-privacy-export`;
  task.executionRoleArn = access.ExecutionRoleArn;
  task.taskRoleArn = access.JournalExportRoleArn;
  for (const container of task.containerDefinitions)
    container.logConfiguration.options['awslogs-group'] = access.LogGroup;
  app.command = ['privacy-export'];
  app.secrets = app.secrets.filter((v) => v.name === 'CLOUD_DATABASE_PASSWORD');
  const env = Object.fromEntries(app.environment.map((v) => [v.name, v.value]));
  Object.assign(env, {
    APP_RUN_MIGRATIONS: 'false',
    PRIVACY_JOURNAL_BUCKET: media.RecoveryBucketName,
    PRIVACY_JOURNAL_KEY: objectKey,
    PRIVACY_JOURNAL_KMS_KEY_ID: media.RecoveryEncryptionKeyArn,
    PRIVACY_SOURCE_DATABASE_ID: f.DatabaseIdentifier,
    S3_EXPECTED_BUCKET_OWNER: c.accountId,
    AWS_REGION: c.region,
  });
  app.environment = Object.entries(env).map(([name, value]) => ({ name, value }));
  const exportTask = await isolatedTask(c, task, f, r.ClusterArn);
  const head = await aws(c, [
    's3api',
    'head-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    objectKey,
    '--expected-bucket-owner',
    c.accountId,
  ]);
  if (!head.VersionId || head.VersionId === 'null' || head.ContentLength > 33554432)
    throw new Error('Privacy export lacks a bounded immutable object version');
  const path = join(directory, 'privacy-journal.json');
  await aws(c, [
    's3api',
    'get-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    objectKey,
    '--version-id',
    head.VersionId,
    '--expected-bucket-owner',
    c.accountId,
    path,
  ]);
  const bytes = await readFile(path);
  const payload = validatePrivacyJournal(JSON.parse(bytes.toString('utf8')), c, f, media);
  if (Date.now() - Date.parse(payload.exported_at) > 3600000)
    throw new Error('Newly exported privacy journal is unexpectedly stale');
  const manifest = {
    version: 1,
    id,
    accountId: c.accountId,
    region: c.region,
    deploymentName: c.deploymentName,
    databaseIdentifier: f.DatabaseIdentifier,
    sourceStorageId: payload.source_storage_id,
    exportedAt: payload.exported_at,
    bucket: media.RecoveryBucketName,
    key: objectKey,
    objectVersion: head.VersionId,
    sha256: hash(bytes).toString('hex'),
    signingKeyArn: f.RecoverySigningKeyArn,
    exportTask,
  };
  const signed = await aws(c, [
    'kms',
    'sign',
    '--key-id',
    f.RecoverySigningKeyArn,
    '--message',
    hash(JSON.stringify(manifest)).toString('base64'),
    '--message-type',
    'DIGEST',
    '--signing-algorithm',
    'ECDSA_SHA_256',
    '--cli-binary-format',
    'base64',
  ]);
  const envelopePath = join(directory, 'privacy-envelope.json');
  await writeFile(envelopePath, JSON.stringify({ manifest, signature: signed.Signature }), {
    mode: 0o600,
  });
  await aws(c, [
    's3api',
    'put-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    `privacy-journals/${id}/envelope.json`,
    '--body',
    envelopePath,
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
      privacyJournal: id,
      exportedAt: payload.exported_at,
      requests: payload.requests.length,
      erasedObjects: payload.erased_storage_keys.length,
      exportTask,
      services: 'paused',
    }),
  );
  return { manifest, payload };
}

export async function loadPrivacyJournal(c, id, directory) {
  if (!journalId.test(id)) throw new Error('An exact signed privacy journal ID is required');
  const [f, media] = await Promise.all([outputs(c, 'foundation'), outputs(c, 'media')]);
  const path = join(directory, 'privacy-envelope.json');
  const objectKey = `privacy-journals/${id}/envelope.json`;
  const head = await aws(c, [
    's3api',
    'head-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    objectKey,
    '--expected-bucket-owner',
    c.accountId,
  ]);
  if (!head.VersionId || head.ContentLength > 32768)
    throw new Error('Invalid signed privacy envelope');
  await aws(c, [
    's3api',
    'get-object',
    '--bucket',
    media.RecoveryBucketName,
    '--key',
    objectKey,
    '--version-id',
    head.VersionId,
    '--expected-bucket-owner',
    c.accountId,
    path,
  ]);
  const envelope = JSON.parse(await readFile(path, 'utf8'));
  const m = envelope.manifest;
  if (
    m.version !== 1 ||
    m.id !== id ||
    m.accountId !== c.accountId ||
    m.region !== c.region ||
    m.deploymentName !== c.deploymentName ||
    m.databaseIdentifier !== f.DatabaseIdentifier ||
    m.sourceStorageId !==
      storageIdentity(c.region, c.accountId, media.MediaBucketName, c.mediaPrefix) ||
    m.signingKeyArn !== f.RecoverySigningKeyArn ||
    m.bucket !== media.RecoveryBucketName ||
    m.key !== `privacy-journals/${id}/journal.json` ||
    !m.objectVersion ||
    m.objectVersion === 'null' ||
    !/^[a-f0-9]{64}$/u.test(m.sha256)
  )
    throw new Error('Privacy envelope is not bound to this deployment');
  const verified = await aws(c, [
    'kms',
    'verify',
    '--key-id',
    f.RecoverySigningKeyArn,
    '--message',
    hash(JSON.stringify(m)).toString('base64'),
    '--message-type',
    'DIGEST',
    '--signature',
    envelope.signature,
    '--signing-algorithm',
    'ECDSA_SHA_256',
    '--cli-binary-format',
    'base64',
  ]);
  if (!verified.SignatureValid) throw new Error('Privacy journal signature is invalid');
  const object = await aws(c, [
    's3api',
    'head-object',
    '--bucket',
    m.bucket,
    '--key',
    m.key,
    '--version-id',
    m.objectVersion,
    '--expected-bucket-owner',
    c.accountId,
  ]);
  if (object.ContentLength > 33554432) throw new Error('Privacy journal exceeds its bound');
  const documentPath = join(directory, 'privacy-journal.json');
  await aws(c, [
    's3api',
    'get-object',
    '--bucket',
    m.bucket,
    '--key',
    m.key,
    '--version-id',
    m.objectVersion,
    '--expected-bucket-owner',
    c.accountId,
    documentPath,
  ]);
  const bytes = await readFile(documentPath);
  if (hash(bytes).toString('hex') !== m.sha256)
    throw new Error('Privacy journal content hash differs from its signature');
  const payload = validatePrivacyJournal(JSON.parse(bytes.toString('utf8')), c, f, media);
  if (payload.exported_at !== m.exportedAt)
    throw new Error('Privacy export time differs from its signature');
  return { manifest: m, payload };
}
