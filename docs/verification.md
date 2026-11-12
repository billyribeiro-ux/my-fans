# Release verification

The latest frontend changes and their official-documentation basis are recorded in the [Svelte idiomatic audit](svelte-idiomatic-audit.md). The following is the retained baseline release verification; its image and counts apply to that immutable package.

## Baseline release

Source versions were selected at the October 7, 2026 cutoff. Adversarial review and final verification continued on October 8, 2026, America/New_York. The replacement Linux ARM64 application image is `sha256:d718f8cc327e797abcbef9e1281cfe9b5b36e4e681c77609347e201f2180294c`; its OCI configuration digest is `sha256:6ce980c151cf14e652187f3d70fb09d1fc674e2b0c6462caa7da1a30f692e544`. Docker's containerd image store exposes the manifest-list digest as its local image ID; it differs from the configuration digest. This image is built for `http://127.0.0.1:3100`. Production requires a build for the actual domain.

## Source and browser verification

| Check          | Observed result                                                                                                                               |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Frontend       | 57 unit tests pass; Svelte/TypeScript zero errors/warnings; all 37 Svelte components pass the official analyzer without issues or suggestions |
| Rust           | 25 unit tests and 76 integration tests pass: 51 isolated SQLx library tests, 21 platform tests and four post-view tests                       |
| Database       | All 29 forward migrations and existing financial/audit invariants pass; this review introduces no migration or runtime grant changes          |
| Static quality | Strict workspace/all-target Clippy, Rustfmt and repository Prettier checks pass                                                               |
| Dependencies   | Exact unchanged pnpm and application Cargo lockfiles retain their passing vulnerability audits; the scanner lock is separately audited        |
| Cloud          | Eight CloudFormation stacks pass schema/Guard validation; 32 insecure mutations rejected; 19 bootstrap/operation tests pass                   |
| Browser        | All 44 Playwright browser/accessibility checks pass against the exact replacement image in 46.5 seconds                                       |

Backend logs are `.local/release-evidence/quality-final-rust-unit.log`, `quality-final-integration.log`, `quality-lifecycle-clippy.log` and `quality-final-rust-format.log`. Frontend/browser/official Svelte evidence is under `.local/release-evidence/quality-final-20261008/`. The handoff verifier checks unchanged schema, policy and lockfile hashes before reusing prior invariant, policy and dependency-audit evidence; those observations are explicitly identified as reused rather than represented as new runs.

Coverage includes local Stripe/Sumsub/S3/SMTP protocol fixtures, signatures and replay, account isolation, balanced immutable money, independent reconciliation, uncertain-checkout recovery, refunds/disputes/payouts, MFA recovery, policy consent, holds and erasure, signed privacy-journal replay, resumable upload concurrency/hash/expiry, HLS/range/caption authorization and media scratch cancellation. Provider fixtures neither create external payments nor send external mail.

## Defects found and closed

The review reproduced two account lifecycle failures before changing the implementation: a request could restore erased creator profile text or create a checkout after account freeze committed. The deterministic failing evidence remains in `.local/release-evidence/quality-lifecycle-before.log`. Sixteen focused lifecycle regressions now pass and are included in the complete backend suite.

Guarded mutations acquire account locks and then revalidate the same session, current role and required recent MFA. Additional regressions hold requests inside actual local Stripe, identity and S3 protocol fixtures while freezing or recovering the account. Successful external identifiers and cancellations remain recorded for reconciliation, while stale callers receive no new hosted capability. Legacy upload completion serializes with erasure; the test verifies that the erasure worker removes the completed current object. Explicit account ordering also closes refund/receipt/settlement lock inversions caused by account foreign keys. These guarantees do not imply atomic deletion across external providers or historical object versions.

Messages now retain drafts per recipient and clear them on identity change or dialog disposal. Canonical `/?post=UUID` links resolve older posts while retaining publication, blocking, creator eligibility and entitlement rules. Directory controls open the paginated creator directory. Moderators use a separate audited recent-MFA inspection endpoint for restricted or removed evidence; inspection changes no payments or entitlements, and every private media request independently checks access. Four backend post-view tests cover older/draft/removed/disabled/blocked records, guest/member/admin boundaries, expired/recovered/revoked assurance and private media.

Policy reload procedures replace and verify both web and worker deployments, check the observed secret version before and after the rollout, and compare the actual public policy with the approved document. Signed recovery checkpoints retain exact historical task revisions and images so a newer release does not prevent isolated historical restoration. Missing retained history is rejected before restore resources are created. Checkpoint serialization enforces the same 32 MiB envelope bound used by its loader. Cloud tests cover rollback, version races, public-policy mismatches, historical definitions and oversized manifests.

## Exact-image runtime and recovery

