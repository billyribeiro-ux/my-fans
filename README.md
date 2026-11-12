# MyFans

A persistent creator subscription platform built with Svelte 5.57.2, SvelteKit 3.0.1, Node 24.21.0 LTS, pnpm 12.10.1, Rust 1.99.0, PostgreSQL 18.6, and FFmpeg 9.0.2. Versions were reviewed against upstream registries and release sources on October 7, 2026. The application supports isolated sandbox operation and a validated production configuration. The production deployment uses private AWS infrastructure, Stripe Connect, Sumsub, S3, ClamAV and SES. Supply your account, domain, provider credentials and operator policy through [the AWS setup guide](docs/aws-runtime.md); local verification is recorded in [verification](docs/verification.md).

## Start development

For native development on Apple silicon macOS, install the pinned Node/pnpm/Rust toolchains and Docker Desktop. FFmpeg 9.0.2 and ffprobe must be on PATH; the media sandbox permits their Homebrew dependencies under `/opt/homebrew/Cellar`. Build both Rust binaries so the isolated image decoder is available beside the API.

```sh
npm install --global pnpm@12.10.1
pnpm install --frozen-lockfile
pnpm db:start
pnpm db:seed
cargo build --locked -p myfans-api --bins
pnpm dev
```

Open http://127.0.0.1:5173. The command starts the Rust API on loopback port 8080 and the frontend on loopback port 5173. Ctrl-C stops both. PostgreSQL remains running so accounts and purchases survive app restarts.

