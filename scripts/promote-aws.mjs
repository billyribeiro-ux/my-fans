import { createHash } from 'node:crypto';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  aws,
  identity,
  outputs,
  validateConfiguration,
  validateRecoveredDatabase,
} from './deploy-aws.mjs';
import { loadCheckpoint, storageIdentity } from './recover-aws.mjs';
import { loadPrivacyJournal } from './privacy-journal-aws.mjs';

const reference = /^[A-Za-z0-9_.:/-]{8,200}$/u;
const taskModes = {
  migrationTask: 'migrate',
  rebindTask: 'rebind-storage',
  privacyReplayTask: 'privacy-replay',
  verificationTask: 'verify-storage',
};
export function validatePromotionEvidence(c, evidence, acceptance) {
  const suffix = evidence.databaseIdentifier?.slice(`${c.deploymentName}-restore-`.length);
  if (
    !/^[a-f0-9]{8}$/u.test(suffix) ||
    evidence.databaseIdentifier !== `${c.deploymentName}-restore-${suffix}` ||
    !evidence.databaseHost?.startsWith(`${evidence.databaseIdentifier}.`) ||
    !evidence.databaseHost.endsWith('.rds.amazonaws.com') ||
    !/^checkpoint-[a-f0-9-]{36}$/u.test(evidence.checkpoint) ||
    !evidence.ownerSecretArn?.startsWith(
      `arn:aws:secretsmanager:${c.region}:${c.accountId}:secret:`,
    ) ||
    !evidence.privacyJournal ||
    evidence.privacyJournal.accountId !== c.accountId ||
    evidence.privacyJournal.deploymentName !== c.deploymentName ||
    evidence.privacyJournal.region !== c.region ||
    evidence.sourceObjectsModified !== false ||
    evidence.liveServicesModified !== true
  )
    throw new Error('Restore evidence is not bound to this account and isolated deployment');
  for (const key of Object.keys(taskModes))
    if (
      !new RegExp(
        `^arn:aws:ecs:${c.region}:${c.accountId}:task/[A-Za-z0-9_-]+/[a-f0-9]{32}$`,
        'u',
      ).test(evidence[key])
    )
      throw new Error('Restore evidence requires exact same-account task ARNs');
  if (
    !acceptance ||
    acceptance.checkpoint !== evidence.checkpoint ||
    acceptance.privacyJournalId !== evidence.privacyJournal.id ||
    acceptance.privacyExportedAt !== evidence.privacyJournal.exportedAt ||
    acceptance.recoveryPointAccepted !== true ||
    !['changeReference', 'providerReconciliationReference', 'privacyReconciliationReference'].every(
      (k) => reference.test(acceptance[k]),
    )
  )
    throw new Error(
      'Supply reviewed change, provider reconciliation, privacy reconciliation, and exact recovery-point acceptance references',
    );
  return suffix;
}
export function validateRestoreTask(task, definition, key, c, evidence, journal, media) {
  const app = definition?.containerDefinitions?.find((v) => v.name === 'app');
  const env = Object.fromEntries((app?.environment || []).map((v) => [v.name, v.value]));
  const secret = Object.fromEntries((app?.secrets || []).map((v) => [v.name, v.valueFrom]));
  if (
    !task ||
    task.taskArn !== evidence[key] ||
    task.taskDefinitionArn !== definition?.taskDefinitionArn ||
    task.lastStatus !== 'STOPPED' ||
    task.containers?.find((v) => v.name === 'app')?.exitCode !== 0 ||
    app?.image !== c.imageUri ||
    app.entryPoint?.length !== 2 ||
    app.entryPoint[0] !== 'node' ||
    app.entryPoint[1] !== '/app/scripts/cloud-entrypoint.mjs' ||
    definition.containerDefinitions.some((v) => v.environmentFiles?.length) ||
    task.overrides?.containerOverrides?.some(
      (v) =>
        Object.hasOwn(v, 'command') ||
        ['environment', 'environmentFiles'].some((field) => v[field]?.length),
    ) ||
    app.command?.length !== 1 ||
    app.command[0] !== taskModes[key] ||
    env.CLOUD_DATABASE_HOST !== evidence.databaseHost ||
    env.S3_BUCKET !== media.MediaBucketName ||
    (key !== 'verificationTask' &&
      secret.CLOUD_DATABASE_PASSWORD !== `${evidence.ownerSecretArn}:password::`)
  )
    throw new Error(`Restore task evidence is incomplete or mismatched: ${key}`);
  if (
    key === 'privacyReplayTask' &&
    (env.PRIVACY_JOURNAL_SHA256 !== journal.sha256 ||
      env.PRIVACY_JOURNAL_VERSION !== journal.objectVersion ||
      env.PRIVACY_JOURNAL_BUCKET !== journal.bucket ||
      env.PRIVACY_JOURNAL_KEY !== journal.key ||
      env.PRIVACY_SOURCE_DATABASE_ID !== journal.databaseIdentifier ||
      env.PRIVACY_SOURCE_STORAGE_ID !== journal.sourceStorageId ||
      env.RECOVERY_DATABASE_HOST !== evidence.databaseHost)
  )
    throw new Error('Privacy replay task does not match the independently verified signed journal');
  return { app, env, secret };
}

