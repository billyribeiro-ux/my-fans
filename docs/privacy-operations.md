# Privacy erasure and evidence retention

Account erasure is implemented as a durable, reviewed operation. Application erasure, removal of S3 versions, and externally reviewed completion are separate states. A disabled account cannot sign in to retrieve its receipt, so the confirmation dialog offers the request UUID before navigating away. There is no public endpoint that reveals the status of another person's request.

## Operator configuration

Production operator-policy JSON must contain all six integer `retention` fields. The [general policy draft](operator-policy.md) supplies proposed values for review. Choose and publish the final values using the operator's applicable obligations; the draft values are not legally mandated defaults.

| Field                      | Enforcement                                                                                                                                                                                                                                                                           |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `erasure_delay_days`       | Minimum delay between authenticated request and application processing, 0–365 days. Approval and resolved obligations are also required.                                                                                                                                              |
| `backup_days`              | Minimum delay from application erasure before an administrator can attest external completion, 35–3650 days. Must cover the deployed 35-day RDS PITR window; elapsed time alone does not prove manual backups were removed.                                                           |
| `security_days`            | Owner maintenance expires the subject's personal account/password/MFA audit events after this many days from application erasure, and only if the event itself is older.                                                                                                              |
| `financial_days`           | The same owner maintenance expires personal free-text finance review actions for linked checkouts/disputes/resolved issues and duplicate payout-review audit narratives. It preserves amounts, postings, receipt and account relationships, transaction IDs and reconciliation state. |
| `policy_evidence_days`     | Owner maintenance expires registration policy-acceptance evidence and redacts expired privacy-review narratives after the request is completed; decisions and restricted receipts remain.                                                                                             |
| `moderation_evidence_days` | Owner maintenance expires subject-linked moderation snapshots, including reports about the subject's posts, and their duplicate audit narratives; it removes identity-provider applicant links.                                                                                       |

The last four fields accept 1–36500 days. Each request captures the exact schedule and privacy-policy version; a database trigger prevents shortening or changing that captured scope. Sandbox defaults exist for tests and local demonstrations only. Application logs, providers, manually exported files and account backups have separate lifecycle controls and must be included in the operator's record of processing.

## Request and review

The signed-in user submits `POST /api/platform/privacy/erasure` with their current password, MFA code when enrolled, and explicit `ERASE` confirmation. The request transaction disables the account, revokes sessions and recovery tokens, hides posts, stops new content writes and cancels pending media work. Running processors finish under their existing lease; erasure waits for their cleanup. Administrators must transfer responsibilities and remove their administrator role before requesting personal erasure.

Administrators with MFA use the Admin privacy queue. Approval requires another reviewer, a reason, no legal hold, and resolved financial obligations. Database checks cover current subscriptions, uncertain checkouts, payouts, nonzero sandbox balances, open disputes/issues, pending provider transactions and active media assembly/processing. Connected-account balances must also be zero according to the payment provider. Recurring subscriptions are canceled through the provider; already paid service and other financial obligations must reach their actual terminal state. An administrator cannot override these checks by clicking Approve.

Hold/release decisions are available for every unfinished request and recorded in immutable `privacy_actions`. The worker and owner purge lock the request row while processing each bounded batch, so a successful hold prevents subsequent batches. A hold cannot recover objects already erased. Record the hold before authorizing processing when preservation is required.

The worker enumerates media and upload chunks, removes current objects with bounded retries, scrubs personal message/content/mail data, and replaces the account profile with a pseudonymous identifier. Financial foreign keys and journal balances remain intact. `application_erased` records this application boundary; it does not assert that historical versions, backups or providers have erased their copies.

## Remove historical S3 versions

The ordinary web/worker IAM role cannot remove S3 historical versions. Create the dedicated owner maintenance roles once, then process the approved request UUID:

```sh
node scripts/privacy-aws.mjs setup .local/aws-production.json
node scripts/privacy-aws.mjs purge .local/aws-production.json REQUEST_UUID
```

The script validates the configured account and immutable image, launches a private administration task, injects only the schema-owner database credential, and supplies scoped version-list/delete IAM. It prints the task ARN and log group. No application/provider/MFA secret is required or fetched by the operator script.

