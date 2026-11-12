# Current image release gate

The final private application image is `sha256:d718f8cc327e797abcbef9e1281cfe9b5b36e4e681c77609347e201f2180294c`, with configuration digest `sha256:6ce980c151cf14e652187f3d70fb09d1fc674e2b0c6462caa7da1a30f692e544`. It includes the completed finance, privacy/recovery, resumable media and frontend implementation. It uses the pinned Node/Alpine/Rust/FFmpeg runtime choices described below. Its exact inventory, findings, scanner identities and hashes are written to `.local/releases/quality-final-20261008/`; a successful `manifest.json` is emitted only after the image gate passes. Source dependency audits and packaged application/recovery checks are separate requirements, recorded in [verification](verification.md). The artifact targets the private loopback origin; production requires a build for the actual domain.

# Alpine ClamAV scanner release check: October 8, 2026

The current restricted scanner image is `sha256:f1294461d7b258bac4d9f9b18ac6fb7d6609281cafafa5aa99d3fb048240dcb8`, with configuration digest `sha256:b9c54911bdca53b5f795f1579ee8b6e7d1bed9869383742a905e74aac84426ac`. The exported Docker archive's actual configuration bytes were independently hashed to confirm that identity. This source-built Alpine 3.24 / ClamAV 1.5.4 image replaces the earlier small-fixture candidate below.

The exact image passed health, the 48-hour signature freshness check, clean acceptance, and EICAR rejection. Its daemon reported signature database `28147`, dated October 8 at 06:24:12 UTC. A full **256 MiB** benign INSTREAM was accepted in **16,742 ms**, including transmission with backpressure. This ran with two CPUs, 5 GiB memory, UID/GID 1000, read-only root, all capabilities dropped, no-new-privileges, no published port, a 128-process limit, and a private 1.5 GiB `/scan` tmpfs. ClamAV's scan deadline is 25 seconds; the application and proof retain their absolute 30-second stream deadline.

The boundary check found two concrete configuration issues before this image passed: the old scanner byte limits were below the application's video limit, and the initial 15-second scan budget rejected the maximum fixture with `Heuristics.Limits.Exceeded.MaxScanTime`. The final scanner permits 256 MiB files/streams and a 512 MiB scan expansion limit. These historical failures were not treated as successful scanning.

Offline Docker Scout 1.25.0 indexed 43 packages and emitted 52 SPDX package entries for this exact archive. Grype 0.120.1, with explicit Alpine 3.24 and its valid advisory database built October 8 at 06:33:47 UTC, reported **zero matches at every severity**, including zero High/Critical matches. No project suppression or applicability exception was used. Exact archive/report hashes, runtime controls, health, advisory metadata, and the bounded protocol result are retained in `.local/releases/scanner-time-budget-final-20261008/scanner-verification.json` with the archive, SPDX, vulnerability report, and acceptance log. Local checks do not establish signed provenance, AWS acceptance, or production authorization; compiled bundled dependencies still require the separate source audits.

# Earlier Alpine scanner small-fixture acceptance: October 7, 2026

The earlier small-fixture candidate is `sha256:39875a1d5bab5eb04de2156fb33fe50e9a691b982d266171e8b0303d46b14ef3`, Linux ARM64 on Alpine 3.24, with ClamAV 1.5.4 built from the checksummed upstream source and the captured patched Cargo lock. Its runtime configuration digest is `sha256:bbff8f42bfd9125453c2dc5260b9d6b6d94e373a313e99f3e8b68a57f04db21f`. The Docker containerd image store reports the image/index identity separately from its configuration; these values are not interchangeable.

The image ran as UID/GID 1000 with a read-only root, all capabilities dropped, no-new-privileges, no published listener, two CPUs, a 5 GiB memory limit, a 128-process limit, and a private 1.5 GiB writable `/scan` tmpfs. Health passed. The actual TCP scanner protocol reported `ClamAV 1.5.4/28146/Wed Oct 7 06:24:18 2026`; [the acceptance script](../scripts/verify-scanner.mjs) enforced the 48-hour freshness bound, accepted a clean fixture, and rejected the harmless EICAR antivirus fixture from the same network namespace.

The initial Alpine image exposed a startup race: freshclam updated the database before clamd created its notification socket, leaving the running daemon with stale preloaded signatures. That candidate's freshness test failed and was not accepted. The rebuilt startup sequence completes an initial signature update before starting clamd, then runs the updater daemon. The final proof above passed without relaxing freshness or changing signatures manually.

Docker Scout 1.25.0 inventoried the exact exported image with networking disabled and a disk-backed scratch directory. Scout reported 43 indexed packages; the emitted SPDX document contains 52 package entries. Grype 0.120.1 used an explicit Alpine 3.24 distribution and a valid schema v6.1.10 advisory database built October 7 at 06:31:48 UTC. It reported **zero matches at every severity**, including zero High/Critical matches; no suppression or applicability exception was used.

