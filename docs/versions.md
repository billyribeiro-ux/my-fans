# Version decisions: October 7, 2026

Cutoff: October 7, 2026. Choices favor released, supported, compatible versions with reproducible pins. A registry's newest major version alone does not establish that it is the best supported combination. This record does not assert production readiness; see [launch gates](production-readiness.md) and [verification](verification.md).

## Application toolchains

| Component                         | Selected version | Decision                                                                                                        |
| --------------------------------- | ---------------- | --------------------------------------------------------------------------------------------------------------- |
| Node.js                           | 24.21.0 LTS      | Latest LTS; 26.11.0 is Current, outside the requested LTS policy                                                |
| pnpm                              | 12.10.1          | Current registry release; package manager and lockfile updated together                                         |
| Rust                              | 1.99.0           | Stable release of October 1; edition 2024, pinned compiler and lockfile                                         |
| PostgreSQL                        | 18.6             | Latest supported stable major/minor; 19 remains beta                                                            |
| FFmpeg                            | 9.0.2            | Current stable release; source checksum pinned, required H.264/AAC codec smoke passes                           |
| Svelte / SvelteKit                | 5.57.2 / 3.0.1   | Current registry releases, rebuilt together                                                                     |
| Node adapter / Svelte Vite plugin | 6.0.0 / 7.3.1    | Current compatible registry releases                                                                            |
| Vite / Vitest                     | 8.3.3 / 5.0.3    | Current compatible registry releases                                                                            |
| TypeScript                        | 6.0.3            | Latest 6.x; Kit 3.0.1 declares `^6.0.0`, and svelte-check 4.7.6 supports 5/6; 7.0.2 is excluded until supported |
| Node type declarations            | 24.19.1          | Current 24.x declarations matching the actual Node runtime; latest 26.x targets a different runtime             |
| Tailwind / Vite integration       | 4.3.3 / 4.3.3    | Current registry releases                                                                                       |
| Lucide Svelte SVG icons           | 1.52.0           | Current registry release; consistent accessible code-native icons                                               |
| Svelte check                      | 4.7.6            | Current registry release                                                                                        |
| Playwright / axe integration      | 1.64.0 / 4.13.0  | Current registry releases; Chromium revision 1248 used for the updated suite                                    |
| Prettier / Svelte plugin          | 3.9.9 / 4.1.1    | Current registry releases                                                                                       |

Node's [release policy and current versions](https://nodejs.org/en/about/previous-releases), the [Rust stable announcement](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/), [PostgreSQL versioning](https://www.postgresql.org/support/versioning/), and [FFmpeg downloads](https://ffmpeg.org/download.html) were checked. JavaScript versions and peer ranges were checked through the publishers' npm registry metadata (`pnpm view PACKAGE version`, `pnpm view PACKAGE@VERSION peerDependencies --json`, and supported-major queries). Registry `latest` results can move after this cutoff; exact selected versions are preserved in the manifests and lockfile.

The original direct Rust dependency pins in `Cargo.toml` were checked with `cargo search` against crates.io on October 7 and retained. Exact versions, feature choices, and the resolved transitive graph are recorded in `Cargo.toml` and `Cargo.lock`. Strict peer dependency and Node engine checks remain enabled. pnpm's reviewed release-age exceptions are exact versions rather than blanket package exemptions.

## Runtime and verification choices

The new runtime candidate uses the official Node 24.21.0 Alpine 3.24 image pinned by digest, with Rust 1.99.0 and FFmpeg built against the same musl distribution. Alpine 3.24.2 was released September 17; current stable package security updates are applied at build time. Unused npm/npx/Yarn tooling is removed. Build-time package updates are not a hermetic package snapshot: retain the exact tested image and inventory. Historical Debian findings and the new candidate's results remain separately recorded.

Private S3 uses `aws-sdk-s3 = 1.152.0` and `aws-config = 1.12.0`; SES feedback uses `aws-sdk-sqs = 1.114.0`. Adaptive browser playback uses pinned `hls.js = 1.7.3` with a same-origin restricted loader. These dependencies use their locked compatible releases. AWS clients run, with default HTTPS transport and the runtime credential chain. The older optional S3 `rustls` feature is intentionally disabled because it selects the legacy transport. Provider HTTP uses Reqwest 0.13.5 with Rustls, bounded deadlines/bodies, no redirect following, and no proxy environment inheritance. HMAC 0.13.0 authenticates provider signatures. These exact versions were checked against crates.io and are locked. Stripe's verified snapshot API contract is `2026-09-30.endive`.

Docker Scout 1.25.0 inventories an exported image offline; Grype 0.120.1 scans that inventory using its fresh public vulnerability database. Both scanner images are pinned by digest. Dependency audits complement container scanning because bundled JavaScript and stripped Rust package detection is incomplete. The Alpine candidate passes its inventory scan with 28 detected packages and zero advisory matches. The historical Debian 13 candidate failed; its findings remain in the [container record](container-findings.md). No vulnerability acceptance or public production approval follows from choosing a current base image. Alpine release evidence: [September 17 maintenance release](https://alpinelinux.org/posts/Alpine-3.21.8-3.22.6-3.23.6-3.24.2-released.html).

The product decision is general creator content with explicit adult content prohibited. Stripe Connect and Sumsub have sandbox and production configurations; private S3, isolated ClamAV/media processors and SES feedback are integrated. Production activation requires real operator/provider/account information and acceptance. Operator jurisdiction, merchant acceptance, provider accounts and production infrastructure acceptance are not inferred. See [provider setup](provider-setup.md).

Infrastructure validation uses cfn-lint 1.57.2 (published October 6) with hash-locked Python dependencies and CloudFormation Guard 3.2.0. The locally installed validator environment is separate from the application runtime. [Validation instructions and scope](aws-media.md) record the schema regions and security rules; no AWS account deployment has been validated.
