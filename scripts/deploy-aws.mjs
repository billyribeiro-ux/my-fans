import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const stages = ['registry', 'foundation', 'media', 'runtime', 'edge'];
export function validateAccount(c) {
  if (
    !c ||
    c.region !== 'us-east-1' ||
    !/^\d{12}$/u.test(c.accountId) ||
    !/^[a-z][a-z0-9-]{2,19}$/u.test(c.deploymentName)
  )
    throw new Error('Expected accountId, us-east-1 region, and lowercase deploymentName');
  return c;
}
export function validateConfiguration(c) {
  validateAccount(c);
  for (const key of ['domainName', 'originDomainName'])
    if (!/^(?=.{1,253}$)[a-z0-9][a-z0-9.-]+\.[a-z]{2,63}$/u.test(c[key]) || c[key].includes('..'))
      throw new Error(`Invalid ${key}`);
  if (c.domainName === c.originDomainName) throw new Error('Viewer and origin domains must differ');
  if (!/^Z[A-Z0-9]+$/u.test(c.hostedZoneId))
    throw new Error('Expected a Route 53 public hosted zone id');
  for (const key of ['imageUri', 'scannerImageUri'])
    if (
      !new RegExp(
        `^${c.accountId}\\.dkr\\.ecr\\.us-east-1\\.amazonaws\\.com/[a-z0-9][a-z0-9/_-]*@sha256:[a-f0-9]{64}$`,
        'u',
      ).test(c[key])
    )
      throw new Error(`Expected immutable account-owned ECR ${key}`);
  for (const key of ['providerSecretArn', 'policySecretArn'])
    if (!c[key]?.startsWith(`arn:aws:secretsmanager:us-east-1:${c.accountId}:secret:`))
      throw new Error(`Expected same-account ${key}`);
  if (
    c.mfaSecretArn &&
    !c.mfaSecretArn.startsWith(`arn:aws:secretsmanager:us-east-1:${c.accountId}:secret:`)
  )
    throw new Error('Rotated MFA secret must belong to this account and region');
  for (const key of ['webDesiredCount', 'workerDesiredCount'])
    if (!Number.isInteger(c[key]) || c[key] < 2 || c[key] > (key === 'webDesiredCount' ? 20 : 10))
      throw new Error(`Expected at least two AZ-distributed ${key}`);
  if (!/^[A-Z]{2}$/u.test(c.stripeAccountCountry)) throw new Error('Invalid Stripe country');
  if (!/^[a-z0-9][a-z0-9/-]{0,63}[a-z0-9]$/u.test(c.mediaPrefix) || c.mediaPrefix.includes('//'))
    throw new Error('Invalid mediaPrefix; omit the trailing slash');
  if (!/^db\.[a-z0-9]+\.[a-z0-9]+$/u.test(c.dbInstanceClass))
    throw new Error('Invalid RDS instance class');
  if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/u.test(c.alarmEmail)) throw new Error('Invalid alarmEmail');
  validateRecoveryBinding(c);
  return c;
}
export function validateRecoveryBinding(c) {
  if (c.mfaSecretVersion && (!c.mfaSecretArn || !/^[a-zA-Z0-9-]{32,64}$/u.test(c.mfaSecretVersion)))
    throw new Error('A pinned MFA version requires an exact secret ARN and version id');
  if (!c.activeRecovery) return;
  const a = c.activeRecovery;
  const base = `${c.deploymentName}-restore-${a.suffix}`;
  if (
    !/^[a-f0-9]{8}$/u.test(a.suffix) ||
    a.databaseIdentifier !== base ||
    a.mediaStackName !== `${base}-media` ||
    a.privacyStackName !== `${base}-privacy` ||
    !/^checkpoint-[a-f0-9-]{36}$/u.test(a.checkpoint) ||
    !/^privacy-[a-f0-9-]{36}$/u.test(a.privacyJournalId) ||
    !/^[a-f0-9]{64}$/u.test(a.evidenceSha256) ||
    !c.mfaSecretArn
  )
    throw new Error(
      'Invalid active recovery binding; use promote-aws to prepare a verified configuration',
    );
}
export function validateRecoveredDatabase(db, c, f) {
  const a = c.activeRecovery;
  if (
    !a ||
    !db ||
    db.DBInstanceIdentifier !== a.databaseIdentifier ||
    db.DBInstanceIdentifier === f.DatabaseIdentifier ||
    db.Endpoint?.Address === f.DatabaseHost ||
    !db.Endpoint?.Address?.startsWith(`${a.databaseIdentifier}.`) ||
    !db.Endpoint.Address.endsWith('.rds.amazonaws.com') ||
    db.DBInstanceStatus !== 'available' ||
    db.DBInstanceArn !== `arn:aws:rds:${c.region}:${c.accountId}:db:${a.databaseIdentifier}` ||
    db.PubliclyAccessible ||
    !db.StorageEncrypted ||
    !db.MultiAZ ||
    !db.DeletionProtection ||
    db.BackupRetentionPeriod < 35 ||
    db.KmsKeyId !== f.DataKeyArn ||
    db.DBSubnetGroup?.VpcId !== f.VpcId ||
    db.DBSubnetGroup?.DBSubnetGroupName !== f.DatabaseSubnetGroup ||
    db.VpcSecurityGroups?.length !== 1 ||
    db.VpcSecurityGroups[0].VpcSecurityGroupId !== f.DatabaseSecurityGroup ||
    !db.DBParameterGroups?.some(
      (v) =>
        v.DBParameterGroupName === f.DatabaseParameterGroup && v.ParameterApplyStatus === 'in-sync',
    ) ||
    db.MasterUsername !== 'myfans_owner' ||
    db.MasterUserSecret?.SecretStatus !== 'active' ||
    !db.MasterUserSecret.SecretArn.startsWith(
      `arn:aws:secretsmanager:${c.region}:${c.accountId}:secret:`,
    ) ||
    !db.TagList?.some((v) => v.Key === 'MyFansRestore' && v.Value === a.checkpoint)
  )
    throw new Error(
      'Recovered database no longer matches the verified private, encrypted, HA deployment binding',
    );
  return {
    ...f,
    DatabaseIdentifier: db.DBInstanceIdentifier,
    DatabaseHost: db.Endpoint.Address,
    DatabaseOwnerSecretArn: db.MasterUserSecret.SecretArn,
  };
}
export async function aws(c, args, { json = true, quiet = false } = {}) {
  return new Promise((accept, reject) => {
    const child = spawn(
      'aws',
      [...args, '--region', c.region, '--no-cli-pager', '--output', 'json'],
      { env: { ...process.env, AWS_PAGER: '' }, stdio: ['ignore', 'pipe', 'pipe'] },
    );
    let output = '';
    let error = '';
    child.stdout.on('data', (b) => {
      output += b;
    });
    child.stderr.on('data', (b) => {
      error += b;
    });
    child.once('error', () => reject(new Error('AWS CLI is unavailable')));
    child.once('close', (code) => {
      if (code)
        return reject(
          new Error(
            quiet
              ? 'AWS operation failed'
              : `AWS ${args[0]} ${args[1]} failed: ${error.slice(-2000)}`,
          ),
        );
      try {
        accept(json && output.trim() ? JSON.parse(output) : output);
      } catch {
        reject(new Error('Invalid AWS response'));
      }
    });
  });
}
export async function outputs(c, stage, { original = false } = {}) {
  validateRecoveryBinding(c);
  const name =
    !original && c.activeRecovery && ['media', 'privacy'].includes(stage)
      ? c.activeRecovery[`${stage}StackName`]
      : `${c.deploymentName}-${stage}`;
  const result = await aws(c, ['cloudformation', 'describe-stacks', '--stack-name', name]);
  const stack = result.Stacks[0];
  if (
    !['CREATE_COMPLETE', 'UPDATE_COMPLETE', 'UPDATE_ROLLBACK_COMPLETE'].includes(stack.StackStatus)
  )
    throw new Error(`${stage} stack is not stable`);
  const resultOutputs = Object.fromEntries(stack.Outputs.map((v) => [v.OutputKey, v.OutputValue]));
  if (!original && c.activeRecovery && stage === 'foundation') {
    const db = (
      await aws(c, [
        'rds',
        'describe-db-instances',
        '--db-instance-identifier',
        c.activeRecovery.databaseIdentifier,
      ])
    ).DBInstances[0];
    return validateRecoveredDatabase(db, c, resultOutputs);
  }
  if (!original && c.activeRecovery && stage === 'media') {
    const prefix = `${c.accountId}-${c.region}-restore-${c.activeRecovery.suffix}`;
    if (
      resultOutputs.MediaBucketName !== `${prefix}-media` ||
      resultOutputs.RecoveryBucketName !== `${prefix}-recovery`
    )
      throw new Error('Recovered media output identity changed');
  }
  return resultOutputs;
}
export async function identity(c) {
  if ((await aws(c, ['sts', 'get-caller-identity'])).Account !== c.accountId)
    throw new Error('Authenticated AWS account does not match deployment configuration');
}
export async function assertRuntimeBinding(c, { checkMfa = true } = {}) {
  const [f, m, r] = await Promise.all([
    outputs(c, 'foundation'),
    outputs(c, 'media'),
    outputs(c, 'runtime'),
  ]);
  for (const key of ['MigrationTask', 'WorkerTask']) {
    const task = (await aws(c, ['ecs', 'describe-task-definition', '--task-definition', r[key]]))
      .taskDefinition;
    const app = task.containerDefinitions.find((v) => v.name === 'app');
    const env = Object.fromEntries((app?.environment || []).map((v) => [v.name, v.value]));
    const secrets = Object.fromEntries((app?.secrets || []).map((v) => [v.name, v.valueFrom]));
    if (
      app?.image !== c.imageUri ||
      env.CLOUD_DATABASE_HOST !== f.DatabaseHost ||
      (key === 'MigrationTask' &&
        secrets.CLOUD_DATABASE_PASSWORD !== `${f.DatabaseOwnerSecretArn}:password::`) ||
      (key === 'WorkerTask' &&
        (env.S3_BUCKET !== m.MediaBucketName ||
          (checkMfa &&
            secrets.CLOUD_MFA_KEY_HEX !==
              `${c.mfaSecretArn || f.MfaKeyArn}${c.mfaSecretVersion ? `:::${c.mfaSecretVersion}` : ''}`)))
    )
      throw new Error(
        'Runtime task bindings differ from this configuration; apply the reviewed runtime change set before administration',
      );
  }
}
async function preflight(c) {
  await identity(c);
  if (!(await aws(c, ['ec2', 'get-ebs-encryption-by-default'])).EbsEncryptionByDefault)
    throw new Error('Enable regional EBS encryption with bootstrap-storage before deployment');
  const ebs = await aws(c, ['ec2', 'get-ebs-default-kms-key-id']);
  const ebsKey = (await aws(c, ['kms', 'describe-key', '--key-id', ebs.KmsKeyId])).KeyMetadata;
  if (ebsKey.KeyState !== 'Enabled') throw new Error('Default EBS encryption key is unavailable');
  const zone = await aws(c, ['route53', 'get-hosted-zone', '--id', c.hostedZoneId]);
  const suffix = zone.HostedZone.Name.replace(/\.$/u, '');
  if (
    zone.HostedZone.Config.PrivateZone ||
    ![c.domainName, c.originDomainName].every(
      (name) => name === suffix || name.endsWith(`.${suffix}`),
    )
  )
    throw new Error('Domains must belong to the supplied public hosted zone');
  const available = await aws(c, [
    'rds',
    'describe-orderable-db-instance-options',
    '--engine',
    'postgres',
    '--engine-version',
    '18.6',
    '--db-instance-class',
    c.dbInstanceClass,
  ]);
  if (
    !available.OrderableDBInstanceOptions.some(
      (v) => v.MultiAZCapable && v.SupportsStorageEncryption,
    )
  )
    throw new Error(
      'PostgreSQL 18.6/class with encrypted Multi-AZ is unavailable in this account/region',
    );
  const zones = (
    await aws(c, [
      'ec2',
      'describe-availability-zones',
      '--filters',
      'Name=state,Values=available',
      'Name=zone-type,Values=availability-zone',
    ])
  ).AvailabilityZones.filter((z) => z.ZoneId !== 'use1-az3');
  if (zones.length < 2) throw new Error('Two supported CloudFront VPC origin AZs are required');
  const lists = (
    await aws(c, [
      'ec2',
      'describe-managed-prefix-lists',
      '--filters',
      'Name=prefix-list-name,Values=com.amazonaws.global.cloudfront.origin-facing',
    ])
  ).PrefixLists;
  if (lists.length !== 1)
    throw new Error('CloudFront origin-facing managed prefix list is unavailable');
  const keys = {};
  for (const name of ['providerSecretArn', 'policySecretArn']) {
    // Metadata only. Secret values are injected by ECS, never read by this CLI.
    const secret = await aws(c, ['secretsmanager', 'describe-secret', '--secret-id', c[name]]);
    if (
      secret.DeletedDate ||
      !secret.KmsKeyId ||
      !Object.values(secret.VersionIdsToStages || {}).some((v) => v.includes('AWSCURRENT'))
    )
      throw new Error(`${name} requires a current value and customer-managed KMS key`);
    const key = await aws(c, ['kms', 'describe-key', '--key-id', secret.KmsKeyId]);
    if (key.KeyMetadata.KeyManager !== 'CUSTOMER' || key.KeyMetadata.KeyState !== 'Enabled')
      throw new Error(`${name} KMS key must be customer-managed and enabled`);
    keys[name] = key.KeyMetadata.Arn;
  }
  for (const image of [c.imageUri, c.scannerImageUri]) {
    const [repositoryName, digest] = image.split('.amazonaws.com/')[1].split('@');
    const result = await aws(c, [
      'ecr',
      'describe-images',
      '--repository-name',
      repositoryName,
      '--image-ids',
      `imageDigest=${digest}`,
    ]);
    if (result.imageDetails.length !== 1) throw new Error('Required image digest is unavailable');
  }
  return {
    availabilityZones: zones
      .slice(0, 2)
      .map((z) => z.ZoneName)
      .join(','),
    prefixList: lists[0].PrefixListId,
    ebsKeyArn: ebsKey.Arn,
    keys,
  };
}
async function parameters(c, stage) {
  if (c.activeRecovery && ['foundation', 'media'].includes(stage))
    throw new Error(
      'Active recovery preserves the original foundation and restored media stack; review their infrastructure separately',
    );
  if (stage === 'registry') return { DeploymentName: c.deploymentName };
  const checked = await preflight(c);
  if (stage === 'foundation')
    return {
      DeploymentName: c.deploymentName,
      AvailabilityZones: checked.availabilityZones,
      CloudFrontPrefixListId: checked.prefixList,
      AlarmEmail: c.alarmEmail,
      DbInstanceClass: c.dbInstanceClass,
    };
  const f = await outputs(c, 'foundation');
  if (stage === 'media')
    return {
      DeploymentName: c.deploymentName,
      MediaPrefix: c.mediaPrefix,
      AlertTopicArn: f.AlarmTopicArn,
    };
  if (stage === 'edge') {
    const r = await outputs(c, 'runtime');
    return {
      DeploymentName: c.deploymentName,
      LoadBalancerArn: r.LoadBalancerArn,
      OriginDomainName: c.originDomainName,
      HostedZoneId: c.hostedZoneId,
      DomainName: c.domainName,
      AlarmTopicArn: f.AlarmTopicArn,
      DataKeyArn: f.DataKeyArn,
    };
  }
  const m = await outputs(c, 'media');
  const fkeys = [
    'VpcId',
    'PrivateSubnets',
    'AppSecurityGroup',
    'AlbSecurityGroup',
    'DatabaseHost',
    'DatabaseOwnerSecretArn',
    'RuntimePasswordArn',
    'MfaKeyArn',
    'DataKeyArn',
    'AlarmTopicArn',
    'MailQueueUrl',
    'MailQueueArn',
    'SesConfigurationSet',
  ];
  return {
    ...Object.fromEntries(fkeys.map((key) => [key, f[key]])),
    MfaKeyArn: c.mfaSecretArn || f.MfaKeyArn,
    MfaKeyVersion: c.mfaSecretVersion || '',
    DatabaseIdentifier: f.DatabaseIdentifier,
    EbsKeyArn: checked.ebsKeyArn,
    DeploymentName: c.deploymentName,
    ImageUri: c.imageUri,
    ScannerImageUri: c.scannerImageUri,
    MediaBucketName: m.MediaBucketName,
    MediaEncryptionKeyArn: m.MediaEncryptionKeyArn,
    MediaTaskRoleArn: m.WorkloadRoleArn,
    MediaTaskRoleName: m.WorkloadRoleArn.split('/').at(-1),
    MediaPrefix: c.mediaPrefix,
    ProviderSecretArn: c.providerSecretArn,
    ProviderSecretKmsKeyArn: checked.keys.providerSecretArn,
    PolicySecretArn: c.policySecretArn,
    PolicySecretKmsKeyArn: checked.keys.policySecretArn,
    HostedZoneId: c.hostedZoneId,
    DomainName: c.domainName,
    OriginDomainName: c.originDomainName,
    StripeAccountCountry: c.stripeAccountCountry,
    SumsubClientId: c.sumsubClientId,
    SumsubLevel: c.sumsubLevel,
    MailFrom: c.mailFrom,
    WebDesiredCount: 0,
    WorkerDesiredCount: 0,
  };
}
async function plan(c, stage) {
  if (!stages.includes(stage))
    throw new Error('Choose registry, foundation, media, runtime or edge');
  const params = await parameters(c, stage);
  const template = await readFile(
    resolve(ROOT, `deploy/aws/${stage}.${stage === 'media' ? 'yaml' : 'json'}`),
    'utf8',
  );
  // JSON pretty-printing does not consume CloudFormation's 51,200-byte request budget.
  const body = stage === 'media' ? template : JSON.stringify(JSON.parse(template));
  if (Buffer.byteLength(body) > 51200) throw new Error('Template exceeds inline request limit');
  const stackName = `${c.deploymentName}-${stage}`;
  let type = 'CREATE';
  try {
    await outputs(c, stage);
    type = 'UPDATE';
  } catch (error) {
    if (!error.message.includes('does not exist')) throw error;
  }
  if (stage === 'runtime' && type === 'UPDATE') {
    const r = await outputs(c, 'runtime');
    const s = await aws(c, [
      'ecs',
      'describe-services',
      '--cluster',
      r.ClusterArn,
      '--services',
      r.WebService,
      r.WorkerService,
    ]);
    if (s.services.some((v) => v.desiredCount || v.runningCount || v.pendingCount))
      throw new Error('Pause and checkpoint services before changing runtime images or schema');
  }
  const result = await aws(c, [
    'cloudformation',
    'create-change-set',
    '--stack-name',
    stackName,
    '--change-set-name',
    `review-${Date.now()}-${randomUUID().slice(0, 8)}`,
    '--change-set-type',
    type,
    '--template-body',
    body,
    '--parameters',
    JSON.stringify(
      Object.entries(params).map(([ParameterKey, value]) => ({
        ParameterKey,
        ParameterValue: String(value),
      })),
    ),
    '--capabilities',
    'CAPABILITY_IAM',
    '--description',
    `Source SHA256 ${createHash('sha256').update(body).digest('hex')}`,
  ]);
  console.log(
    JSON.stringify({
      changeSetArn: result.Id,
      stackName,
      sourceSha256: createHash('sha256').update(body).digest('hex'),
      next: 'Review CloudFormation changes and service validation, then invoke apply with this exact change-set ARN.',
    }),
  );
}
async function wait(c, check, label, timeout = 45 * 60000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    console.log(`${label}: waiting`);
    await new Promise((accept) => setTimeout(accept, 20000));
  }
  throw new Error(`${label} timed out; inspect AWS state before retrying`);
}
export async function runTask(c, mode, extraEnvironment = []) {
  await assertRuntimeBinding(c);
  const [f, r] = await Promise.all([outputs(c, 'foundation'), outputs(c, 'runtime')]);
  const taskDefinition = ['migrate', 'bootstrap-admin'].includes(mode)
    ? r.MigrationTask
    : r.WorkerTask;
  const result = await aws(c, [
    'ecs',
    'run-task',
    '--cluster',
    r.ClusterArn,
    '--task-definition',
    taskDefinition,
    '--capacity-provider-strategy',
    JSON.stringify([
      {
        capacityProvider: ['migrate', 'bootstrap-admin'].includes(mode)
          ? r.AdminCompute
          : r.WorkerCompute,
        weight: 1,
      },
    ]),
    '--network-configuration',
    JSON.stringify({
      awsvpcConfiguration: {
        subnets: f.PrivateSubnets.split(','),
        securityGroups: [f.AppSecurityGroup],
        assignPublicIp: 'DISABLED',
      },
    }),
    '--overrides',
    JSON.stringify({
      containerOverrides: [{ name: 'app', command: [mode], environment: extraEnvironment }],
    }),
  ]);
  if (result.failures?.length || result.tasks?.length !== 1)
    throw new Error('One-off task could not be scheduled');
  const arn = result.tasks[0].taskArn;
  await wait(
    c,
    async () => {
      const task = (
        await aws(c, ['ecs', 'describe-tasks', '--cluster', r.ClusterArn, '--tasks', arn])
      ).tasks[0];
      if (task.lastStatus !== 'STOPPED') return false;
      if (task.containers.find((v) => v.name === 'app')?.exitCode !== 0)
        throw new Error(`Task failed: ${arn}. Inspect its redacted CloudWatch log.`);
      return true;
    },
    mode,
  );
  console.log(JSON.stringify({ mode, taskArn: arn, taskDefinition }));
  return { taskArn: arn, taskDefinition };
}
export async function pause(c) {
  const r = await outputs(c, 'runtime');
  const before = await aws(c, [
    'ecs',
    'describe-services',
    '--cluster',
    r.ClusterArn,
    '--services',
    r.WebService,
    r.WorkerService,
  ]);
  const counts = Object.fromEntries(before.services.map((v) => [v.serviceArn, v.desiredCount]));
  for (const service of [r.WebService, r.WorkerService])
    await aws(c, [
      'ecs',
      'update-service',
      '--cluster',
      r.ClusterArn,
      '--service',
      service,
      '--desired-count',
      '0',
    ]);
  await wait(
    c,
    async () =>
      (
        await aws(c, [
          'ecs',
          'describe-services',
          '--cluster',
          r.ClusterArn,
          '--services',
          r.WebService,
          r.WorkerService,
        ])
      ).services.every((v) => v.runningCount === 0 && v.pendingCount === 0),
    'service drain',
  );
  return counts;
}
const canonicalJson = (value) =>
  JSON.stringify(value, function (_key, item) {
    return item && typeof item === 'object' && !Array.isArray(item)
      ? Object.fromEntries(
          Object.keys(item)
            .sort()
            .map((key) => [key, item[key]]),
        )
      : item;
  });