The task first enumerates primary and recovery object versions into an owner-only queue. It waits for primary-version replication to report `COMPLETED` before enumerating the replica, and recognizes only the configured media prefix and signed-checkpoint UUID object paths. It enumerates both buckets before deleting anything, preserving pagination correctness. Each invocation is bounded; repeat the same command while its log reports `versions_erased:false`. Retries resume exact persisted markers and version IDs, including after partial S3 failures. Investigate a failed replication status and repair replication before retrying; do not weaken this gate.

`versions_erased_at` is written only after the queue is empty. The object queue remains as evidence of exactly what was deleted. This task does not delete audit buckets, RDS snapshots, exported manifests or provider records. Development local storage has no historical object versions and records that boundary during application erasure.

## Expire retained personal evidence

Run the owner maintenance task after a request's captured periods expire:

```sh
node scripts/privacy-aws.mjs retention .local/aws-production.json REQUEST_UUID
```

This command receives no task IAM credentials. Its owner-only `privacy_retention_batch(uuid)` function locks the request, rejects pending application erasure, legal holds or renewed financial obligations, and protects related checkout participants, reporters and reported creators on hold. Each category removes at most 100 eligible rows; repeat while any count is 100. A zero count may mean a future deadline, not that all categories have expired. Review the captured schedule again at the remaining deadlines. Each invocation appends immutable category counts to `privacy_retention_runs`.

The function is not executable by `PUBLIC` or `myfans_runtime`, requires the direct schema-owner session, pins its search path, and enables a transaction-scoped immutable-row exception only for the selected evidence tables. The normal application remains unable to alter or delete evidence. Policy acceptance, security events, personal finance review actions, moderation snapshots and their duplicate audit narratives are deleted only at the selected deadlines. Completed privacy requests retain their decisions, timestamps, holds and restricted completion receipts; expired free-text review reasons become the fixed text `Expired personal review narrative`. This narrowly scoped owner exception preserves every other action column and records the redaction count. Immutable monetary journals, provider account IDs and transaction IDs remain pseudonymous records, not a promise of full anonymity. Provider deletion or retention restrictions must be recorded with the provider separately.

## External completion and recovery

Admin **Record external completion evidence** requires application erasure, historical-version completion, no hold or renewed obligations, and the captured backup interval to have elapsed. The operator supplies three restricted reference identifiers (8–200 ASCII letters/digits or `-_.:/`) for storage, backups and providers. Store the underlying evidence in the operator's controlled evidence system; do not paste names, documents, passwords, tokens or narrative personal data into these references.

Before attesting, inspect and resolve retained manual RDS snapshots, signed checkpoints, exported data, provider-held data, email systems and any independent backups. RDS automated retention alone does not expire manually retained snapshots or exports. The application records the reviewer and the references; it does not verify the truth of an external attestation.

The recovery script creates isolated restore resources and does not cut traffic over. It exports an independently signed privacy journal from the paused source, rejects a journal older than the checkpoint's privacy watermark, omits erased object versions from the new bucket, and replays account freezes, holds and completed erasures in the restored database. Storage rebinding places that database into quarantine; production startup rejects it until the owner replay succeeds. A completed request's tombstone and restricted deletion receipts remain available to reconciliation. Never re-enable an erased account or copy recovered objects into the original live bucket.

If the source database is lost, the [recovery runbook](aws-runtime.md#signed-checkpoints-and-isolated-recovery) accepts the latest independently retained signed journal only with its exact export timestamp explicitly acknowledged. That is a finite recovery point, not proof that later requests never existed. Reconcile the remaining interval using independent records before public cutover. Checkpoints automatically capture a signed journal, and `recover-aws.mjs privacy-journal` captures a newer one during a planned maintenance pause. Actual account acceptance must exercise fresh export, signed-journal tampering rejection, held-account preservation, erased-object exclusion and the quarantine startup gate.

Local verification covers authenticated freeze, session revocation, rejected post races, holds and financial blockers, durable object cleanup, immutable evidence, bounded retention and preserved money values. Actual AWS replication/delete permissions, provider cancellation/erasure, backup removal and recovery reconciliation require the configured account's acceptance exercise; they are not represented as completed by a local test.