The replacement image passed both Linux media isolation profiles, under UID 65532 and application UID 1000. Tests cover protected-file read/truncate, network and inherited-environment denial, real image processing, invalid executables/workspaces and argument bounds. The diagnostic runs the production video renderer with audio and silent fixtures, validating both HLS tiers, MP4 fallback, preview and scratch cleanup. Evidence is `.local/release-evidence/quality-media-isolation.log`; see [media isolation](media-isolation.md).

The isolated upgrade restored the consistent checkpoint `.local/deployments/myfans-provider-qa/backups/2026-10-08T15-17-18.008Z-be9d6fc9-7fdb-4320-8cd4-511e7218987c`, validated the prior release, and verified the candidate using the restricted runtime login. All 29 migrations, ledger/media references and authenticated decryption of 33 MFA records and zero encrypted mail bodies passed. Live data was not changed by the rehearsal. Evidence is `.local/release-evidence/quality-upgrade.log`.

The bounded local workload completed 4,015 requests at concurrency 16 with zero failures in 4.143 seconds. Route p95 values were 23–36 milliseconds. Evidence is `.local/release-evidence/quality-load.json`. This is a local fixture workload, not production capacity in an unconfigured hosting account.

The final desktop/mobile role replay passed all 16 checkpoints for registration/email/MFA, creator rejection/resubmission/approval, media feedback, private video processing/captioning/publication/playback, and pending/null/terminal subscription states with persisted cancellation. The additional UI walkthrough passed 12 checkpoints covering recipient-specific drafts, a post older than the newest 50, unavailable/directory routing, recent-MFA private captioned-video inspection, expired-assurance retry, dialog accessibility and zero payment/entitlement writes. Fourteen screenshots were reviewed. Evidence is under `.local/release-evidence/quality-role-qa-20261008/` and `quality-ui-final-20261008/`; scoped fixtures were cleaned up after both runs.

After those checks, the final consistent checkpoint `.local/deployments/myfans-provider-qa/backups/2026-10-08T15-37-49.777Z-34e86444-a432-4a72-8344-e05122deb7a7` passed isolated restoration with the exact replacement image. Archive/media checksums, ledger and media references, all 29 migration checksums, and authenticated decryption of 41 MFA records and zero encrypted mail bodies passed. Evidence is `.local/release-evidence/quality-backup-restore.log`. The app and database restarted healthy. The non-secret diagnostic reports healthy local storage/media isolation and zero failed jobs; it intentionally returns nonzero because real provider configuration and acceptance are absent.

The private transfer package is `.local/releases/quality-final-20261008/`. Its packaging command emits `manifest.json` only after the exact exported image passes the High/Critical advisory gate. The manifest binds image/source archives, SPDX inventory, advisory reports and an allowlisted source inventory. Credentials, databases, uploads, backups and browser traces are excluded. `.local/releases/quality-handoff-20261008.json` independently binds the reviewed evidence and verifies source snapshots, archives and OCI blobs against their hashes. Those local hashes establish integrity; hosted source-to-image provenance remains a separate configured CI result. The earlier role-completion package and handoff remain unchanged historical artifacts.

## Operator policy and scanner

The [general policy draft](operator-policy.md) supplies 36 sections covering terms, privacy, content and refunds, plus six proposed retention periods. Its 27,477-byte JSON fits the managed-secret limit. The actual Rust configuration loader accepted its schema with reserved test contacts substituted and rejected the original placeholder contacts. The probe stopped at the next origin gate before accessing a database or provider. `.local/release-evidence/operator-policy-validation.json` binds the unchanged draft and results. Schema validity is not legal approval; actual identity, address and contacts remain operator inputs.

The unchanged scanner image is `sha256:f1294461d7b258bac4d9f9b18ac6fb7d6609281cafafa5aa99d3fb048240dcb8`, configuration `sha256:b9c54911bdca53b5f795f1579ee8b6e7d1bed9869383742a905e74aac84426ac`. Its retained October 8 proof covers ClamAV 1.5.4, clean-file acceptance, harmless EICAR rejection and a full 256 MiB benign stream accepted in 16,742 ms within its deadline. Signature version 28147 was dated October 8 at 06:24:12 UTC. Offline inventory and its fresh Grype database reported zero advisory matches. Exact controls, hashes and scope remain in `.local/releases/scanner-time-budget-final-20261008/scanner-verification.json` and [container findings](container-findings.md). Compiled dependency discovery is incomplete, so source audits remain a separate gate.

## Deployment boundary

The application, operator/creator/subscriber workflows and account setup procedures are implemented. AWS deployment, actual provider onboarding/payment/identity/mailbox acceptance, hosted CI/provenance, target-account restore/failover and traffic capacity require the supplied accounts. These outcomes are not established by local tests. Operational ownership, real operator facts and adoption of the supplied policy/retention choices also remain explicit inputs.

See [role completion](product-completion.md), [AWS deployment](aws-runtime.md), [production activation](production-readiness.md), [privacy recovery](privacy-recovery.md) and [historical verification](verification-history.md). Earlier packages and handoffs remain immutable historical artifacts.