function validatePolicyDocument(expected) {
  if (
    !expected ||
    Object.keys(expected).sort().join(',') !== 'eligibility,operator,policies,retention' ||
    typeof expected.eligibility?.registration_enabled !== 'boolean' ||
    Buffer.byteLength(JSON.stringify(expected)) > 65536
  )
    throw new Error('Supply the complete approved public policy JSON, within 64 KiB');
}
export async function verifyPublicPolicy(c, expected, request = fetch) {
  validatePolicyDocument(expected);
  const response = await request(`https://${c.domainName}/api/platform/site`, {
    redirect: 'error',
    cache: 'no-store',
    headers: { accept: 'application/json', 'cache-control': 'no-cache' },
    signal: AbortSignal.timeout(15000),
  });
  if (!response.ok || !response.body) throw new Error('Public policy endpoint is unavailable');
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > 66560) throw new Error('Public policy response exceeds its size bound');
      chunks.push(value);
    }
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
  const actual = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  if (actual.mode !== 'production' || actual.ready !== true)
    throw new Error('Public policy endpoint is not a ready production deployment');
  delete actual.mode;
  delete actual.ready;
  const approved = canonicalJson(expected);
  if (canonicalJson(actual) !== approved)
    throw new Error('Public policy does not exactly match the approved document');
  return {
    policySha256: createHash('sha256').update(approved).digest('hex'),
    registrationEnabled: expected.eligibility.registration_enabled,
  };
}
export async function reloadPolicy(c, expected, operations = {}) {
  validatePolicyDocument(expected);
  const call = operations.aws || aws;
  const binding = operations.assertRuntimeBinding || assertRuntimeBinding;
  const getOutputs = operations.outputs || outputs;
  await binding(c);
  const r = await getOutputs(c, 'runtime');
  const services = [r.WebService, r.WorkerService];
  const definitions = [r.WebTask, r.WorkerTask];
  for (const definition of definitions) {
    const task = (
      await call(c, ['ecs', 'describe-task-definition', '--task-definition', definition])
    ).taskDefinition;
    const policy = task?.containerDefinitions
      ?.find((v) => v.name === 'app')
      ?.secrets?.find((v) => v.name === 'CLOUD_OPERATOR_POLICY');
    if (task?.taskDefinitionArn !== definition || policy?.valueFrom !== c.policySecretArn)
      throw new Error('Runtime policy reference differs from the configured secret');
  }
  const describe = () =>
    call(c, ['ecs', 'describe-services', '--cluster', r.ClusterArn, '--services', ...services]);
  const currentVersion = async () => {
    const metadata = await call(c, [
      'secretsmanager',
      'describe-secret',
      '--secret-id',
      c.policySecretArn,
    ]);
    const versions = Object.entries(metadata.VersionIdsToStages || {}).filter(([, stages]) =>
      stages.includes('AWSCURRENT'),
    );
    if (metadata.ARN !== c.policySecretArn || metadata.DeletedDate || versions.length !== 1)
      throw new Error('Policy secret metadata has no unambiguous current version');
    return versions[0][0];
  };
  const version = await currentVersion();
  const inspect = (result, counts, deployments) => {
    if (result.failures?.length || result.services?.length !== 2)
      throw new Error('Both deployment services must be available');
    return services.map((arn, index) => {
      const service = result.services.find((v) => v.serviceArn === arn || v.serviceName === arn);
      const deployment = service?.deployments?.[0];
      if (
        !service ||
        service.status !== 'ACTIVE' ||
        service.taskDefinition !== definitions[index] ||
        service.desiredCount < 1 ||
        service.runningCount !== service.desiredCount ||
        service.pendingCount !== 0 ||
        service.deployments?.length !== 1 ||
        deployment.status !== 'PRIMARY' ||
        deployment.rolloutState !== 'COMPLETED' ||
        (counts && service.desiredCount !== counts[index]) ||
        (deployments && deployment.id !== deployments[index])
      )
        throw new Error(
          'Policy reload requires stable services and the exact successful replacement deployments',
        );
      return service.desiredCount;
    });
  };
  const before = await describe();
  const counts = inspect(before);
  const deployments = [];
  for (let index = 0; index < services.length; index++) {
    const changed = await call(c, [
      'ecs',
      'update-service',
      '--cluster',
      r.ClusterArn,
      '--service',
      services[index],
      '--force-new-deployment',
    ]);
    const deployment = changed.service?.deployments?.find((v) => v.status === 'PRIMARY');
    const previous = before.services.find(
      (v) => v.serviceArn === services[index] || v.serviceName === services[index],
    );
    if (!deployment?.id || deployment.id === previous.deployments[0].id)
      throw new Error('AWS did not confirm a new policy deployment');
    deployments.push(deployment.id);
  }
  await call(
    c,
    ['ecs', 'wait', 'services-stable', '--cluster', r.ClusterArn, '--services', ...services],
    { json: false },
  );
  inspect(await describe(), counts, deployments);
  if ((await currentVersion()) !== version)
    throw new Error('Policy secret changed during deployment; repeat the reviewed reload');
  const verified = await verifyPublicPolicy(c, expected, operations.fetch || fetch);
  return { ...verified, policyVersionId: version, deployments, desiredCounts: counts };
}
async function main() {
  const [command, configPath, argument] = process.argv.slice(2);
  const raw = JSON.parse(await readFile(configPath, 'utf8'));
  const c =
    ['apply', 'bootstrap-storage'].includes(command) ||
    (command === 'plan' && argument === 'registry')
      ? validateAccount(raw)
      : validateConfiguration(raw);
  await identity(c);
  const evidence = resolve(ROOT, `.local/aws/${c.deploymentName}/migration.json`);
  if (command === 'bootstrap-storage') {
    await aws(c, ['ec2', 'enable-ebs-encryption-by-default']);
    console.log(
      'Regional EBS encryption is enabled. Existing volumes and the selected default KMS key are unchanged.',
    );
    return;
  }
  if (command === 'preflight') {
    console.log(JSON.stringify(await preflight(c), null, 2));
    return;
  }
  if (command === 'plan') return plan(c, argument);
  if (command === 'apply') {
    if (!argument?.startsWith(`arn:aws:cloudformation:us-east-1:${c.accountId}:changeSet/`))
      throw new Error('Supply the reviewed change-set ARN');
    const change = await aws(c, [
      'cloudformation',
      'describe-change-set',
      '--change-set-name',
      argument,
    ]);
    if (
      !stages.some((s) => change.StackName === `${c.deploymentName}-${s}`) ||
      change.Status !== 'CREATE_COMPLETE' ||
      change.ExecutionStatus !== 'AVAILABLE'
    )
      throw new Error('Change set is not executable for this deployment');
    await aws(c, ['cloudformation', 'execute-change-set', '--change-set-name', argument]);
    console.log('Change set execution started. Wait for stack completion before the next stage.');
    return;
  }
  if (command === 'pause') {
    console.log(JSON.stringify({ previousCounts: await pause(c) }));
    return;
  }
  if (command === 'migrate') {
    const r = await outputs(c, 'runtime');
    const services = await aws(c, [
      'ecs',
      'describe-services',
      '--cluster',
      r.ClusterArn,
      '--services',
      r.WebService,
      r.WorkerService,
    ]);
    if (services.services.some((s) => s.runningCount || s.desiredCount || s.pendingCount))
      throw new Error('Schema migration requires drained services and a checkpoint');
    const result = await runTask(c, 'migrate');
    await mkdir(dirname(evidence), { recursive: true, mode: 0o700 });
    await writeFile(evidence, JSON.stringify(result), { mode: 0o600 });
    return;
  }
  if (command === 'start') {
    const r = await outputs(c, 'runtime');
    const verified = JSON.parse(await readFile(evidence, 'utf8'));
    if (verified.taskDefinition !== r.MigrationTask)
      throw new Error('Current image revision has no successful migration evidence');
    const task = (
      await aws(c, [
        'ecs',
        'describe-tasks',
        '--cluster',
        r.ClusterArn,
        '--tasks',
        verified.taskArn,
      ])
    ).tasks[0];
    if (
      !task ||
      task.taskDefinitionArn !== r.MigrationTask ||
      task.lastStatus !== 'STOPPED' ||
      task.containers.find((v) => v.name === 'app')?.exitCode !== 0
    )
      throw new Error('AWS could not confirm successful migration; rerun the idempotent migration');
    await outputs(c, 'edge');
    // Actual managed host must pass the production scanner/sandbox/provider diagnostics
    // before a web replica can accept traffic.
    await runTask(c, 'doctor');
    for (const [service, count] of [
      [r.WorkerService, c.workerDesiredCount],
      [r.WebService, c.webDesiredCount],
    ])
      await aws(c, [
        'ecs',
        'update-service',
        '--cluster',
        r.ClusterArn,
        '--service',
        service,
        '--desired-count',
        String(count),
      ]);
    await aws(
      c,
      [
        'ecs',
        'wait',
        'services-stable',
        '--cluster',
        r.ClusterArn,
        '--services',
        r.WebService,
        r.WorkerService,
      ],
      { json: false },
    );
    console.log(
      'Services are stable. Run account acceptance and an encrypted checkpoint before opening registration.',
    );
    return;
  }
  if (command === 'doctor') return runTask(c, 'doctor');
  if (command === 'reload-policy' || command === 'verify-policy') {
    if (!argument) throw new Error('Supply the approved public policy JSON file path');
    const bytes = await readFile(argument);
    if (bytes.length > 65536) throw new Error('Approved policy exceeds 64 KiB');
    const expected = JSON.parse(bytes.toString('utf8'));
    console.log(
      JSON.stringify(
        command === 'reload-policy'
          ? await reloadPolicy(c, expected)
          : await verifyPublicPolicy(c, expected),
      ),
    );
    return;
  }
  if (command === 'bootstrap-admin') {
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/u.test(argument))
      throw new Error('Supply an existing verified, MFA-enrolled account email');
    return runTask(c, 'bootstrap-admin', [{ name: 'ADMIN_BOOTSTRAP_EMAIL', value: argument }]);
  }
  if (command === 'status') {
    for (const stage of stages) console.log(stage, await outputs(c, stage));
    return;
  }
  throw new Error(
    'Usage: node scripts/deploy-aws.mjs bootstrap-storage|preflight|plan|apply|pause|migrate|start|doctor|bootstrap-admin|reload-policy|verify-policy|status CONFIG [STAGE|CHANGE_SET_ARN|ADMIN_EMAIL|APPROVED_POLICY_FILE]',
  );
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((e) => {
    console.error(e.message);
    process.exitCode = 1;
  });
