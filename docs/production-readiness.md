# Production activation

The repository supplies the application, production configuration checks, AWS infrastructure and operating workflows. Default local operation remains a sandbox. Public operation requires the operator's real account configuration and successful account acceptance. Follow [AWS deployment](aws-runtime.md), [provider setup](provider-setup.md), [release](release.md) and [privacy operations](privacy-operations.md).

The selected product is a general creator subscription platform with explicit adult content prohibited. Stripe Connect direct charges, Sumsub document/selfie verification, private S3, isolated ClamAV/media processing and SES are integrated. This provider combination must match the actual business and approved merchant account.

## Information to supply

- AWS account/region, domain, hosted zone, certificates, verified SES identity, alert recipient and immutable image references in `deploy/aws/accounts.example.json`.
- Approved Stripe/Sumsub credentials and account/verification settings, SES SMTP credentials, and provider webhook destinations in managed secrets.
- Legal operator identity/address and support/privacy contacts in `deploy/operator-policy.example.json`. Complete general policy text, a US eligibility baseline and six proposed retention periods are supplied; review and adopt them for the actual business using the [policy guide](operator-policy.md).
- GitHub repository, protected release environment and optional ECR publishing role when enabling hosted verification/provenance.
- Operational owners for refunds/disputes, moderation, privacy requests, incident response, account recovery and alerts.

Production startup rejects fixture accounts, sandbox payment/identity configuration, mismatched baked/runtime/mail origins, missing private storage/scanner/SMTP feedback, privileged runtime database users, invalid policy, missing keys and database TLS without hostname verification. Schema administration is a separate owner task. No default administrator is created; bootstrap promotes an existing verified MFA account once.

## Account acceptance

| Boundary       | Repository behavior                                                                                                                                                           | Evidence to collect in the configured account                                                                                    |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Payments       | Durable verified webhooks, idempotency/recovery, paid renewals, refund/dispute revocation, independent transaction/fee/tax/payout reconciliation and audited operator actions | Hosted onboarding, card authentication, renewal/failure, reordered callbacks, refunds/disputes, bank payout and tax agreement    |
| Identity       | Hosted verification, matching approved adult document/selfie, signed callbacks, periodic refresh and revocation                                                               | Configured level, rejected/approved/reversed identity cases and provider approval                                                |
| Mail           | TLS encrypted outbox, leased retries, hard-bounce/complaint suppression and DLQ                                                                                               | DNS/DKIM, actual mailbox receipt, SES simulator feedback and alert delivery                                                      |
| Media          | Private authorized streaming/HLS/ranges, resumable uploads, scan freshness, isolated bounded decoders                                                                         | Target kernel isolation selftest, IAM/KMS denials, current antivirus updates and removal propagation                             |
| Infrastructure | Private origin/ALB, isolated compute pools, Multi-AZ RDS, scoped roles, managed secrets, WAF, telemetry and alarms                                                            | Reviewed change sets, deployment/doctor, alert drills, capacity measurements and database failover                               |
| Recovery       | Signed consistent checkpoints, isolated restores, key binding/rotation and privacy maintenance                                                                                | Actual restore duration, approved RPO/RTO, replication/version permissions, erased-subject reconciliation and rollback rehearsal |
| Privacy/legal  | Versioned policy acceptance, legal holds, reviewed erasure, historical-version queue and owner-only evidence retention                                                        | Approved text/periods, provider/backup removal evidence, support and moderation ownership                                        |
| Release        | Source/dependency/image checks, browser tests, corresponding source bundles and provenance workflow                                                                           | Successful hosted CI/attestations for exact SHA and target-origin images; operator release decision                              |

Independent security/accessibility review and realistic capacity/failure exercises use the configured environment. Local unit, integration, browser and image evidence is recorded in [verification](verification.md), with explicit limits. No automated local result asserts legal compliance, merchant approval or a completed cloud deployment.
