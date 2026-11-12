import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  reloadPolicy,
  verifyPublicPolicy,
  validateConfiguration,
  validateRecoveredDatabase,
} from './deploy-aws.mjs';
import { validatePromotionEvidence, validateRestoreTask } from './promote-aws.mjs';
import {
  checkpointManifestBytes,
  resolveCheckpointTasks,
  storageIdentity,
  validateManifest,
} from './recover-aws.mjs';
import { validatePrivacyJournal } from './privacy-journal-aws.mjs';

const c = {
  accountId: '123456789012',
  region: 'us-east-1',
  deploymentName: 'myfans-prod',
  hostedZoneId: 'Z12345678',
  domainName: 'fans.example.com',
  originDomainName: 'origin-fans.example.com',
  imageUri: `123456789012.dkr.ecr.us-east-1.amazonaws.com/app@sha256:${'a'.repeat(64)}`,
  scannerImageUri: `123456789012.dkr.ecr.us-east-1.amazonaws.com/scan@sha256:${'b'.repeat(64)}`,
  providerSecretArn: 'arn:aws:secretsmanager:us-east-1:123456789012:secret:provider-123456',
  policySecretArn: 'arn:aws:secretsmanager:us-east-1:123456789012:secret:policy-123456',
  stripeAccountCountry: 'US',
  mediaPrefix: 'myfans',
  webDesiredCount: 2,
  workerDesiredCount: 2,
  alarmEmail: 'operations@example.com',
  dbInstanceClass: 'db.r8g.large',
};
const f = { DatabaseIdentifier: 'myfans-prod', RecoverySigningKeyArn: 'trusted-signer' };
const media = { MediaBucketName: 'primary', RecoveryBucketName: 'recovery' };
const id = 'checkpoint-12345678-1234-1234-1234-123456789abc';
const m = {
  version: 1,
  id,
  accountId: c.accountId,
  region: c.region,
  deploymentName: c.deploymentName,
  databaseIdentifier: f.DatabaseIdentifier,
  mediaBucket: media.MediaBucketName,
  recoveryBucket: media.RecoveryBucketName,
  signingKeyArn: f.RecoverySigningKeyArn,
  mediaPrefix: c.mediaPrefix,
  imageUri: c.imageUri,
  scannerImageUri: c.scannerImageUri,
  snapshotId: 'myfans-snapshot',
  mfaVersion: '12345678-1234-1234-1234-123456789abc',
  privacyJournalId: 'privacy-12345678-1234-1234-1234-123456789abc',
  privacyJournalExportedAt: '2026-10-07T00:00:00Z',
  objects: [
    {
      key: 'myfans/asset',
      checkpointKey: `${id}/objects/myfans/asset`,
      checkpointVersion: 'immutable-version',
      sha256: Buffer.alloc(32).toString('base64'),
      size: 12,
    },
  ],
};
const approvedPolicy = {
  eligibility: {
    minimum_age: 18,
    registration_enabled: false,
    countries: [{ code: 'US', name: 'United States' }],
  },
  operator: { legal_name: 'Approved operator' },
  policies: [{ kind: 'terms', version: 'v2' }],
  retention: { backup_days: 35 },
};
function policyFixture(change = {}) {
  const calls = [];
  const r = {
    ClusterArn: 'cluster',
    WebService: 'web',
    WorkerService: 'worker',
    WebTask: 'web:3',
    WorkerTask: 'worker:3',
  };
  const state = ['web', 'worker'].map((name, index) => ({
    serviceArn: name,
    status: 'ACTIVE',
    taskDefinition: `${name}:3`,
    desiredCount: index + 2,
    runningCount: index + 2,
    pendingCount: 0,
    deployments: [{ id: `${name}-old`, status: 'PRIMARY', rolloutState: 'COMPLETED' }],
  }));
  let reads = 0;
  const operations = {
    assertRuntimeBinding: async () => {
      calls.push(['binding']);
    },
    outputs: async () => r,
    aws: async (_c, args) => {
      calls.push(args);
      if (args[0] === 'secretsmanager')
        return {
          ARN: c.policySecretArn,
          VersionIdsToStages: {
            [change.versionRace && reads++ ? 'newer' : 'approved']: ['AWSCURRENT'],
          },
        };
      if (args[1] === 'describe-task-definition')
        return {
          taskDefinition: {
            taskDefinitionArn: args.at(-1),
            containerDefinitions: [
              {
                name: 'app',
                secrets: [
                  {
                    name: 'CLOUD_OPERATOR_POLICY',
                    valueFrom: change.wrongReference ? 'other-policy' : c.policySecretArn,
                  },
                ],
              },
            ],
          },
        };
      if (args[1] === 'describe-services')
        return { services: structuredClone(change.missingService ? state.slice(0, 1) : state) };
      if (args[1] === 'update-service') {
        const s = state.find((v) => v.serviceArn === args[args.indexOf('--service') + 1]);
        s.deployments = [
          { id: `${s.serviceArn}-new`, status: 'PRIMARY', rolloutState: 'COMPLETED' },
        ];
        return { service: structuredClone(s) };
      }
      if (args[1] === 'wait') {
        if (change.rollback) state[0].deployments[0].id = 'web-old';
        if (change.scaled) state[0].desiredCount++;
        return '';
      }
      throw new Error(`Unexpected fixture call ${args.join(' ')}`);
    },
    fetch: async (url, options) => {
      calls.push(['fetch', url, options.redirect]);
      return Response.json({
        ...approvedPolicy,
        ...(change.publicMismatch
          ? { eligibility: { ...approvedPolicy.eligibility, registration_enabled: true } }
          : {}),
        mode: 'production',
        ready: true,
      });
    },
  };
  return { calls, operations };
}
test('policy reload replaces both services without scaling and verifies exact public policy', async () => {
  const { calls, operations } = policyFixture();
  const result = await reloadPolicy(c, approvedPolicy, operations);
  assert.deepEqual(result.desiredCounts, [2, 3]);
  assert.equal(result.registrationEnabled, false);
  assert.equal(result.policyVersionId, 'approved');
  assert.equal(calls.filter((v) => v[1] === 'update-service').length, 2);
  assert(
    calls
      .filter((v) => v[1] === 'update-service')
      .every((v) => v.at(-1) === '--force-new-deployment' && !v.includes('--desired-count')),
  );
  assert(!calls.some((v) => v.includes('get-secret-value')));
  assert.deepEqual(calls.at(-1), ['fetch', 'https://fans.example.com/api/platform/site', 'error']);
});
test('policy reload rejects rollback, concurrent secret/scaling changes and partial or wrong public state', async () => {
  for (const changed of [
    { rollback: true },
    { versionRace: true },
    { scaled: true },
    { missingService: true },
    { publicMismatch: true },
    { wrongReference: true },
  ]) {
    const { calls, operations } = policyFixture(changed);
    await assert.rejects(() => reloadPolicy(c, approvedPolicy, operations));
    if (changed.missingService || changed.wrongReference)
      assert(!calls.some((v) => v[1] === 'update-service'));
  }
  const { calls, operations } = policyFixture();
  await assert.rejects(() => reloadPolicy(c, {}, operations));
  assert.equal(calls.length, 0);
});
test('public policy verification refuses sandbox, oversized and redirected endpoint responses', async () => {
  await assert.rejects(() =>
    verifyPublicPolicy(c, approvedPolicy, async () =>
      Response.json({ ...approvedPolicy, mode: 'sandbox', ready: true }),
    ),
  );
  await assert.rejects(() =>
    verifyPublicPolicy(c, approvedPolicy, async () => new Response('x'.repeat(66561))),
  );
  await assert.rejects(() =>
    verifyPublicPolicy(c, approvedPolicy, async () => new Response(null, { status: 302 })),
  );
});
test('deployment rejects mutable/cross-account images and missing HA or region constraints', () => {
  assert.equal(validateConfiguration(c), c);
  for (const changed of [
    { region: 'eu-west-1' },
    { imageUri: c.imageUri.replace(/@.*/u, ':latest') },
    { imageUri: c.imageUri.replace(c.accountId, '999999999999') },
    { workerDesiredCount: 1 },
    { domainName: c.originDomainName },
    { mediaPrefix: 'myfans/' },
    { providerSecretArn: c.providerSecretArn.replace(c.accountId, '999999999999') },
  ])
    assert.throws(() => validateConfiguration({ ...c, ...changed }));
});
test('privacy recovery rejects another source, duplicate subjects and unsafe erasure keys', () => {
  const journal = {
    version: 1,
    source_database_identifier: f.DatabaseIdentifier,
    source_storage_id: storageIdentity(c.region, c.accountId, media.MediaBucketName, c.mediaPrefix),
    exported_at: '2026-10-07T00:00:00Z',
    requests: [
      {
        id: '12345678-1234-1234-1234-123456789abc',
        user_id: '12345678-1234-1234-1234-123456789def',
        status: 'application_erased',
        legal_hold: false,
        privacy_version: 'v1',
        created_at: '2026-10-06T00:00:00Z',
        not_before: '2026-10-06T00:00:00Z',
        application_erased_at: '2026-10-06T12:00:00Z',
        completed_at: null,
        retention: {
          erasure_delay_days: 0,
          backup_days: 35,
          security_days: 30,
          financial_days: 365,
          policy_evidence_days: 365,
          moderation_evidence_days: 365,
        },
      },
    ],
    erased_storage_keys: ['12345678-1234-1234-1234-123456789abc.jpg'],
  };
  assert.equal(validatePrivacyJournal(journal, c, f, media), journal);
  for (const change of [
    { source_storage_id: 's3:attacker' },
    { source_database_identifier: 'other' },
    { requests: [...journal.requests, ...journal.requests] },
    { erased_storage_keys: ['../outside-prefix'] },
    { erased_storage_keys: [...journal.erased_storage_keys, ...journal.erased_storage_keys] },
    {
      requests: [
        {
          ...journal.requests[0],
          retention: { ...journal.requests[0].retention, security_days: 0 },
        },
      ],
    },
  ])
    assert.throws(() => validatePrivacyJournal({ ...journal, ...change }, c, f, media));
});
test('recovery binding normalizes the exact application S3 prefix contract', () => {
  const expected = `s3:${createHash('sha256').update('us-east-1\n123456789012\nprimary\nmyfans/').digest('hex')}`;
  assert.equal(storageIdentity(c.region, c.accountId, 'primary', 'myfans'), expected);
  assert.equal(storageIdentity(c.region, c.accountId, 'primary', 'myfans/'), expected);
});
test('manifest rejects another deployment, changed image, duplicate keys and incomplete versions', () => {
  assert.equal(validateManifest(m, c, f, media), m);
  for (const change of [
    { signingKeyArn: 'attacker-key' },
    { accountId: '999999999999' },
    { imageUri: `${c.imageUri}modified` },
    { objects: [...m.objects, ...m.objects] },
    { objects: [{ ...m.objects[0], checkpointVersion: 'null' }] },
    { objects: [{ ...m.objects[0], key: 'outside-prefix' }] },
    { objects: [{ ...m.objects[0], size: 536870913 }] },
    { objects: [{ ...m.objects[0], checkpointKey: 'another-checkpoint' }] },
  ])
    assert.throws(() => validateManifest({ ...m, ...change }, c, f, media));
});
function recoveryTaskFixture(change = {}) {
  const arn = (role, version) =>
    `arn:aws:ecs:${c.region}:${c.accountId}:task-definition/myfans-prod-${role}:${version}`;
  const old = { WorkerTask: arn('worker', 1), MigrationTask: arn('migration', 1) };
  const current = { WorkerTask: arn('worker', 2), MigrationTask: arn('migration', 2) };
  const manifest = { ...m, taskDefinitions: old };
  const calls = [];
  const newer = { ...c, imageUri: c.imageUri.replace('a'.repeat(64), 'c'.repeat(64)) };
  const call = async (_c, args) => {
    calls.push(args);
    if (args[0] === 'ecr')
      return {
        imageDetails: change.deletedImage
          ? []
          : [{ imageDigest: args.at(-1).slice('imageDigest='.length) }],
      };
    assert.equal(args[1], 'describe-task-definition');
    if (change.deletedTask) throw new Error('Task definition was deleted');
    const reference = args.at(-1);
    const worker = reference.includes('-worker:');
    const image = reference.endsWith(':1') ? m.imageUri : newer.imageUri;
    return {
      taskDefinition: {
        taskDefinitionArn: reference,
        status: 'INACTIVE',
        networkMode: 'awsvpc',
        runtimePlatform: { cpuArchitecture: 'ARM64' },
        containerDefinitions: [
          { name: 'prepare', image },
          {
            name: 'app',
            image,
            entryPoint: change.wrongEntry
              ? ['sh', '-c']
              : ['node', '/app/scripts/cloud-entrypoint.mjs'],
            command: [worker ? 'worker' : 'migrate'],
            environment: [
              { name: 'PUBLIC_ORIGIN', value: `https://${c.domainName}` },
              { name: 'MAIL_PUBLIC_ORIGIN', value: `https://${c.domainName}` },
            ],
            secrets: [
              { name: 'CLOUD_OPERATOR_POLICY', valueFrom: c.policySecretArn },
              { name: 'CLOUD_DATABASE_PASSWORD', valueFrom: 'password-reference' },
              {
                name: worker ? 'CLOUD_MFA_KEY_HEX' : 'CLOUD_RUNTIME_PASSWORD',
                valueFrom: 'key-or-password-reference',
              },
              {
                name: 'CLOUD_STRIPE_SECRET',
                valueFrom: `${change.wrongProvider ? 'other' : c.providerSecretArn}:stripe_secret_key::`,
              },
            ],
          },
          ...(worker
            ? [{ name: 'scanner', image: change.wrongScanner ? newer.imageUri : m.scannerImageUri }]
            : []),
        ],
      },
    };
  };
  return { manifest, current, newer, old, calls, call };
}
test('historical recovery selects signed immutable task revisions while source runs a newer image', async () => {
  const { manifest, current, newer, old, calls, call } = recoveryTaskFixture();
  assert.equal(
    validateManifest(manifest, newer, f, media, { allowHistoricalImages: true }),
    manifest,
  );
  const definitions = await resolveCheckpointTasks(newer, manifest, current, call);
  assert.equal(definitions.WorkerTask.taskDefinitionArn, old.WorkerTask);
  assert.equal(definitions.MigrationTask.taskDefinitionArn, old.MigrationTask);
  assert.equal(
    definitions.WorkerTask.containerDefinitions.find((v) => v.name === 'app').image,
    m.imageUri,
  );
  assert(calls.every((v) => ['describe-task-definition', 'describe-images'].includes(v[1])));
  assert.equal(calls.filter((v) => v[0] === 'ecr').length, 2);
  assert(!calls.some((v) => v.includes(current.WorkerTask) || v.includes(current.MigrationTask)));
});
test('historical recovery fails preflight for lost revisions/images or changed launch/provider identity', async () => {
  for (const change of [
    { deletedTask: true },
    { deletedImage: true },
    { wrongEntry: true },
    { wrongScanner: true },
    { wrongProvider: true },
  ]) {
    const { manifest, current, newer, calls, call } = recoveryTaskFixture(change);
    await assert.rejects(() => resolveCheckpointTasks(newer, manifest, current, call));
    assert(calls.every((v) => ['describe-task-definition', 'describe-images'].includes(v[1])));
  }
  const { manifest, newer } = recoveryTaskFixture();
  assert.throws(() =>
    validateManifest(
      { ...manifest, imageUri: manifest.imageUri.replace(c.accountId, '999999999999') },
      newer,
      f,
      media,
      { allowHistoricalImages: true },
    ),
  );
  assert.throws(() =>
    validateManifest(
      {
        ...manifest,
        taskDefinitions: {
          ...manifest.taskDefinitions,
          WorkerTask: manifest.taskDefinitions.WorkerTask.replace(c.accountId, '999999999999'),
        },
      },
      newer,
      f,
      media,
      { allowHistoricalImages: true },
    ),
  );
});
test('legacy checkpoint rejects a newer live launch contract before any recovery mutation', async () => {
  const { current, newer, old, calls, call } = recoveryTaskFixture();
  await assert.rejects(() => resolveCheckpointTasks(newer, m, current, call), /Legacy checkpoint/);
  assert.equal(calls.length, 1);
  const definitions = await resolveCheckpointTasks(c, m, old, call);
  assert.equal(definitions.WorkerTask.taskDefinitionArn, old.WorkerTask);
});
test('capture refuses oversized checkpoint serialization before signing an unreadable envelope', () => {
  assert.deepEqual(JSON.parse(checkpointManifestBytes(m)), m);
  assert.throws(
    () =>
      checkpointManifestBytes({ ...m, objects: [{ ...m.objects[0], key: 'x'.repeat(33554432) }] }),
    /nothing was signed or published/,
  );
});