On Linux, use the [container bundle](#container-bundle), which installs the decoder, FFmpeg and static sandbox launcher together. Native Linux media processing requires Landlock ABI 3 or newer, seccomp, and the exact `/usr/local/bin/{myfans-media,myfans-media-sandbox,ffmpeg,ffprobe}` paths; FFmpeg on PATH alone is insufficient. The [Linux CI setup](.github/workflows/ci.yml) builds, installs and verifies these dependencies. See [media isolation](docs/media-isolation.md) for the required enforcement checks.

Sandbox accounts use password `DevelopmentOnly!2026`: `demo@myfans.test`, `admin@myfans.test`, and `<creator-handle>@myfans.test`, such as `ellawild@myfans.test`. Seeding is explicit and does not overwrite existing accounts. Administrators must enroll MFA in Settings before opening Admin. Administrator actions and payouts require a session verified with MFA within the preceding 15 minutes.

## Container bundle

```sh
docker compose --profile bundle up -d --build --wait app
docker compose --profile bundle exec app myfans-api seed
```

Open http://127.0.0.1:3000. This is the optimized application bundle, running as an unprivileged user with a read-only root filesystem. It has its own database and media volumes, separate from the development instance. Node/Rust/PostgreSQL images are pinned by digest; FFmpeg source is pinned by checksum. The first build compiles FFmpeg and Rust. This is a local sandbox distribution, not a public production deployment.

## Implemented

- Accounts: durable sessions, verified email, encrypted MFA, single-use recovery codes, encrypted TLS mail outbox, SES bounce/complaint suppression, export, deactivation, reviewed erasure and enforced evidence retention.
- Creator Studio: profiles, private identity/payment progress, shared review messages and rejected-profile resubmission, resumable private uploads, isolated image/video processing, adaptive HLS playback, approval queues, captions, drafts, scheduled publishing, public/subscriber/purchase visibility, and ownership checks.
- Monetization: explicit sandbox checkout confirmation, server-owned prices, idempotent subscriptions/purchases/tips, 20% platform fee, subscriber payment history including individual renewals/refunds, period-end cancellation, balanced immutable sandbox ledger, settlement, reviewed payout reservations, and full refunds that revoke access.
- Provider adapters: hosted Stripe Connect/Checkout, signed webhooks, durable event/checkout recovery, serialized and resumable purchases, paid recurring entitlements, refund/dispute revocation, Sumsub document/selfie and age checks, private S3, and ClamAV with signature freshness checks. Production startup requires matching live provider configuration, operator policy, verified database TLS and restricted runtime credentials.
- Discovery: searchable creator directory, indexed caption/creator search, category/creator filters, Saved/Following feeds, stable cursor pages, and canonical post links with normal publication and entitlement checks.
- Community: persisted likes, bookmarks, comments, participant-authorized messaging and notifications with visibility-aware incremental refresh, reports, user blocking, and reviewed moderation appeals.
- Administration: creator/media/report/payout reviews, audited private post inspection with recent MFA, refunds, account suspension/restoration, blocked-post restoration, failed-job retry, independently reconciled provider finance, dispute/payout review, indeterminate-checkout resolution, privacy operations, operational counters, and immutable moderation evidence.
- AWS infrastructure: private CloudFront origin and ALB, WAF, isolated ECS Managed Instances capacity pools, Multi-AZ RDS with PITR, encrypted versioned media/recovery buckets, scoped roles, managed secrets, SES feedback, dashboards and alarms, signed recovery checkpoints, key rotation and privacy maintenance. See [account setup](docs/aws-runtime.md).
- Delivery: responsive desktop/mobile UI, Lucide SVG icons, SvelteKit server boundary, origin checks, bounded bodies, CSP/security headers, request IDs, database timeouts, pinned dependencies, CI, container packaging, and recovery/load checks.

By default, verification and recovery messages appear in the authenticated Settings sandbox inbox. That inbox does **not** prove control of an email address and sends no email. Configuring SMTP hides local inbox links and sends newly queued messages through a TLS-protected authenticated relay. Existing sandbox messages are never sent externally. Resending verification is limited to once per account per minute and invalidates earlier verification links. Sandbox creator approval is a test review, not KYC or age verification. Seed imagery is public Unsplash stock photography; actual uploaded media uses private storage and access checks.

MFA enrollment displays ten recovery codes once. Save them privately before dismissing the dialog; the server stores only digests. Existing MFA accounts can generate or rotate a set in Settings using their password and a current authenticator code. Rotation invalidates previous codes and revokes other sessions. A password and unused recovery code can replace the authenticator code at sign-in or password reset. Recovery revokes every existing session and requires enrollment of a replacement authenticator before administrative actions or payouts. A password reset does not sign you in: keep another unused code for sign-in and replace the authenticator in Settings immediately afterward.

## Media and finance behavior

Images accept JPEG, PNG and WebP up to 10 MiB. Resumable MP4/WebM uploads accept up to 256 MiB and five minutes, with 4 MiB chunks, authoritative offsets, matching retry checks and 24-hour expiry. Production scans every upload before an isolated decoder processes it; local sandbox scanning is disabled unless configured. Linux processing requires tested Landlock/seccomp enforcement, bounded memory/CPU/files and no network or inherited secrets. Images are re-encoded without source metadata; videos produce H.264/AAC MP4 plus 720p/360p HLS renditions and blurred previews. Approved videos need plain English WebVTT captions. Every media, caption, playlist, segment and range request rechecks authorization and uses private no-store responses through the CDN.

Sandbox subscriptions grant 30 days of access and do not auto-renew. Canceling preserves access until the period ends. Settlement moves creator funds from pending to available after one minute; this delay is only a sandbox fixture. Payout approval records a sandbox financial event and moves no bank funds. Refunds create reversing ledger entries; refunds after payout may produce a negative creator balance, preventing further payouts.

Account deactivation disables access, revokes all sessions, stops renewal flags, and hides creator posts. It is not data erasure. Nonzero creator balances must be resolved first; administrator offboarding is operator-managed. A separate authenticated erasure request freezes access immediately, waits for review and unresolved obligations, removes live objects and personal data, and tracks historical versions/provider/backup completion separately. The operator supplies all retention periods; owner maintenance enforces captured evidence deadlines while preserving money journals. See [privacy operations](docs/privacy-operations.md).

Blocking hides the other person's creator content from discovery and prevents new messages, likes, saves, and comments between the accounts. Both participants can still read existing message history. Blocking does not reverse purchases, cancel subscriptions, or revoke existing paid media entitlements; those require the financial workflows. You can manage your own blocks in Settings. A reciprocal block remains in effect until its owner removes it.

Creators can appeal removed posts in Settings. Only the post owner can file, only one appeal may be pending per post, and recent MFA is required to review. Accepted appeals return blocked posts to draft; normal creator/media/publishing checks still apply. Reviewed appeal reasons and decisions cannot be rewritten or deleted. Operational policy and appeal handling ownership remain production requirements.

Collection views use 50-row timestamp/UUID cursor pages. Separate Studio, earnings, and admin collections retain independent cursors; comments and message history can load older pages. Financial history identifies each posting separately from its parent transaction.

## Configuration

Frontend private variables are declared in `apps/web/src/env.ts`. `API_BASE_URL` defaults to `http://127.0.0.1:8080`. `APP_ORIGIN` is a build-time origin and must match the browser address; it defaults to `http://127.0.0.1:5173` for local builds. The container builds for `http://127.0.0.1:3000`. The packaged Node server sets `BODY_SIZE_LIMIT=12M`; route-specific JSON limits remain 16 KB.

Rust accepts `DATABASE_URL` or `DATABASE_URL_FILE` (mutually exclusive), `API_BIND`, `MEDIA_ROOT`, `RUST_LOG`, `APP_EMBEDDED_WORKER`, and `APP_RUN_MIGRATIONS`. The secret file must contain one PostgreSQL connection URL, at most 4096 bytes. Defaults target local PostgreSQL on port 54328 and `.local/media`. `APP_MODE` accepts `sandbox` or `production`; production validates live providers, published operator policy, matching HTTPS origins, SES feedback, restricted database access and private storage. Production uses `OPERATOR_POLICY_FILE`; [the general policy draft](deploy/operator-policy.example.json) supplies complete text and proposed retention periods, with deliberately invalid operator identity/contact placeholders that must be replaced. See the [policy guide](docs/operator-policy.md) before adopting it. The API rejects non-loopback bindings. Local sandbox MFA encryption uses a generated private `.mfa-key` inside the media directory. S3 deployments require `MFA_ENCRYPTION_KEY_FILE` containing exactly 32 raw bytes shared by all app/worker processes. Back up the matching key with the database and media. The provider environment template is [deploy/providers.env.example](deploy/providers.env.example); `myfans-api doctor` reports missing provider configuration without printing secrets.

SMTP uses pinned Lettre 0.11.23 with Rustls. Set `MAIL_TRANSPORT=smtp`, `SMTP_HOST`, `SMTP_USERNAME`, `SMTP_PASSWORD_FILE`, `MAIL_FROM`, and `MAIL_PUBLIC_ORIGIN` in managed runtime configuration. The password file must be readable by the app user and contain at most 4096 bytes. `SMTP_SECURITY` accepts `starttls` (default port 587) or `tls` (default port 465); `SMTP_PORT` may override the port. Plaintext transport and disabled certificate validation are unavailable. `MAIL_PUBLIC_ORIGIN` must be an HTTPS origin matching the frontend; loopback HTTP is allowed for local testing. SMTP settings without an explicit transport selection are rejected at startup. See [operating procedures](docs/operations.md) for delivery and acceptance details.

Startup applies embedded migrations by default. For a separately managed runtime database role, apply migrations using the migration owner (`myfans-api migrate`), apply `db/roles/runtime.sql`, grant that role to a separately managed login, and start with `APP_RUN_MIGRATIONS=false`. The runtime role cannot own schema objects, truncate tables, delete ledger postings, or modify audit rows.

TypeScript 6.0.3 is a compatibility exception: the current framework tooling declares TypeScript 6 peers. Bits UI is deferred because its dependency tree declares SvelteKit 2 compatibility. Native accessible dialogs are used instead.

See the [October 7 version decisions](docs/versions.md) for supported-major exceptions and release evidence. The [container finding record](docs/container-findings.md) preserves historical findings and the passing Alpine candidate inventory.

## Verify

```sh
pnpm check
pnpm test
pnpm test:api
pnpm test:integration
pnpm test:db
pnpm build
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
pnpm format:check
pnpm audit
cargo audit
pnpm --filter @myfans/web exec playwright install chromium
pnpm test:e2e
node scripts/load-smoke.mjs
```

Integration tests use `sqlx::test` to create and remove separate migrated databases; they require a PostgreSQL account with disposable-database creation rights and the platform-specific media helper setup above. `TEST_DATABASE_URL` overrides the local development server for these tests. Ordinary Rust unit tests leave integration tests ignored; `pnpm test:integration` runs them explicitly. Database invariant tests create an isolated local cluster, or use an **empty disposable** `TEST_DATABASE_URL`.

Browser tests use unique accounts and cover desktop/mobile workflows. To test the container bundle, run `PLAYWRIGHT_BASE_URL=http://127.0.0.1:3000 pnpm test:e2e` after starting and seeding it. The bounded load smoke test targets loopback only and is not a production capacity benchmark.

## Deployment and recovery

The separate [private staging deployment](docs/deployment.md) uses immutable images, generated file-mounted credentials, separate migration/runtime logins, and health checks for both services. It includes backup/restore rehearsals and an offline private package command. The CI container job exercises this deployment with the full browser suite. This is ready to verify in a private Docker environment; it does not enable public production.

See [verification evidence](docs/verification.md), [security design](docs/security.md), [third-party notices](THIRD_PARTY_NOTICES.md), [operating procedures](docs/operations.md), [architecture](docs/architecture.md), [delivery status](docs/backlog.md), and [production launch gates](docs/production-readiness.md). Recovery scripts create private local backups and verify restoration in a disposable database without altering the live database.

The remaining account-dependent work is supplying hosting/domain/account references, approved provider credentials, real operator identity and contacts, adopting the supplied policy draft, then executing the documented account acceptance and restore drills. The [operator, creator and subscriber workflow review](docs/product-completion.md) records the delivered journeys. Local tests cannot establish merchant approval, mailbox delivery, target-account permissions or actual cloud failover. The [release workflow](docs/release.md) builds origin-specific ARM64 artifacts, scans both images, collects corresponding third-party source and issues verifiable provenance after the repository and release environment are configured.