async function main() {
  const [command, configPath, evidencePath, acceptancePath, destinationPath] =
    process.argv.slice(2);
  if (command !== 'prepare' || !destinationPath || resolve(configPath) === resolve(destinationPath))
    throw new Error(
      'Usage: node scripts/promote-aws.mjs prepare SOURCE_CONFIG RESTORE_EVIDENCE ACCEPTANCE_JSON NEW_CONFIG',
    );
  const source = await readFile(configPath, 'utf8');
  const c = validateConfiguration(JSON.parse(source));
  const evidenceBytes = await readFile(evidencePath);
  const evidence = JSON.parse(evidenceBytes);
  const acceptance = JSON.parse(await readFile(acceptancePath, 'utf8'));
  const suffix = validatePromotionEvidence(c, evidence, acceptance);
  await identity(c);
  const [f, originalFoundation, r, originalMedia] = await Promise.all([
    outputs(c, 'foundation'),
    outputs(c, 'foundation', { original: true }),
    outputs(c, 'runtime'),
    outputs(c, 'media'),
  ]);
  const services = await aws(c, [
    'ecs',
    'describe-services',
    '--cluster',
    r.ClusterArn,
    '--services',
    r.WebService,
    r.WorkerService,
  ]);
  if (
    services.failures?.length ||
    services.services?.length !== 2 ||
    services.services.some((v) => v.desiredCount || v.runningCount || v.pendingCount)
  )
    throw new Error('Promotion preparation requires the source services to remain fully drained');
  const directory = await mkdtemp(join(tmpdir(), 'myfans-promotion-'));
  try {
    const journal = await loadPrivacyJournal(c, evidence.privacyJournal.id, directory);
    if (JSON.stringify(journal.manifest) !== JSON.stringify(evidence.privacyJournal))
      throw new Error('Restore evidence journal differs from its independently signed envelope');
    const erased = new Set(journal.payload.erased_storage_keys);
    const { m } = await loadCheckpoint(c, evidence.checkpoint, directory, erased, {
      verifyObjects: false,
      allowHistoricalImages: true,
    });
    if (Date.parse(journal.manifest.exportedAt) < Date.parse(m.privacyJournalExportedAt))
      throw new Error('Privacy journal predates the checkpoint watermark');
    const candidate = {
      ...c,
      imageUri: m.imageUri,
      scannerImageUri: m.scannerImageUri,
      mfaSecretArn: m.mfaSecretArn,
      mfaSecretVersion: m.mfaVersion,
      activeRecovery: {
        suffix,
        databaseIdentifier: evidence.databaseIdentifier,
        mediaStackName: `${c.deploymentName}-restore-${suffix}-media`,
        privacyStackName: `${c.deploymentName}-restore-${suffix}-privacy`,
        checkpoint: evidence.checkpoint,
        privacyJournalId: journal.manifest.id,
        evidenceSha256: createHash('sha256').update(evidenceBytes).digest('hex'),
        sourceConfigurationSha256: createHash('sha256').update(source).digest('hex'),
        preparedAt: new Date().toISOString(),
        acceptance,
      },
    };
    validateConfiguration(candidate);
    const db = (
      await aws(c, [
        'rds',
        'describe-db-instances',
        '--db-instance-identifier',
        evidence.databaseIdentifier,
      ])
    ).DBInstances[0];
    const recovered = validateRecoveredDatabase(db, candidate, originalFoundation);
    if (
      recovered.DatabaseHost !== evidence.databaseHost ||
      recovered.DatabaseOwnerSecretArn !== evidence.ownerSecretArn ||
      recovered.DatabaseIdentifier === f.DatabaseIdentifier
    )
      throw new Error(
        'Recovered database identity changed or matches the currently active database',
      );
    const media = await outputs(candidate, 'media');
    for (const key of [
      'MediaBucketName',
      'MediaEncryptionKeyArn',
      'RecoveryBucketName',
      'RecoveryEncryptionKeyArn',
      'WorkloadRoleArn',
    ])
      if (!media[key] || media[key] !== evidence.media[key] || media[key] === originalMedia[key])
        throw new Error(
          'Recovered media stack differs from restore evidence or the original resources',
        );
    const keyMetadata = await aws(c, [
      'secretsmanager',
      'describe-secret',
      '--secret-id',
      m.mfaSecretArn,
    ]);
    if (keyMetadata.DeletedDate || !keyMetadata.VersionIdsToStages?.[m.mfaVersion])
      throw new Error('The exact checkpoint MFA key version must remain retained');
    for (const key of Object.keys(taskModes)) {
      const state = (
        await aws(c, ['ecs', 'describe-tasks', '--cluster', r.ClusterArn, '--tasks', evidence[key]])
      ).tasks?.[0];
      if (!state)
        throw new Error(
          'AWS no longer retains restore task status; repeat the isolated restore to obtain fresh verifiable evidence',
        );
      const definition = (
        await aws(c, [
          'ecs',
          'describe-task-definition',
          '--task-definition',
          state.taskDefinitionArn,
        ])
      ).taskDefinition;
      const result = validateRestoreTask(
        state,
        definition,
        key,
        candidate,
        evidence,
        journal.manifest,
        media,
      );
      if (
        key === 'rebindTask' &&
        (result.env.EXPECTED_PREVIOUS_STORAGE_ID !== journal.manifest.sourceStorageId ||
          result.env.RESTORE_STORAGE_ID !==
            storageIdentity(c.region, c.accountId, media.MediaBucketName, c.mediaPrefix))
      )
        throw new Error(
          'Rebinding task did not use the signed source and recovered target storage identities',
        );
      if (
        key === 'verificationTask' &&
        result.secret.CLOUD_MFA_KEY_HEX !== `${m.mfaSecretArn}:::${m.mfaVersion}`
      )
        throw new Error('Ciphertext verification did not use the exact historical key version');
    }
    // Check the destination, not just the retained checkpoint copies. Excluded erased
    // objects must not have a current version in the promoted bucket.
    const expected = new Map(
      m.objects
        .filter((v) => !erased.has(v.key.slice(`${c.mediaPrefix}/`.length)))
        .map((v) => [v.key, v]),
    );
    let token;
    let seen = 0;
    do {
      const args = [
        's3api',
        'list-objects-v2',
        '--bucket',
        media.MediaBucketName,
        '--expected-bucket-owner',
        c.accountId,
        '--max-keys',
        '1000',
        '--no-paginate',
      ];
      if (token) args.push('--continuation-token', token);
      const page = await aws(c, args);
      for (const object of page.Contents || []) {
        const wanted = expected.get(object.Key);
        if (!wanted || object.Size !== wanted.size)
          throw new Error('Recovered bucket contains unexpected or changed current objects');
        const actual = await aws(c, [
          's3api',
          'head-object',
          '--bucket',
          media.MediaBucketName,
          '--key',
          object.Key,
          '--expected-bucket-owner',
          c.accountId,
          '--checksum-mode',
          'ENABLED',
        ]);
        if (
          actual.ChecksumSHA256 !== wanted.sha256 ||
          actual.ServerSideEncryption !== 'aws:kms' ||
          actual.SSEKMSKeyId !== media.MediaEncryptionKeyArn
        )
          throw new Error('Recovered object checksum or encryption differs from the signed target');
        expected.delete(object.Key);
        seen++;
      }
      token = page.IsTruncated ? page.NextContinuationToken : undefined;
      if (page.IsTruncated && !token) throw new Error('Invalid restored bucket pagination');
    } while (token);
    if (expected.size) throw new Error('Recovered bucket is missing signed, non-erased objects');
    await writeFile(destinationPath, `${JSON.stringify(candidate, null, 2)}\n`, {
      mode: 0o600,
      flag: 'wx',
    });
    console.log(
      JSON.stringify({
        candidateConfiguration: resolve(destinationPath),
        databaseIdentifier: recovered.DatabaseIdentifier,
        mediaBucket: media.MediaBucketName,
        objects: seen,
        services: 'paused; no infrastructure or traffic changed',
        next: 'Create candidate privacy permissions; review/apply candidate runtime change set; migrate; doctor; record acceptance; explicitly start with the candidate configuration.',
      }),
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