Exact archive and report hashes, runtime controls, health, database metadata, and protocol proof are retained privately in `.local/releases/scanner-alpine-final-20261007/scanner-verification.json`, alongside `image.tar`, `runtime.spdx.json`, `runtime-vulnerabilities.json`, and `acceptance.log`. The earlier failed-startup image and its separate inventory remain in `.local/releases/scanner-alpine-20261007/`. Inventory cannot completely detect compiled Rust/ClamAV dependencies; the scanner Cargo audit remains a separate CI gate. These are local exact-image checks, not signed provenance, external AWS acceptance, distribution-license certification, or production authorization.

# Earlier provider container result

The October 7 provider candidate `sha256:be53d7175e4e2ee1b7a494a0a722524377db3bbc7e8085ec273746d931d6a6c6` uses Node 24.21.0 on Alpine 3.24, with Rust and FFmpeg built for the same musl runtime. Docker Scout inventoried 28 runtime packages; Grype 0.120.1 reported **zero matches at every severity**, including zero High/Critical findings. The package gate passed without suppressions or vulnerability exceptions. Rust and JavaScript lockfile audits also passed.

The first passing final-image inventory is under `.local/releases/providers-20261007/`. This is time-dependent advisory evidence for that exact image, not proof that all vulnerabilities are known or that bundled Rust/JavaScript dependencies are completely detectable. The release manifest states the inventory/provenance limitations. Actual private browser/restore evidence is recorded separately in [verification](verification.md); public production remains disabled.

The historical Debian 13 findings below were preserved. They have not been declared false positives or accepted risks. The new runtime removes those installed packages/versions by changing the actual distribution and rebuilding native components; it does not suppress their advisories.

# Historical Debian 13 release gate: October 7, 2026

**Packaging was blocked for this historical candidate.** Tested candidate: `sha256:c7ad47f1640da773796da391fdd1b16466427958e209fb63d3caa73e3ed2a607`, Linux arm64, Node 24.21.0 on Debian 13. The scan identified 143 package/advisory matches, including 49 High and zero Critical matches across nine distinct High CVEs. Source-package matches repeat across installed binary packages; 49 is not a count of independently demonstrated exploits.

Evidence is retained privately in `.local/releases/staging-20261007-oct7/runtime.spdx.json` and `runtime-vulnerabilities.json`. Docker Scout 1.25.0 inventoried 118 packages offline. Grype 0.120.1 used a valid schema v6.1.10 database built October 7 at 06:31:48 UTC and an explicit Debian 13 distribution. The helper returned nonzero and created no `manifest.json`. This directory is a failed candidate, not an approved transfer package. `pnpm audit` and `cargo audit` passed separately; they do not cover these operating-system findings.

| Installed source package    | CVE and affected operation                                                                                                    | Grype fix state |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | --------------- |
| ncurses 6.5+20250216-2      | [CVE-2025-69720](https://security-tracker.debian.org/tracker/CVE-2025-69720): infocmp string analysis                         | wont-fix        |
| glibc 2.41-12+deb13u4       | [CVE-2026-5435](https://security-tracker.debian.org/tracker/CVE-2026-5435): deprecated DNS record printing                    | wont-fix        |
| glibc 2.41-12+deb13u4       | [CVE-2026-19499](https://security-tracker.debian.org/tracker/CVE-2026-19499): monetary string formatting and padding          | wont-fix        |
| gcc-14 14.2.0-19            | [CVE-2026-95619](https://security-tracker.debian.org/tracker/CVE-2026-95619): libstdc++ aligned allocation overflow           | not-fixed       |
| gcc-14 14.2.0-19            | [CVE-2026-102010](https://security-tracker.debian.org/tracker/CVE-2026-102010): libstdc++ priority-queue erase operation      | wont-fix        |
| util-linux 2.41.5-0+deb13u1 | [CVE-2026-76642](https://security-tracker.debian.org/tracker/CVE-2026-76642): mount helper failure and privileged hooks       | wont-fix        |
| util-linux 2.41.5-0+deb13u1 | [CVE-2026-78408](https://security-tracker.debian.org/tracker/CVE-2026-78408): namespace entry and inherited cgroup descriptor | wont-fix        |
| util-linux 2.41.5-0+deb13u1 | [CVE-2026-78409](https://security-tracker.debian.org/tracker/CVE-2026-78409): mount subdirectory path resolution              | wont-fix        |
| util-linux 2.41.5-0+deb13u1 | [CVE-2026-78410](https://security-tracker.debian.org/tracker/CVE-2026-78410): authorized bind-mount source replacement        | wont-fix        |

Debian's linked trackers still list these installed stable versions as vulnerable. Several carry a `no-dsa` minor-issue classification. That classification is not proof that this application's image is unaffected. Scanner fix states are reproduced as observed, not treated as authorization to suppress findings.

The application runs as `node` with a read-only root, dropped capabilities, no-new-privileges, bounded resources, and a private loopback listener. Those controls reduce exposure but are not a demonstrated reachability analysis. No VEX assertion, waiver, or risk acceptance has been added. The packaging gate fails on any High/Critical match.

Resolution requires a tested patched/minimized runtime or a documented, independently reviewed applicability determination with an explicitly approved release policy. Rebuild, inventory, and rescan the exact candidate; repeat packaged browser, media codec, health, migration, and restore checks after runtime changes. Do not mix Debian unstable libraries into a stable image or remove package metadata simply to hide matches. Production also requires the separate [launch gates](production-readiness.md).
