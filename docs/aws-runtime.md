# AWS runtime and recovery

The repository contains deployable infrastructure and operating commands. These files have been checked locally; no AWS account has been provisioned or certified by those checks. The remaining deployment inputs are the AWS account, delegated DNS zone, immutable image digests, provider credentials, approved identity level, sender identity, operator policy text, and the people who receive operational alerts.

The initial region is `us-east-1`. This keeps the CloudFront certificate and global WAF resources in their required region, and provides a concrete, tested template target. Data residency is an operator decision: do not select this deployment if the approved residency policy requires another region. Preflight resolves two account-specific availability zones and excludes `use1-az3`, which does not support CloudFront VPC origins. It also checks the chosen PostgreSQL engine/instance combination against the account's live RDS catalog. [CloudFront VPC origin requirements](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/private-content-vpc-origins.html), [certificate regions](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/cnames-and-https-requirements.html).

## Architecture

- `foundation.json` creates a dedicated VPC, two NAT gateways, private application subnets, isolated database subnets, security groups, PostgreSQL 18.6 Multi-AZ, 35-day point-in-time recovery, retained KMS keys and generated credentials, encrypted logs and alerting, and SES → encrypted SNS → SQS feedback with a dead-letter queue.
- `registry.json` creates retained, encrypted ECR repositories with immutable tags and scan-on-push for the application and scanner images.
- `media.yaml` creates the private versioned media/recovery/audit buckets and scoped application role described in [AWS media](aws-media.md). Its `MediaObjectPrefix` output includes the trailing slash required by the application. The template parameter and account configuration omit that slash.
- `runtime.json` creates distinct web and worker ECS Managed Instances services, separate web/worker/administration capacity pools, an internal TLS ALB, DNS-validated origin certificate, a separate migration task, encrypted logs, health checks, rollback circuit breakers, error alarms, and an operations dashboard. Runtime tasks use the restricted `myfans_app` login; migration credentials are injected only into the administration task.
- `edge.json` creates CloudFront with a private VPC origin, DNS-validated viewer certificate, global WAF, application DNS, and selected-field access logs. Every application response uses `CachingDisabled`, including authorized media. Cookies, queries, and webhook signatures reach the application; CloudFront replaces the viewer `Host` header with the origin domain. The application uses its explicit viewer `ORIGIN`. Error caching is also disabled. [Managed origin policies](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/using-managed-origin-request-policies.html).
- `recovery-access.json` creates an independent execution role for an isolated restore. Live execution roles are never broadened for recovery.
- `key-rotation.json` generates a replacement encryption key inside Secrets Manager and grants only its temporary owner task access to the old and new key references.
- `privacy-access.json` supplies a separate owner execution role and narrowly scoped historical-version purge role. [Privacy operations](privacy-operations.md) covers authenticated requests, holds, evidence expiration, historical copies and reviewed external completion.
- `github-release-role.json` optionally grants a single repository's protected `release` environment permission to publish to the two existing ECR repositories. It requires the account GitHub OIDC provider, exact repository/environment trust claims, and operator-configured branch/environment protections. It grants no deployment or secret access. [GitHub AWS OIDC configuration](https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-aws).

Only CloudFront's managed origin-facing prefix can reach the internal ALB on 443; only that ALB can reach Node on 3000; only application/migration tasks can reach PostgreSQL on 5432. Rust and ClamAV bind loopback. Viewer → CloudFront → ALB uses TLS. ALB → Node uses HTTP within the private VPC and security-group boundary. PostgreSQL clients verify the server hostname and bundled RDS CA. Provider requests use HTTPS; SES SMTP requires STARTTLS on 587.

