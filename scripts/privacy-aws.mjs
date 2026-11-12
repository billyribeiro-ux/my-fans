import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  aws,
  identity,
  outputs,
  assertRuntimeBinding,
  validateConfiguration,
} from './deploy-aws.mjs';
import { createStack, isolatedTask } from './recover-aws.mjs';

async function main() {
  const [command, configPath, requestId] = process.argv.slice(2);
  const c = validateConfiguration(JSON.parse(await readFile(configPath, 'utf8')));
  if (!['setup', 'purge', 'retention'].includes(command))
    throw new Error(
      'Usage: node scripts/privacy-aws.mjs setup|purge|retention CONFIG [APPROVED_REQUEST_UUID]',
    );
  if (
    command !== 'setup' &&
    !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/u.test(requestId)
  )
    throw new Error('Supply the exact approved privacy request UUID');
  await identity(c);
  const [f, r, m] = await Promise.all([
    outputs(c, 'foundation'),
    outputs(c, 'runtime'),
    outputs(c, 'media'),
  ]);
  if (command === 'setup') {
    const access = await createStack(
      c,
      c.activeRecovery?.privacyStackName || `${c.deploymentName}-privacy`,
      'privacy-access.json',
      {
        DeploymentName: c.deploymentName,
        MaintenanceName: c.activeRecovery
          ? `restore-${c.activeRecovery.suffix}/privacy-erasure`
          : 'privacy-erasure',
        DataKeyArn: f.DataKeyArn,
        OwnerSecretArn: f.DatabaseOwnerSecretArn,
        ImageRepositoryArn: `arn:aws:ecr:${c.region}:${c.accountId}:repository/${c.imageUri.split('.amazonaws.com/')[1].split('@')[0]}`,
        MediaBucketName: m.MediaBucketName,
        RecoveryBucketName: m.RecoveryBucketName,
        RecoveryKeyArn: m.RecoveryEncryptionKeyArn,
        MediaPrefix: c.mediaPrefix,
      },
    );
    console.log(
      JSON.stringify({
        state: 'Owner erasure permissions created; no request was processed',
        ...access,
      }),
    );
    return;
  }
  await assertRuntimeBinding(c);
  const access = await outputs(c, 'privacy');
  const task = (
    await aws(c, ['ecs', 'describe-task-definition', '--task-definition', r.MigrationTask])
  ).taskDefinition;
  const app = task.containerDefinitions.find((v) => v.name === 'app');
  if (app?.image !== c.imageUri)
    throw new Error('Erasure task image differs from the configured deployment');
  task.family = `${c.deploymentName}-privacy-erasure`;
  task.executionRoleArn = access.ExecutionRoleArn;
  // Database evidence expiration needs no AWS API credentials at all.
  if (command === 'purge') task.taskRoleArn = access.TaskRoleArn;
  else delete task.taskRoleArn;
  for (const container of task.containerDefinitions)
    container.logConfiguration.options['awslogs-group'] = access.LogGroup;
  app.command = [command === 'purge' ? 'privacy-purge' : 'privacy-retention'];
  app.secrets = app.secrets.filter((v) => v.name === 'CLOUD_DATABASE_PASSWORD');
  app.environment = app.environment.filter(
    (v) =>
      ![
        'APP_RUN_MIGRATIONS',
        'PRIVACY_REQUEST_ID',
        'S3_BUCKET',
        'S3_RECOVERY_BUCKET',
        'S3_PREFIX',
        'S3_EXPECTED_BUCKET_OWNER',
        'AWS_REGION',
      ].includes(v.name),
  );
  app.environment.push(
    ...Object.entries({
      APP_RUN_MIGRATIONS: 'false',
      PRIVACY_REQUEST_ID: requestId,
      S3_BUCKET: m.MediaBucketName,
      S3_RECOVERY_BUCKET: m.RecoveryBucketName,
      S3_PREFIX: `${c.mediaPrefix}/`,
      S3_EXPECTED_BUCKET_OWNER: c.accountId,
      AWS_REGION: c.region,
    }).map(([name, value]) => ({ name, value })),
  );
  const taskArn = await isolatedTask(c, task, f, r.ClusterArn);
  console.log(
    JSON.stringify({
      requestId,
      taskArn,
      logGroup: access.LogGroup,
      next:
        command === 'purge'
          ? 'Inspect versions_erased in the owner task evidence. A false value means repeat the same bounded purge. Admin completion still requires provider evidence and expired/removed backups.'
          : 'Inspect expired_personal_evidence counts. Repeat while a count is 100; remaining periods are measured from application erasure. Immutable money journals remain pseudonymous.',
    }),
  );
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
