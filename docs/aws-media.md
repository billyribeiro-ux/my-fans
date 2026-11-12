# AWS private media foundation

[media.yaml](../deploy/aws/media.yaml) provisions the S3 media foundation without application compute or a public endpoint. It creates primary, recovery, and audit buckets; two KMS keys; restricted replication and ECS task roles; a regional CloudTrail trail; and a replication-failure alarm. All bucket names derive from the supplied deployment name, region, and account. Every bucket blocks public access, disables ACL ownership ambiguity, enables versioning, requires TLS, and survives stack deletion/replacement. Both KMS keys are retained and rotate automatically. Bucket policies and the audit trail are also retained, preserving transport/encryption restrictions and audit capture after stack deletion. Retained resources continue to incur charges and need an explicit ownership/cleanup plan.

The primary bucket requires uploads to specify its exact KMS key ARN. The Rust application's existing `S3_KMS_KEY_ID` setting supports this. Its task role can inspect the primary bucket and read/write/delete current objects only within the configured prefix. It cannot delete historical versions, administer buckets/keys, or access recovery objects or audit logs. `ListBucket` also authorizes `HeadBucket`; it is scoped to the single bucket without a prefix condition because `HeadBucket` does not carry that request field. Provision a separate ECS execution role when adding compute; this task role is not an execution role.

Replication uses its own role and recovery key. It preserves new object versions in a second bucket in the same region and does not copy delete markers. It cannot permanently delete recovery versions. KMS permissions are bound to S3 in the stack's region and the corresponding object-prefix encryption context. Bucket Keys remain disabled because enabling them changes the encryption context to the bucket ARN and requires a coordinated policy change. Key policies delegate administration to account IAM; the `Resource: '*'` in a KMS key policy denotes that individual key, not every account key. Workload identity policies reference the exact key ARNs.

The regional trail records management activity and primary/recovery object activity, with integrity digests. Server access logs supplement it. Audit-bucket data events and recursive access logging are excluded. Neither application nor replication role can read or delete audit records. The alarm watches `OperationsFailedReplication`; supply `AlertTopicArn` for an approved SNS destination and verify its policy/encryption permits CloudWatch delivery. An alarm without that destination is visible in CloudWatch but sends no notifications.

## Local validation

The October 7 tool pins are cfn-lint 1.57.2 and CloudFormation Guard 3.2.0. Python validator dependencies are hash-locked. Install into the ignored project directory, then validate:

```sh
python3 -m venv .local/cfn-tools
.local/cfn-tools/bin/pip install --require-hashes -r deploy/aws/requirements.lock
cargo install cfn-guard --version 3.2.0 --locked --root .local/cfn-tools
.local/cfn-tools/bin/python scripts/verify-infrastructure.py
```

The script validates the schema for `us-east-1` and `eu-west-1`, enforces nine project policy rules, and checks ten deliberately insecure mutations. It checks public access, ACL ownership, encryption, versioning/retention, key rotation, service trust, replication deletion behavior, audit integrity, TLS denial, and workload object permissions. This is a focused project policy suite, not a compliance certification or complete IAM authorization simulator. CI includes the same gate. No AWS credentials or account mutations are involved in local validation.

## Account preparation and review

Use a dedicated environment and a lowercase `DeploymentName` of 3–20 characters. Supply actual account/region and alert routing through your configuration process. No secrets appear in this template. Review current service costs and account permissions before creating resources: object and version storage, replication requests, KMS keys/requests, CloudTrail data events/log storage, and CloudWatch metrics/alarms incur charges. Old versions and logs have no automatic expiration yet; approve a retention/deletion policy before public operation. Retaining deleted media for recovery has privacy consequences that must be reflected in that policy.

The following example creates a **reviewable change set only**. Replace the illustrative name and provide the approved SNS topic ARN. Use your normal authenticated AWS profile and deployment region; this document does not select an operator account.

```sh
aws cloudformation deploy \
  --template-file deploy/aws/media.yaml \
  --stack-name myfans-media-staging \
  --parameter-overrides DeploymentName=myfans-staging AlertTopicArn=YOUR_APPROVED_SNS_TOPIC_ARN \
  --capabilities CAPABILITY_IAM \
  --no-execute-changeset
```

Inspect account-aware validation events and the actual change set before execution. Verify existing CloudTrail governance for the deployment activity. Local validation does not establish account quotas, naming availability, service permissions, organization policies, regional acceptance, or successful resource creation. This session has not created or executed an AWS change set.

After successful deployment, map outputs `MediaBucketName`, `MediaBucketOwner`, `MediaRegion`, `MediaObjectPrefix`, and `MediaEncryptionKeyArn` to `S3_BUCKET`, `S3_EXPECTED_BUCKET_OWNER`, `AWS_REGION`, `S3_PREFIX`, and `S3_KMS_KEY_ID`. Set `MEDIA_STORAGE=s3`. Use the output task role on the future ECS workload; do not export static access keys. Supply the separately preserved 32-byte application encryption key using `MFA_ENCRYPTION_KEY_FILE`. That application key encrypts MFA/mail and is distinct from both S3 keys.

## Required account acceptance

1. Allow initial versioning configuration to propagate before writing. Run `myfans-api doctor` with the intended workload role and isolated provider-test database. Verify correctly owned bucket access and explicit denial for a wrong account owner, prefix, KMS key, and anonymous request.
2. Upload a harmless synthetic fixture through the application. Verify the primary object uses the required KMS key and that the replica reaches `COMPLETED` before depending on it. Confirm read/delete behavior, retained versions, encryption, trail events and digest delivery. Check that the application cannot read the replica or delete historical versions.
3. Drill a controlled replication failure and confirm the alarm reaches its approved destination. Repair the cause and use the documented S3 repair/batch-replication process for failed objects; enabling a rule does not automatically copy existing data.
4. Stop all writers for a coordinated recovery test. Preserve the database checkpoint, media version mapping, both KMS keys and the external application encryption key. Restore into separate destinations, then verify ledger, authorization, media references, MFA/mail decryption, and revoked-access behavior before routing traffic.

Replication is asynchronous, is in the same account/region, and retains malicious overwrites as additional versions. It does not protect against account/region loss or replace encrypted database backups, point-in-time recovery, external key recovery, version selection, and a tested restoration tool. The local filesystem backup commands deliberately reject S3 deployments. [AWS runtime](aws-runtime.md) supplies compute/TLS, Multi-AZ RDS/PITR, managed secret delivery and coordinated signed S3/database/key restoration. [Privacy operations](privacy-operations.md) supplies scoped historical-version deletion and evidence retention. This selected same-region recovery design does not claim protection from loss of the entire AWS account or region.

Primary references: [S3 security](https://docs.aws.amazon.com/AmazonS3/latest/userguide/security-best-practices.html), [KMS replication permissions and encryption context](https://docs.aws.amazon.com/AmazonS3/latest/userguide/replication-config-for-kms-objects.html), [replication metrics](https://docs.aws.amazon.com/AmazonS3/latest/userguide/repl-metrics.html), [CloudTrail bucket policy](https://docs.aws.amazon.com/awscloudtrail/latest/userguide/create-s3-bucket-policy-for-cloudtrail.html), [ECS task-role trust](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task-iam-roles.html), and [CloudFormation Guard](https://github.com/aws-cloudformation/cloudformation-guard).
