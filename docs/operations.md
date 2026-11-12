# Operating the sandbox

The development instance uses PostgreSQL on loopback port 54328 and host `.local/media`; the container bundle uses `bundle-postgres` and `media_data`. Never mix their database and media backups. This guide covers local sandbox operation. Use the separate [AWS runtime procedures](aws-runtime.md), [provider setup](provider-setup.md), [finance operations](finance.md) and [privacy operations](privacy-operations.md) for the implemented production workflows.

## Start, stop, and inspect

Development: complete the platform-specific [native media setup](../README.md#start-development), then `pnpm db:start`, `pnpm db:seed` and `pnpm dev`. Stop the app with Ctrl-C; PostgreSQL remains running. The development supervisor stops both Node and Rust. Seed only disposable/sandbox databases. Seed credentials are public fixtures.

Bundle:

```sh
docker compose --profile bundle up -d --build --wait app
docker compose --profile bundle exec app myfans-api seed
docker compose --profile bundle logs --tail 100 app
docker compose --profile bundle stop app
```

The API exposes `/health/live` and `/health/ready` on its loopback listener. Readiness checks the database; it does not establish payment, email, media processing, or provider health. A healthy HTTP listener is not launch acceptance. The bundle is accessible at http://127.0.0.1:3000 only.

Rust handles SIGINT and SIGTERM. It drains HTTP requests and lets a claimed job finish before closing the database pool. The supervisors and Compose allow 150 seconds before forced termination; a forced stop leaves a leased job eligible for recovery. Do not kill a database or remove a volume to stop an application.

## Worker separation

An embedded worker runs by default. Set `APP_EMBEDDED_WORKER=false` to run an API without a worker, and run `cargo run --locked -p myfans-api -- worker` separately with the same database, media directory, and encryption key. The packaged executable accepts `myfans-api worker`. Multiple workers claim jobs using PostgreSQL row locks and `SKIP LOCKED`; shared media storage is necessary. Media decoders also run behind a tested Landlock/seccomp sandbox on Linux. Leased jobs heartbeat every 30 seconds; lease loss cancels work and stale claims recover after five minutes. Production worker/compute/decoder limits are configured separately; measure capacity in the target account.

Jobs have five-minute leases, five attempts, and exponential retry delays. Admin → Operations lists failed jobs and permits audited retry. Confirm the cause is corrected before retrying. Image/video rejection is distinct from a retryable job failure; invalid files need a new upload. Settlement and publication revalidate durable state and run idempotently. Jobs awaiting media processing, settlement, or scheduled publication require an active worker.

## SMTP delivery

External email is off by default. Configure an approved authenticated relay using the variables in README and mount the password file privately into the runtime. Preserve certificate validation; only implicit TLS or required STARTTLS are supported. Set the public email origin to the frontend's configured origin. Verify the sender domain, SPF/DKIM/DMARC, service/content eligibility, and actual delivery to operator-controlled mailboxes before launch. No external email was sent during local verification.

The worker runs mail delivery independently of media/finance jobs. A delivery has a five-minute lease, at most eight attempts, exponential backoff capped at 15 minutes, a ten-second SMTP operation timeout and a twenty-second total send deadline. Verification/reset messages stop being eligible when their 30-minute link expires or is consumed. Stable Message-IDs survive retries, but a crash or lost SMTP acknowledgment may cause duplicates. A `sent` record means relay acceptance, not proof of mailbox delivery. The production SES integration correlates hard-bounce/complaint feedback with the account, configuration set, delivery and recipient before recording suppression; exercise the [SES acceptance procedure](aws-runtime.md#deploy-in-stages) with the configured account.

SMTP bodies are encrypted with the instance key and authenticated against their message ID and recipient. Completion or permanent failure erases their ciphertext. No browser mailbox endpoint exposes SMTP messages; switching back to sandbox reveals only messages created as sandbox fixtures. Existing fixture mail is never sent externally. Monitor pending/sending/failed delivery counts and oldest eligible age through the database and `mail_delivery_completed` logs; these logs contain IDs/status/attempts only. Terminal failures require correcting the service configuration and issuing a new verification or recovery message, rather than replaying erased bodies.

## Provider payment recovery

Stripe event reconciliation runs in independently leased connected-account jobs. A provider failure or expired event-history checkpoint holds that account for investigation while other accounts continue. Stale checkouts have separate jobs: known sessions are read from Stripe, missing session IDs are discovered through durable bounded pages, and intents that never attempted creation expire safely. An uncertain attempted payment remains reserved. Use the checkout ID and connected account in the approved provider console to establish the outcome; do not clear the reservation, rewrite financial rows, or issue another charge based only on a missing webhook. Retry a failed recovery job after correcting the provider condition. Use the Admin finance recovery workflow for indeterminate outcomes; it requires recent MFA, a reason and verified provider evidence. An unknown outcome cannot be declared unpaid from an empty search. See [finance operations](finance.md).

## Consistent backups

Quiesce **all** application and worker processes using the database/media pair before taking a backup. Prevent other SQL writers for the duration. A PostgreSQL dump and a filesystem archive taken during processing are not a common snapshot. Scripts refuse a running local API or bundle app, but cannot discover every independently launched worker or external database writer.

Development: stop `pnpm dev` and separate workers, then:

```sh
bash scripts/backup-local.sh .local/backups/host-snapshot host
bash scripts/verify-backup.sh .local/backups/host-snapshot
pnpm dev
```

Bundle:

```sh
docker compose --profile bundle stop app
bash scripts/backup-local.sh .local/backups/bundle-snapshot bundle
bash scripts/verify-backup.sh .local/backups/bundle-snapshot
docker compose --profile bundle up -d --wait app
```

Choose a new destination for each backup. The private directory contains a custom-format database dump, media archive including `.mfa-key`, mode marker, and relative SHA-256 manifest. Verification requires the matching built app image: it restores into a newly named disposable database, verifies balanced posted transactions and referenced processing/approved/quarantined files, then runs `myfans-api verify-storage` against the restored pair with migrations disabled and media mounted read-only. This decrypts every stored MFA secret and queued SMTP body in bounded pages without printing secrets or starting listeners/workers. A missing, wrong, or damaged key/ciphertext fails verification. Only the temporary restore is removed; live database/media remain untouched. Backups from an older schema require their matching release image.

These archives contain private content and account data. They are not encrypted by these local scripts. Encrypt and restrict off-host copies, define retention, and test the approved key recovery procedure before production. Do not commit or attach archives to tickets. Startup publishes a complete new key atomically across concurrent processes and refuses to generate a replacement when encrypted MFA secrets already exist. Losing `.mfa-key` makes stored MFA secrets and queued SMTP bodies unreadable. Startup authenticates all stored ciphertext and rejects a correctly sized wrong key; replacing the key does not recover data.

## Restore and rollback

Practice restoration into new database and media destinations first. Keep application/worker writers stopped, verify the manifest, restore the dump with `pg_restore --exit-on-error`, and extract the media archive into a separate private directory. Verify database/media references before configuring the app to use both restored destinations. Test login with MFA, media access, ledger balance, and queue recovery. Switch traffic only after verification and retain the original destinations until rollback acceptance. Never use a dump from one instance with media/key material from another.

Migrations run on startup unless `APP_RUN_MIGRATIONS=false`. A migration owner may run `myfans-api migrate`; a runtime login granted `myfans_runtime` must not own the schema. `db/roles/runtime.sql` contains the reviewed grant set. The runtime checks all applied migration versions, checksums, and success states before starting with automatic migrations disabled. Migration files are immutable once applied; add a forward migration instead of modifying deployed files. Do not assume a previous binary can run against a changed schema: test compatibility on a restored copy and use an explicit forward repair when incompatible.

## Incidents and administrative access

Use request IDs, route/status timing logs, job state, and the audit trail to investigate. Logs omit request query strings, bodies, cookies, tokens, and SQL arguments. Keep credentials and recovery links out of log collectors. Local processes emit JSON logs; the AWS deployment supplies encrypted CloudWatch logs, retention, dashboards and alert routing. Confirm the alarm subscription and test delivery to the responsible operator before activation.

Recent MFA is required for administrator actions and payout requests. If an account is compromised, suspend it in Admin → Operations; this revokes sessions and removes creator distribution. Review related purchases/payouts and preserve evidence before remediation. Restoring an account leaves a creator pending review and blocked posts unpublished. An audited post restore returns a removed post to draft; approval and publishing remain separate checks.

Users who lose their authenticator can sign in with their password and an unused recovery code, then enroll a replacement in Settings. Recovery revokes every existing session; administrative actions and payouts remain blocked until replacement enrollment succeeds. A code cannot be reused, and rotation invalidates the previous set. Neither operators nor account exports can retrieve printable codes. An account with neither its authenticator nor any saved recovery code cannot use self-service recovery. Support cannot bypass MFA or retrieve codes; preserve this assurance in published recovery policy. Do not overwrite MFA secrets to grant access.

If ledger imbalance is detected, stop monetization writes and preserve a backup. Never edit posted entries; investigate and post a reviewed reversing/correcting transaction. No real bank or processor action occurs in this sandbox. For live Stripe charges, follow the implemented [finance recovery procedures](finance.md): investigate authoritative provider records, retry corrected failures, use audited checkout/dispute actions and preserve unresolved discrepancies until verified. Provider-dashboard actions remain necessary for dispute evidence and later-invoice or partial refunds.

## Release verification

Run the commands in README, then test the optimized bundle with `PLAYWRIGHT_BASE_URL=http://127.0.0.1:3000 pnpm test:e2e`. Perform an isolated backup restore. Review `docs/production-readiness.md` before any public exposure. A passing local suite does not replace a clean CI run, provider acceptance, failover rehearsal, accessibility review, or independent security assessment.
