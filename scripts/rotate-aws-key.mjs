import { randomUUID, createHash } from 'node:crypto';
import { mkdtemp, readFile, writeFile, rm, mkdir, rename } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  aws,
  identity,
  outputs,
  assertRuntimeBinding,
  validateConfiguration,
} from './deploy-aws.mjs';
import { checkpoint, createStack, isolatedTask } from './recover-aws.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
async function main() {
  const [command, configPath, journalPath] = process.argv.slice(2);
  const configBytes = await readFile(configPath, 'utf8');
  const c = validateConfiguration(JSON.parse(configBytes));
  await identity(c);
  await assertRuntimeBinding(c, { checkMfa: command !== 'execute' });
  const [f, r] = await Promise.all([outputs(c, 'foundation'), outputs(c, 'runtime')]);
  const directory = await mkdtemp(join(tmpdir(), 'myfans-rotate-'));
  try {
    if (command === 'prepare') {
      const saved = await checkpoint(c, directory);
      const id = randomUUID();
      const name = `rotation-${id.slice(0, 8)}`;
      const repository = `arn:aws:ecr:${c.region}:${c.accountId}:repository/${c.imageUri.split('.amazonaws.com/')[1].split('@')[0]}`;
      const access = await createStack(c, `${c.deploymentName}-${name}`, 'key-rotation.json', {
        DeploymentName: c.deploymentName,
        RestoreName: name,
        CurrentKeyArn: saved.mfaSecretArn,
        OwnerSecretArn: f.DatabaseOwnerSecretArn,
        DataKeyArn: f.DataKeyArn,
        ImageRepositoryArn: repository,
      });
      const journal = {
        version: 1,
        rotationId: id,
        accountId: c.accountId,
        deploymentName: c.deploymentName,
        checkpointId: saved.id,
        oldKeyArn: saved.mfaSecretArn,
        oldKeyVersion: saved.mfaVersion,
        nextKeyArn: access.NextKeyArn,
        executionRoleArn: access.ExecutionRoleArn,
        logGroup: access.LogGroup,
        imageUri: c.imageUri,
        migrationTask: r.MigrationTask,
        configurationSha256: createHash('sha256').update(configBytes).digest('hex'),
      };
      const path = resolve(ROOT, `.local/aws/${c.deploymentName}/${name}.json`);
      await mkdir(dirname(path), { recursive: true, mode: 0o700 });
      await writeFile(path, JSON.stringify(journal, null, 2), { mode: 0o600, flag: 'wx' });
      console.log(
        JSON.stringify({
          journalPath: path,
          checkpoint: saved.id,
          nextKeyArn: access.NextKeyArn,
          state: 'prepared; services paused; database still uses old key',
        }),
      );
      return;
    }
    if (command !== 'execute')
      throw new Error(
        'Usage: node scripts/rotate-aws-key.mjs prepare|execute CONFIG [JOURNAL_PATH]',
      );
    const journal = JSON.parse(await readFile(journalPath, 'utf8'));
    if (
      journal.version !== 1 ||
      journal.accountId !== c.accountId ||
      journal.deploymentName !== c.deploymentName ||
      journal.imageUri !== c.imageUri ||
      journal.migrationTask !== r.MigrationTask ||
      !/^[a-f0-9-]{36}$/u.test(journal.rotationId)
    )
      throw new Error('Rotation journal does not match the current deployment revision');
    const configMatches =
      createHash('sha256').update(configBytes).digest('hex') === journal.configurationSha256;
    if (!configMatches && c.mfaSecretArn !== journal.nextKeyArn)
      throw new Error('Configuration changed since rotation preparation');
    for (const arn of [journal.oldKeyArn, journal.nextKeyArn])
      if (!arn.startsWith(`arn:aws:secretsmanager:${c.region}:${c.accountId}:secret:`))
        throw new Error('Rotation secret identity mismatch');
    const services = await aws(c, [
      'ecs',
      'describe-services',
      '--cluster',
      r.ClusterArn,
      '--services',
      r.WebService,
      r.WorkerService,
    ]);
    if (services.services.some((v) => v.runningCount || v.pendingCount || v.desiredCount))
      throw new Error('Key rotation requires drained services');
    const task = (
      await aws(c, ['ecs', 'describe-task-definition', '--task-definition', r.MigrationTask])
    ).taskDefinition;
    if (task.containerDefinitions.find((v) => v.name === 'app')?.image !== journal.imageUri)
      throw new Error('Rotation image mismatch');
    task.family = `${c.deploymentName}-key-rotation`;
    task.executionRoleArn = journal.executionRoleArn;
    for (const container of task.containerDefinitions)
      container.logConfiguration.options['awslogs-group'] = journal.logGroup;
    const app = task.containerDefinitions.find((v) => v.name === 'app');
    app.command = ['rotate-key'];
    app.environment = app.environment.filter(
      (v) => !['KEY_ROTATION_ID', 'APP_RUN_MIGRATIONS'].includes(v.name),
    );
    app.environment.push(
      { name: 'KEY_ROTATION_ID', value: journal.rotationId },
      { name: 'APP_RUN_MIGRATIONS', value: 'false' },
    );
    app.secrets = app.secrets.filter((v) => v.name === 'CLOUD_DATABASE_PASSWORD');
    app.secrets.push(
      { name: 'CLOUD_MFA_KEY_HEX', valueFrom: `${journal.oldKeyArn}:::${journal.oldKeyVersion}` },
      { name: 'CLOUD_NEXT_MFA_KEY_HEX', valueFrom: journal.nextKeyArn },
    );
    // The database records rotationId with the ciphertext updates in one transaction.
    // Retrying after an unknown task result is safe; service restart is still gated.
    journal.taskArn = await isolatedTask(c, task, f, r.ClusterArn);
    journal.databaseRotatedAt = new Date().toISOString();
    await writeFile(journalPath, JSON.stringify(journal, null, 2), { mode: 0o600 });
    c.mfaSecretArn = journal.nextKeyArn;
    delete c.mfaSecretVersion;
    const temporary = `${resolve(configPath)}.${randomUUID()}.tmp`;
    await writeFile(temporary, `${JSON.stringify(c, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
    await rename(temporary, resolve(configPath));
    console.log(
      JSON.stringify({
        journalPath,
        taskArn: journal.taskArn,
        state:
          'database re-encrypted; account configuration now references the new retained key; services remain paused',
        next: 'Review/apply runtime change set, rerun migrate, start services, doctor, then capture and verify a new checkpoint.',
      }),
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
main().catch((e) => {
  console.error(e.message);
  process.exitCode = 1;
});