const recoveredId = 'myfans-prod-restore-1234abcd';
const ownerSecret = 'arn:aws:secretsmanager:us-east-1:123456789012:secret:restored-owner-123456';
const taskArn = `arn:aws:ecs:us-east-1:123456789012:task/myfans/${'a'.repeat(32)}`;
const restoredEvidence = {
  checkpoint: id,
  databaseIdentifier: recoveredId,
  databaseHost: `${recoveredId}.abcdef.us-east-1.rds.amazonaws.com`,
  ownerSecretArn: ownerSecret,
  sourceObjectsModified: false,
  liveServicesModified: true,
  migrationTask: taskArn,
  rebindTask: taskArn,
  privacyReplayTask: taskArn,
  verificationTask: taskArn,
  privacyJournal: {
    id: m.privacyJournalId,
    accountId: c.accountId,
    deploymentName: c.deploymentName,
    region: c.region,
    exportedAt: m.privacyJournalExportedAt,
  },
};
const acceptance = {
  checkpoint: id,
  privacyJournalId: m.privacyJournalId,
  privacyExportedAt: m.privacyJournalExportedAt,
  recoveryPointAccepted: true,
  changeReference: 'change:INC-1234',
  providerReconciliationReference: 'finance:INC-1234',
  privacyReconciliationReference: 'privacy:INC-1234',
};
const candidate = {
  ...c,
  mfaSecretArn: ownerSecret,
  mfaSecretVersion: m.mfaVersion,
  activeRecovery: {
    suffix: '1234abcd',
    databaseIdentifier: recoveredId,
    mediaStackName: `${recoveredId}-media`,
    privacyStackName: `${recoveredId}-privacy`,
    checkpoint: id,
    privacyJournalId: m.privacyJournalId,
    evidenceSha256: 'a'.repeat(64),
  },
};
test('promotion requires exact account, isolated identities and explicit reconciliation evidence', () => {
  assert.equal(validatePromotionEvidence(c, restoredEvidence, acceptance), '1234abcd');
  assert.equal(validateConfiguration(candidate), candidate);
  for (const changed of [
    { databaseIdentifier: 'myfans-prod' },
    { ownerSecretArn: ownerSecret.replace(c.accountId, '999999999999') },
    { privacyReplayTask: taskArn.replace(c.accountId, '999999999999') },
    { sourceObjectsModified: true },
    { privacyJournal: { ...restoredEvidence.privacyJournal, accountId: '999999999999' } },
  ])
    assert.throws(() =>
      validatePromotionEvidence(c, { ...restoredEvidence, ...changed }, acceptance),
    );
  assert.throws(() =>
    validatePromotionEvidence(c, restoredEvidence, { ...acceptance, recoveryPointAccepted: false }),
  );
  assert.throws(() =>
    validatePromotionEvidence(c, restoredEvidence, { ...acceptance, privacyExportedAt: 'older' }),
  );
  assert.throws(() =>
    validateConfiguration({
      ...candidate,
      activeRecovery: { ...candidate.activeRecovery, mediaStackName: 'myfans-prod-media' },
    }),
  );
  assert.throws(() => validateConfiguration({ ...candidate, mfaSecretVersion: 'AWSCURRENT' }));
});
test('active recovery revalidates encrypted HA RDS identity and preserves foundation dependencies', () => {
  const foundation = {
    DatabaseIdentifier: 'original',
    DatabaseHost: 'original.example',
    DataKeyArn: 'key',
    VpcId: 'vpc-1',
    DatabaseSubnetGroup: 'isolated',
    DatabaseSecurityGroup: 'sg-db',
    DatabaseParameterGroup: 'strict',
    MailQueueUrl: 'preserved',
  };
  const db = {
    DBInstanceIdentifier: recoveredId,
    DBInstanceStatus: 'available',
    DBInstanceArn: `arn:aws:rds:${c.region}:${c.accountId}:db:${recoveredId}`,
    Endpoint: { Address: restoredEvidence.databaseHost },
    PubliclyAccessible: false,
    StorageEncrypted: true,
    MultiAZ: true,
    DeletionProtection: true,
    BackupRetentionPeriod: 35,
    KmsKeyId: 'key',
    DBSubnetGroup: { VpcId: 'vpc-1', DBSubnetGroupName: 'isolated' },
    VpcSecurityGroups: [{ VpcSecurityGroupId: 'sg-db' }],
    DBParameterGroups: [{ DBParameterGroupName: 'strict', ParameterApplyStatus: 'in-sync' }],
    MasterUsername: 'myfans_owner',
    MasterUserSecret: { SecretStatus: 'active', SecretArn: ownerSecret },
    TagList: [{ Key: 'MyFansRestore', Value: id }],
  };
  const result = validateRecoveredDatabase(db, candidate, foundation);
  assert.equal(result.DatabaseOwnerSecretArn, ownerSecret);
  assert.equal(result.DatabaseIdentifier, recoveredId);
  assert.equal(result.MailQueueUrl, 'preserved');
  assert.equal(foundation.DatabaseIdentifier, 'original');
  for (const changed of [
    { DBInstanceArn: db.DBInstanceArn.replace(c.accountId, '999999999999') },
    { PubliclyAccessible: true },
    { MultiAZ: false },
    { BackupRetentionPeriod: 1 },
    { StorageEncrypted: false },
    { DeletionProtection: false },
    { KmsKeyId: 'other-key' },
    { TagList: [] },
    { VpcSecurityGroups: [{ VpcSecurityGroupId: 'public' }] },
  ])
    assert.throws(() => validateRecoveredDatabase({ ...db, ...changed }, candidate, foundation));
  assert.throws(() =>
    validateRecoveredDatabase(db, candidate, { ...foundation, DatabaseIdentifier: recoveredId }),
  );
});
test('promotion rejects failed tasks and replay against a changed journal or target', () => {
  const journal = {
    sha256: 'a'.repeat(64),
    objectVersion: 'exact-version',
    bucket: 'recovery',
    key: 'journal',
    databaseIdentifier: 'source',
    sourceStorageId: 's3:source',
  };
  const env = {
    CLOUD_DATABASE_HOST: restoredEvidence.databaseHost,
    S3_BUCKET: 'target',
    PRIVACY_JOURNAL_SHA256: journal.sha256,
    PRIVACY_JOURNAL_VERSION: journal.objectVersion,
    PRIVACY_JOURNAL_BUCKET: journal.bucket,
    PRIVACY_JOURNAL_KEY: journal.key,
    PRIVACY_SOURCE_DATABASE_ID: journal.databaseIdentifier,
    PRIVACY_SOURCE_STORAGE_ID: journal.sourceStorageId,
    RECOVERY_DATABASE_HOST: restoredEvidence.databaseHost,
  };
  const definition = {
    taskDefinitionArn: 'verified-definition',
    containerDefinitions: [
      {
        name: 'app',
        image: c.imageUri,
        entryPoint: ['node', '/app/scripts/cloud-entrypoint.mjs'],
        command: ['privacy-replay'],
        environment: Object.entries(env).map(([name, value]) => ({ name, value })),
        secrets: [{ name: 'CLOUD_DATABASE_PASSWORD', valueFrom: `${ownerSecret}:password::` }],
      },
    ],
  };
  const task = {
    taskArn,
    taskDefinitionArn: definition.taskDefinitionArn,
    lastStatus: 'STOPPED',
    containers: [{ name: 'app', exitCode: 0 }],
  };
  const media = { MediaBucketName: 'target' };
  assert.ok(
    validateRestoreTask(task, definition, 'privacyReplayTask', c, restoredEvidence, journal, media),
  );
  assert.throws(() =>
    validateRestoreTask(
      { ...task, containers: [{ name: 'app', exitCode: 1 }] },
      definition,
      'privacyReplayTask',
      c,
      restoredEvidence,
      journal,
      media,
    ),
  );
  assert.throws(() =>
    validateRestoreTask(
      task,
      definition,
      'privacyReplayTask',
      c,
      restoredEvidence,
      { ...journal, sha256: 'b'.repeat(64) },
      media,
    ),
  );
  assert.throws(() =>
    validateRestoreTask(task, definition, 'privacyReplayTask', c, restoredEvidence, journal, {
      MediaBucketName: 'original',
    }),
  );
  assert.throws(() =>
    validateRestoreTask(task, definition, 'verificationTask', c, restoredEvidence, journal, media),
  );
  for (const changed of [
    { command: ['doctor'] },
    { command: [] },
    { environment: [{ name: 'CLOUD_DATABASE_HOST', value: 'different.example' }] },
    { environmentFiles: [{ type: 's3', value: 'arn:aws:s3:::other/env' }] },
  ]) {
    assert.throws(() =>
      validateRestoreTask(
        { ...task, overrides: { containerOverrides: [{ name: 'app', ...changed }] } },
        definition,
        'privacyReplayTask',
        c,
        restoredEvidence,
        journal,
        media,
      ),
    );
  }
  for (const changed of [
    { entryPoint: ['node', '-e', 'process.exit(0)'] },
    { entryPoint: [] },
    { environmentFiles: [{ type: 's3', value: 'arn:aws:s3:::other/env' }] },
  ]) {
    assert.throws(() =>
      validateRestoreTask(
        task,
        {
          ...definition,
          containerDefinitions: [{ ...definition.containerDefinitions[0], ...changed }],
        },
        'privacyReplayTask',
        c,
        restoredEvidence,
        journal,
        media,
      ),
    );
  }
  assert.ok(
    validateRestoreTask(
      { ...task, overrides: { containerOverrides: [{ name: 'app' }] } },
      definition,
      'privacyReplayTask',
      c,
      restoredEvidence,
      journal,
      media,
    ),
  );
});