Two web tasks and two worker tasks are the initial availability floor. Workers receive 2 vCPU/8 GiB; ClamAV reserves 4 GiB and has a 5 GiB cap to allow concurrent signature reloads. Web tasks receive 1 vCPU/2 GiB. These are explicit starting allocations, not a measured production capacity claim. Confirm p95 latency, database connections, queue age, memory, and media processing time during account acceptance before choosing scale limits. [ClamAV memory requirements](https://docs.clamav.net/manual/Installing/Docker.html).

ECS Managed Instances is selected for the media sandbox's kernel requirement. AWS reported its move to Linux 6.18 on September 14, 2026; Fargate's update was still pending. The application requires Landlock ABI 3 or later and tests actual Landlock/seccomp enforcement; a kernel version string alone is insufficient. `start` runs the production doctor on managed capacity before enabling web replicas. [AWS kernel rollout evidence](https://github.com/aws/containers-roadmap/issues/2811#issuecomment-5657598011).

Capacity uses current-generation ARM64 On-Demand instances, private subnets and automatic repair. Separate capacity providers prevent web, media and owner jobs from sharing a host; `distinctInstance` separates replicas within each service. Managed hosts have no inbound security-group rules and HTTPS-only control-plane egress. AWS maintains their operating system and replaces instances within 14 days. Services must tolerate that replacement. The instance profile has compute-agent permissions, with no application database/provider secrets. The infrastructure role's `iam:PassRole` permission is explicitly restricted to that profile's role. Host storage uses a 100 GiB EBS data volume; application state remains in RDS/S3. [Managed instance security](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/managed-instances-security.html), [compute lifecycle](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/ManagedInstances.html), [instance-profile permissions](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/managed-instances-instance-profile.html).

ClamAV runs non-root with a read-only root filesystem, no Linux capabilities, bounded scanning limits, and its own writable signature/scratch volume. It cannot mount application secrets. ECS assigns an IAM role per task, so the scanner shares its worker's task-role boundary; it is not a separate IAM sandbox. Web requests do not run media processing. Each worker emits a heartbeat only after a completed tick; ECS restarts workers with a stale heartbeat.

The scanner builds ClamAV 1.5.4 from its checksum-pinned official source using Rust 1.99 and an Alpine 3.24 runtime. Only signature data comes from the official Debian image. The checked-in scanner Cargo lock applies compatible dependency corrections; audit that lock as well as the final image. Scanner input/file limits match the 256 MiB video contract, aggregate scan bytes are limited to 512 MiB, and the daemon scan budget is 25 seconds inside the application's 30-second deadline. The restricted local boundary check accepted a 256 MiB benign stream in 16.7 seconds; repeat acceptance on the selected hosts rather than treating this as a universal latency guarantee. Worker EMF telemetry uses namespace `MyFans` and exact `Deployment` dimension: missing worker emissions for five minutes, failed jobs, open finance issues, scanner availability, queue age over 120 seconds and SMTP backlog feed alarms and the operations dashboard. Missing worker/scanner data is treated as unhealthy; intentionally paused services can therefore alarm.

WAF uses IP reputation, known-bad-input, common-rule, and per-client rate controls. `SizeRestrictions_BODY` counts rather than blocks because legitimate media uploads exceed its threshold; the application still enforces its own upload/webhook limits. WAF request sampling is disabled. Block logs redact cookies, authorization, signatures and query strings. CloudFront standard logging v2 retains status/timing/path/request-id fields and excludes cookies, query strings, referer and authorization. Application logs omit request bodies. [CloudFront selected-field logging](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/standard-logging.html).

## Supply account information

Copy `deploy/aws/accounts.example.json` into an ignored `.local/aws-production.json` and replace its example values. Configuration contains references, never secret values. Application and scanner images must be ARM64, scanned, account-owned ECR digest references. Build the application for the exact `https://domainName` origin. Production startup compares the immutable `/app/web-origin` baked into the image with `PUBLIC_ORIGIN` and `MAIL_PUBLIC_ORIGIN`; changing the domain requires rebuilding the application. Use the scanner Dockerfile and retain both tested images; version tags are rejected.

Create two customer-KMS-encrypted Secrets Manager secrets using your secure account workflow:

1. Provider secret: JSON keys `stripe_secret_key`, `stripe_webhook_secret`, `sumsub_app_token`, `sumsub_secret_key`, `sumsub_webhook_secret`, `smtp_username`, and `smtp_password`. Use approved **live** credentials for production. Configure Stripe's pinned webhook API version and Sumsub's production webhook/verification level as described in the provider setup document. SES SMTP credentials are region-specific.
2. Operator-policy secret: the complete JSON document accepted by `OPERATOR_POLICY_FILE`, including legal operator identity, support/privacy contacts, versioned terms/privacy/content/refund text, eligible countries and the six explicit [retention periods](privacy-operations.md). Keep this secret within Secrets Manager's 64 KiB value limit. Use `registration_enabled: false` until account acceptance is complete.

Start with [operator-policy.example.json](../deploy/operator-policy.example.json), which supplies general terms, privacy, content and refund drafts plus proposed retention values. Replace its deliberately invalid operator identity/contact placeholders and review the policy, eligibility and retention choices for the actual business using the [policy guide](operator-policy.md). The draft cannot activate production unchanged.

The foundation generates the runtime database password and a 64-character random hexadecimal MFA encryption key inside AWS. The latter decodes to exactly 32 bytes. RDS generates and rotates the owner password using Secrets Manager. No script retrieves secret values. ECS injects secrets at task launch, and `cloud-entrypoint.mjs` writes exclusive-create `0600` files on an encrypted ephemeral volume, removes the injected `CLOUD_*` values before spawning children, and passes file paths to the application. A separate short-lived root init container only sets volume ownership; application containers run as UID 1000. [ECS secret injection and rotation behavior](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/secrets-envvar-secrets-manager.html), [RDS managed credentials](https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/rds-secrets-manager.html).

Before running the commands, authenticate an operator/deployment role in the target account. It needs the explicitly selected CloudFormation/IAM/ECS/ECR/network/RDS/DNS/ACM/CloudFront/WAF/logging/KMS/SES/SNS/SQS/S3 administration permissions. Keep that role separate from runtime roles. Confirm CloudFront managed-prefix-list security-group quota (the list has a large rule weight), EC2 Standard On-Demand vCPU, NAT/EIP, RDS, certificate and SES quotas. Organization AMI controls must allow the ECS Managed Instances image owner in `us-east-1`, `260073348889`. DNS must be delegated to the provided public zone. Confirm the alarm email subscription after foundation deployment. These account quotas and provider approvals cannot be proven by local schema validation.

Enable EBS encryption before any compute launch. This idempotent command changes the regional account default for future EBS volumes; it does not rewrite existing volumes or select a different default key. Preflight refuses an unencrypted default, resolves its enabled KMS key and grants the infrastructure role only that key's required EC2 permissions. If a customer-managed default key is used, its key policy must permit the deployed infrastructure role through IAM delegation. Do not disable this account control while the platform operates. [Managed instance storage encryption](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/ManagedInstances.html).

```sh
node scripts/deploy-aws.mjs bootstrap-storage .local/aws-production.json
```

Bootstrap image repositories first. The registry stage needs only `accountId`, `region`, and `deploymentName`; it does not require already-published images or provider secrets.

```sh
node scripts/deploy-aws.mjs plan .local/aws-production.json registry
node scripts/deploy-aws.mjs apply .local/aws-production.json CHANGE_SET_ARN
# After registry stack completion and local image tests/scans:
node scripts/publish-aws.mjs .local/aws-production.json sha256:APP_IMAGE_ID sha256:SCANNER_IMAGE_ID
```

Publication rejects an application built for another domain and non-ARM64 images. The ECR login token passes directly from AWS CLI stdout to Docker's password input, never through command arguments or tool output. The command returns immutable ECR digests to place in the account configuration. Retain local SBOM/advisory evidence and confirm ECR scan findings before deployment. Repository scan-on-push provides additional registry evidence; it does not replace application lockfile audits or local release tests.

## Deploy in stages

```sh
node scripts/deploy-aws.mjs preflight .local/aws-production.json
node scripts/deploy-aws.mjs plan .local/aws-production.json foundation
node scripts/deploy-aws.mjs apply .local/aws-production.json CHANGE_SET_ARN
```

`plan` creates a reviewable CloudFormation change set without executing it. Review its changes and account-aware validation in CloudFormation. `apply` accepts only the exact executable change-set ARN for this deployment/account. Wait for stack completion, then repeat `plan` and `apply` for `media`, then `runtime`. Runtime services begin at zero so schema administration finishes before traffic can reach an application.

```sh
node scripts/deploy-aws.mjs migrate .local/aws-production.json
# Plan/apply edge after runtime and migrations finish.
node scripts/deploy-aws.mjs plan .local/aws-production.json edge
node scripts/deploy-aws.mjs apply .local/aws-production.json CHANGE_SET_ARN
node scripts/privacy-aws.mjs setup .local/aws-production.json
node scripts/deploy-aws.mjs start .local/aws-production.json
node scripts/deploy-aws.mjs doctor .local/aws-production.json
```

The migration command runs the immutable application's SQL migrations and then provisions/regrants the restricted login. `start` requires both local evidence and an AWS-confirmed successful migration task for the current task-definition revision, then runs doctor on the actual worker platform. ECS retains stopped task information for a limited time; rerun the idempotent migration if that evidence has expired. CloudFormation service counts remain zero in the template; the launcher owns operational scaling counts from the account configuration. This intentional drift is visible and documented.

Register and verify the intended administrator, enroll MFA, and bootstrap that existing account:

```sh
node scripts/deploy-aws.mjs bootstrap-admin .local/aws-production.json admin@example.com
```

This command refuses to run if an administrator already exists or the selected account lacks verified email and MFA. It does not generate default users or publish credentials. For initial enrollment while registration is disabled, use a tightly controlled acceptance window: publish the approved policy with `registration_enabled: true` through the secure Secrets Manager workflow, run `reload-policy` below against that exact local public document, and wait for successful public verification before enrolling the operator. After enrollment/bootstrap, publish the approved document with the flag false, reload both services again, and verify that false is served before treating the window as closed. Changing a secret alone does not update existing tasks. Do not open general registration until account acceptance completes.

SES must have production access, a verified sending identity/DKIM, an approved MAIL FROM setup and SMTP credentials. The application sends the generated `SES_CONFIGURATION_SET` header. SNS can publish only from that configuration set; SQS accepts only that topic/account. Raw delivery passes SES JSON to the worker, which validates account identity and records suppression. Drill hard bounces and complaints using SES's mailbox simulator, verify application suppression and DLQ/queue-age alerts, and confirm the operator receives actual alerts.

## Updates and secret changes

This release uses a maintenance-window migration strategy. It does not promise zero-downtime arbitrary schema rollback.

1. Create and verify the signed checkpoint below. Checkpoint capture drains both services and leaves them paused.
2. Build, test, scan and publish the next immutable image; update the digest in the account file.
3. Review/apply the `runtime` change set with counts zero, run `migrate`, then `start` and `doctor`.
4. Run the browser/provider acceptance suite against the real domain. If schema verification fails, keep services paused and restore into isolated resources; never point an older binary at an unreviewed newer schema.

A provider-secret version change is consumed only by new tasks; `aws ecs update-service --force-new-deployment` is required for each service after a reviewed provider rotation. Webhook-secret rotation must also coordinate provider event delivery. Rotating the runtime database password requires drained services, updating the generated secret through the operator's secure workflow, running `migrate` to alter the login, and restarting both services. RDS owner-password rotation does not affect runtime logins.

Operator policy is also loaded once at task launch. Save the approved **public policy document** in an ignored local file and publish those same bytes to `policySecretArn` using the account's secure secret workflow. Then replace both services and verify the complete served document:

```sh
node scripts/deploy-aws.mjs reload-policy .local/aws-production.json .local/operator-policy.approved.json
# Read-only verification can be repeated independently:
node scripts/deploy-aws.mjs verify-policy .local/aws-production.json .local/operator-policy.approved.json
```

`reload-policy` preserves current replica counts, requires stable services and the configured runtime revision, checks secret-version metadata before/after replacement, and requires the exact new deployments to complete rather than accepting a circuit-breaker rollback. It compares the complete production JSON at `https://domainName/api/platform/site` with the approved document, including registration, countries, text, versions and retention. It prints only a canonical document hash, version ID and registration state. It never retrieves a secret value, writes the policy or enables registration itself. A failed command is not successful policy adoption: inspect service events, correct the approved document/secret and repeat. Do not rotate other secrets or update deployments concurrently.

Rolling replacement temporarily serves both policy versions. For an immediate registration closure or a policy change that cannot tolerate that overlap, first `pause`, publish the approved secret, rerun the idempotent `migrate` if its stopped-task evidence expired, then `start` and `verify-policy`. Pausing stops all serving replicas; `start` launches fresh tasks that read the current policy. Keep the maintenance window in effect until exact public verification succeeds. [ECS launch-time secret behavior](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/secrets-envvar-secrets-manager.html), [deployment rollback state](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/deployment-circuit-breaker.html).

Rotate the MFA/outbox key through the transactional workflow:

```sh
node scripts/rotate-aws-key.mjs prepare .local/aws-production.json
node scripts/rotate-aws-key.mjs execute .local/aws-production.json JOURNAL_PATH
# Review/apply the runtime change set, then migrate, start and doctor.
```

Preparation drains services, creates a signed checkpoint, generates the new retained key inside AWS and records a rotation UUID. Execution uses an isolated owner task with exact old-key-version and new-key injection. The Rust command re-encrypts MFA/outbox ciphertext and records the rotation UUID/key fingerprints in one transaction; retrying the same operation after an uncertain task result is idempotent. After success, the tool atomically updates the ignored account file's `mfaSecretArn` reference. Services remain paused until the reviewed runtime change set injects that key and verification succeeds. No key bytes are fetched by the operator scripts.

**Do not replace the MFA encryption key as an ordinary secret rotation.** Existing MFA and outbox ciphertext require the matching key. Signed checkpoints preserve exact secret versions using `RECOVERY-<version>` staging labels. Retain old and new secrets and their KMS keys until every dependent checkpoint has passed its approved retention period. Historical restore uses the manifest's signed key ARN/version, even after the live key reference changes. The asymmetric checkpoint signing key has no automatic-rotation support: use a new signer for new checkpoints and retain old verifiers with historical evidence.

## Signed checkpoints and isolated recovery

RDS continuously provides a 35-day PITR window; S3 continuously retains object versions and asynchronous replicas. A coordinated release checkpoint adds a quiesced database snapshot, immutable recovery copies of every current media object, a SHA-256 checksum per copy, pinned application/scanner images and exact worker/migration task-definition revisions, the MFA key version, and a KMS ECDSA signature over the complete manifest.

```sh
node scripts/recover-aws.mjs checkpoint .local/aws-production.json
node scripts/recover-aws.mjs verify .local/aws-production.json checkpoint-UUID
node scripts/recover-aws.mjs restore .local/aws-production.json checkpoint-UUID
```

`checkpoint` drains web and workers and waits for zero running/pending tasks, exports and signs a privacy journal, then takes the database snapshot and lists media versions. The checkpoint records that journal's watermark. Do not run concurrent schema/key administration during capture. It copies selected source versions to the retained recovery bucket under a unique checkpoint prefix, validates the snapshot and key version, signs the manifest using the foundation's trusted KMS signer, and uploads it with create-only semantics. Failure leaves services paused and partial resources retained for investigation. Resume by rerunning the idempotent `migrate` and `start` commands once the checkpoint is verified.

`verify` validates deployment/account/image binding, verifies the KMS signature against the configured foundation key, checks the encrypted database snapshot identity, and checks recovery objects' immutable versions, sizes and SHA-256 checksums. It does not trust a signer supplied by an arbitrary downloaded manifest. If historical erasure has removed checkpoint objects, supply the independently signed privacy journal ID as the next argument; only its exact erased keys are excluded from object checks.

`restore` pauses the source services, exports a fresh signed privacy journal using the **current** runtime, and repeats verification. Before creating a database or bucket, it resolves the checkpoint's signed historical task revisions, checks their exact image/entrypoint/provider/origin contract, verifies both ECR digests remain available, and checks required secret metadata and the retained historical key version. The source configuration keeps its current image digests; the isolated restore uses the checkpoint's image digests. The source runtime is never rolled backward for this normal recovery path.

It creates a **new** private Multi-AZ RDS instance and a **new** media stack. It never overwrites the original database or bucket. PostgreSQL's snapshot restore API cannot request managed master credentials during restore; the tool enables RDS management immediately afterwards on the newly restored instance. It copies only signed checkpoint object versions that are absent from the verified journal's erasure set, checks their checksums, creates scoped recovery execution/replay roles, and runs four isolated tasks: migrations/restricted-role provisioning, conditional storage rebinding into quarantine, mandatory privacy replay, and authenticated ciphertext verification using the exact historical MFA key. Normal application startup refuses a quarantined restored database until replay succeeds. Recovery verification does not start a worker loop or process external transactions. [RDS snapshot restore options](https://docs.aws.amazon.com/cli/latest/reference/rds/restore-db-instance-from-db-snapshot.html).

Worker and migration task definitions have `DeletionPolicy: Retain` and `UpdateReplacePolicy: Retain`. Retain those revisions, both referenced ECR images, corresponding release source and key versions until all dependent checkpoints expire. Do not call `DeleteTaskDefinitions` on a required revision. An `INACTIVE` revision can still be described and cloned into a new isolated recovery task; a deleted one cannot. [ECS task-definition states](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task-definition-state.html). Missing history fails preflight before recovery resources are created.

**Legacy checkpoints:** a manifest created before task revisions were included is accepted only when the current task definitions match its image and launch contract. If a newer release is active, preflight fails closed. To recover such a checkpoint, first export the latest privacy journal using the current configuration and keep both services paused. Run `verify` to obtain the independently signature-verified historical image digests, copy the source account configuration to an ignored historical configuration and set those two image references. From the retained release source corresponding to that checkpoint, review/apply only the historical `runtime` change set with both desired counts zero. Do **not** run its migration, doctor or start commands against the current source database. With the current recovery tooling, run `restore HISTORICAL_CONFIG CHECKPOINT_ID SIGNED_PRIVACY_JOURNAL_ID EXACT_EXPORTED_AT`; the supplied signed journal avoids executing old export code on the source database. Use that same historical configuration for promotion preparation. If the archived contract, images, key or journal are unavailable, recovery stops for investigation rather than substituting an unverified newer launch contract.

Create an additional independent privacy journal during a planned maintenance pause when the required privacy recovery point is newer than the last checkpoint:

```sh
node scripts/recover-aws.mjs privacy-journal .local/aws-production.json
```

If the source database is unavailable in a disaster, use the latest independently retained signed journal. Supply its exact export timestamp as an explicit recovery-point acknowledgment; the launcher verifies the signature, exact S3 version and content hash, deployment/storage identity, and a watermark no older than the checkpoint:

```sh
node scripts/recover-aws.mjs restore .local/aws-production.json checkpoint-UUID privacy-UUID EXACT_EXPORTED_AT
```

This fallback covers requests recorded through that timestamp. It cannot prove what happened after a lost journal's recovery point. Record that interval and reconcile independent request/provider evidence before public cutover. No process can infer missing erasure requests from an earlier snapshot. Owner export has only create-only S3 write permissions under `privacy-journals/`; replay has only versioned reads, with no journal write or signing permissions. Journal data contains request UUIDs, captured schedules/holds and erased object keys, not user names, emails or request narratives. Both the data and signed envelope remain encrypted and versioned off host.

The recovery binding command requires an explicit recovery flag, exact restored hostname, migration-owner role, no active runtime connections, and the expected previous storage hash under an exclusive transaction lock. The launcher independently checks that the restored RDS identity/endpoint and target media bucket differ from the originals. Those controls make a reviewed recovery operation deliberate; access to the privileged operator role remains the security boundary.

The tool writes resource/task evidence, the privacy journal identity/hash/recovery point and replay task ARN to `.local/aws/<deployment>/restore-*.json`. Restore resources have deletion/termination protections and are retained for inspection; source services remain paused. A restore drill consumes additional RDS/NAT/storage/logging resources; remove them only through a separately reviewed cleanup after recording evidence and retention requirements. The tool performs no automatic traffic switch. A real incident requires reviewing the recovered state and reconciling provider events/payments that occurred after the checkpoint before promoting recovered resources.

### Promote a verified restore

Promotion uses the same retained network, cluster, ALB, CloudFront domain and provider identities. It changes the active database, media role/bucket/key, checkpoint application/scanner images and exact historical MFA-key version through a reviewed runtime change set. The prepared configuration takes image digests from the verified signed checkpoint, even when the paused source used a newer release. The original foundation database and both original storage buckets remain intact. Do not edit resource IDs into a configuration by hand or update the foundation database to force replacement.

While the source remains paused, reconcile provider transactions, webhook outcomes and privacy requests through the proposed recovery point. Save restricted evidence identifiers in an ignored acceptance file; the referenced records must describe the actual review, including any interval after a disaster journal:

```json
{
  "checkpoint": "checkpoint-UUID",
  "privacyJournalId": "privacy-UUID",
  "privacyExportedAt": "EXACT_EXPORTED_AT_FROM_RESTORE_EVIDENCE",
  "recoveryPointAccepted": true,
  "changeReference": "change:APPROVED_INCIDENT_ID",
  "providerReconciliationReference": "finance:RECONCILIATION_RECORD_ID",
  "privacyReconciliationReference": "privacy:RECONCILIATION_RECORD_ID"
}
```

Use the exact source configuration that produced the restore. Prepare promptly while ECS still retains the stopped task status; if that evidence has expired, the tool fails closed and a new isolated restore is required. Preparation is read-only in AWS and exclusively creates a new local configuration. It verifies the trusted checkpoint/journal signatures, AWS task exit status and actual commands/target bindings, private encrypted Multi-AZ database identity, exact restored media stack, retained MFA version, and every current destination object against the signed checksum set. Unexpected or erased objects prevent promotion.

```sh
node scripts/promote-aws.mjs prepare .local/aws-production.json .local/aws/DEPLOYMENT/restore-SUFFIX.json .local/recovery-acceptance.json .local/aws-recovered.json
node scripts/privacy-aws.mjs setup .local/aws-recovered.json
node scripts/deploy-aws.mjs plan .local/aws-recovered.json runtime
# Review the exact change set: expected database host/owner secret, media role/bucket/KMS,
# pinned MFA version, active database alarms/dashboard, and both desired counts zero.
node scripts/deploy-aws.mjs apply .local/aws-recovered.json REVIEWED_CHANGE_SET_ARN
# Wait for UPDATE_COMPLETE; the existing public origin remains without serving replicas.
node scripts/deploy-aws.mjs migrate .local/aws-recovered.json
node scripts/deploy-aws.mjs doctor .local/aws-recovered.json
# After target/provider/privacy acceptance, explicitly enable the recovered services.
node scripts/deploy-aws.mjs start .local/aws-recovered.json
node scripts/deploy-aws.mjs status .local/aws-recovered.json
```

`start` verifies the current migration revision and reruns production diagnostics before adding replicas; this is the explicit traffic activation step. DNS and the edge stack do not change. The runtime's three database alarms and capacity dashboard follow the recovered database identifier. Owner execution IAM follows its RDS-managed secret, application IAM follows the recovered media role/KMS key, and new privacy maintenance roles have separate logs and permissions for the recovered buckets. Original resources and permissions remain retained for evidence and reviewed cleanup.

Use `.local/aws-recovered.json` for **all subsequent** runtime deployments, checkpoints, journal export, erasure/retention maintenance and key rotation. The tools resolve active bindings from AWS each time and refuse mismatched runtime tasks. New checkpoints bind the recovered database and new recovery bucket. The exact historical MFA secret version remains pinned through ordinary deployments/checkpoints; only a successful reviewed `rotate-aws-key` transaction changes the key reference and removes that version pin. Historical [ECS secret version injection](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/secrets-envvar-secrets-manager.html) does not change any existing secret value.

Retain the source configuration and signed evidence for historical verification; do not use it to administer the active runtime. Capture a new signed checkpoint after promotion. Reverting traffic after new transactions have occurred is another recovery/reconciliation operation, not simply restarting the retained original database. Old DB snapshots, original object versions, old journal envelopes and provider records remain subject to the approved privacy/backup retention review before external erasure completion.

The bounded checkpoint implementation supports up to 100,000 current objects of at most 512 MiB each, with a 32 MiB manifest limit, matching the application's bounded object store. Exceeding that bound fails capture before a valid manifest is published. A larger platform needs a partitioned manifest/job workflow before increasing this limit. This is an explicit operating limit, not an unbounded backup claim.

## Verification and account acceptance

```sh
.local/cfn-tools/bin/python scripts/verify-infrastructure.py
.local/cfn-tools/bin/python scripts/verify-cloud-infrastructure.py
node --test scripts/cloud-entrypoint.test.mjs scripts/cloud-operations.test.mjs
```

The cloud checks run `cfn-lint` and project Guard policies on eight registry/runtime/recovery/rotation/privacy/release templates, including 32 insecure-mutation controls for mutable/unscanned images, public DB access, lost HA/backups, unretained keys or historical task revisions, public ingress, forged mail publishers, privileged containers, scanner secret-volume access, public tasks, public origins, private-response caching, sampled WAF secrets, broad privacy deletion/journal access and wildcard GitHub trust. Nineteen bootstrap/operation tests cover permissions, symlink refusal, DSN encoding, role separation, rotation key files, secret removal, policy reload/version races and rollback, exact public policy, deployment/image/signer binding, historical task selection and missing-history failure, bounded checkpoint serialization, exact S3 prefix identity, duplicate keys/subjects, unsafe privacy keys, invalid object versions, recovery account/resource binding, mandatory reconciliation references and failed or mismatched replay task evidence.

Before public registration, perform actual account acceptance: TLS and DNS; signed provider webhook replay/deduplication; checkout/renewal/refund/dispute and provider accounting; live identity decision/revocation; clean/infected/stale-scanner uploads; SES bounce/complaint suppression; cross-user media denial; alarm delivery; AZ replacement/RDS failover; a signed checkpoint and isolated restore; and browser/accessibility/load tests at the real domain. Record the account, image digest, timestamps and results. Local checks do not substitute for these external-service observations. PostgreSQL 18.6 was verified against [the official RDS release notes](https://docs.aws.amazon.com/AmazonRDS/latest/PostgreSQLReleaseNotes/postgresql-versions.html); the launcher still verifies regional availability before deployment.
