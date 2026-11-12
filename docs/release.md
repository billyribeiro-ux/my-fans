# Private release artifacts

The manually dispatched [release workflow](../.github/workflows/release.yml) builds private Linux ARM64 application and scanner artifacts for an exact HTTPS application origin. It requires a successful **Verify** push run for the same repository and commit. It creates no GitHub Release and activates no production deployment.

This workflow has been checked locally with actionlint, ShellCheck, YAML parsing, and the documented action interfaces. It has not been executed remotely: this workspace does not supply an authorized GitHub repository, Enterprise Cloud attestation capability, protected publication environment, AWS role, or production domain. Local hashes are not issued Sigstore attestations.

The source collector also passed a real Linux smoke test using the pinned Alpine snapshot's json-c 0.18-r1 recipe and patch, a host-matching unprivileged UID absent from the container's passwd database, dropped capabilities, and no-new-privileges. `abuild listpkg` emitted the expected package filenames, and `abuild fetch verify` validated both the upstream archive and patch checksums. This checks the collector interface; collecting every actual runtime package remains a required remote release step.

## Configure the repository

Use a private repository on GitHub Enterprise Cloud. GitHub artifact attestations for private repositories require Enterprise Cloud and are unsupported on GitHub Enterprise Server. Attestation failure blocks the signed artifact and publication jobs; there is no unsigned-success fallback. [GitHub attestation documentation](https://github.com/actions/attest#usage) explains the OIDC and attestation permissions.

Protect the default branch and require the Verify workflow. Permit only reviewed changes to workflows, build recipes, dependency locks, and source checksum pins. Create a protected environment named **release** before enabling ECR publication. Restrict its branches and reviewers. The build job has no OIDC permission, no stored checkout credentials, and no cloud secrets; attestations and publication have separate permissions.

## Dispatch a build

After the exact default-branch commit passes Verify, record that run ID and supply the real canonical HTTPS origin. Inputs are validated through environment variables, rather than interpolated into executable commands.

```sh
gh workflow run release.yml --repo OWNER/REPOSITORY \
  -f app_origin=https://YOUR_ACTUAL_DOMAIN \
  -f verification_run_id=EXACT_SUCCESSFUL_VERIFY_RUN_ID \
  -f expected_repository=OWNER/REPOSITORY \
  -f publish_ecr=false
```

Replace every capitalized value. The origin must have a lowercase DNS hostname, HTTPS, no credentials, no port, no path, and no trailing slash. It becomes the immutable SvelteKit origin; runtime settings cannot replace it. Build a new artifact for a different origin.

The optional aports_commit is an exact upstream Alpine source-recipe commit, initially pinned to the verified October 7, 2026 snapshot. Collection requires every runtime APK name and version to occur in those recipes. If an Alpine upgrade no longer matches, find and review the corresponding upstream commit, then dispatch with its full SHA. Missing or mismatched source blocks the release; do not regenerate upstream checksum declarations to bypass this gate.

## Inspect the artifacts

The workflow produces an unsigned intermediate artifact and, only after attestation and verification succeed, a **release-attested-COMMIT-ATTEMPT** artifact. Retention is 14 days. Retain the verified bundle with its corresponding source for the actual deployment and applicable distribution obligations before that retention expires.

| File                                                                      | Purpose                                                                                                                                                                                                                                          |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| application.oci.tar, scanner.oci.tar                                      | OCI archives, including BuildKit provenance. Their runtime configuration digests must match the locally scanned Docker images.                                                                                                                   |
| source.tar.gz                                                             | Explicit project source, migrations, pinned build recipes, dependency locks, deployment helpers, and notices. Credentials, media, backups, auth state, and browser traces are excluded.                                                          |
| corresponding-source.tar.gz                                               | Checksummed FFmpeg 9.0.2, Node 24.21.0, and ClamAV 1.5.4 archives; Alpine recipe snapshot, downloaded source and patches for both runtime APK inventories; application/scanner Cargo vendor sources; scanner lock updates and rebuild materials. |
| npm-sources.tar.gz, npm-licenses.json                                     | Installed pinned npm package contents and application dependency license declarations. The archive retains the package-store layout and dependency links.                                                                                        |
| application.spdx.json, scanner.spdx.json                                  | Runtime package inventories bound to the respective OCI archive with SBOM attestations.                                                                                                                                                          |
| *.vulnerabilities.json                                                    | Complete Grype findings. Any High or Critical finding blocks packaging; no suppression is applied.                                                                                                                                               |
| release.json, verified-ci.json, *.build.json, local-package-manifest.json | Commit, exact CI run, image configuration digests, origin, source-recipe commit, build metadata, and local source digests.                                                                                                                       |
| SHA256SUMS, *.sigstore.json                                               | Release subject digests and the actual GitHub-issued provenance/SBOM bundles.                                                                                                                                                                    |

The source step uses upstream recipe checksums through abuild fetch verify; it never runs abuild checksum to replace declarations. The Alpine recipe archive includes patches and compilation recipes. Restore vendor directories/configuration beside the extracted project or scanner source and adjust the directory paths before an offline rebuild. Scanner rebuilding must use the captured scanner.Cargo.lock and CMake change in scanner.Dockerfile; the original upstream lock is not the patched release graph. The FFmpeg configure command is captured in install-ffmpeg.sh and the Dockerfile.

SPDX inventories are not complete detection of compiled or bundled dependencies. Separate pnpm, application Cargo, and scanner Cargo audits remain mandatory. License declarations and collected source are engineering evidence; they do not select distribution terms for the project's original code or certify compliance for an actual distribution. Review original-code terms, applicable notices, GPL correspondence, codec terms, and imagery/font rights before sharing beyond authorized private recipients. [Third-party notices](../THIRD_PARTY_NOTICES.md) describe the current boundary; [FFmpeg's licensing guidance](https://ffmpeg.org/legal.html) explains its GPL build.

## Verify before loading

Download the final artifact from its exact completed workflow run. Use the expected repository, source commit, and signer workflow, rather than accepting any valid signature.

```sh
gh run download RUN_ID --repo OWNER/REPOSITORY \
  --name release-attested-COMMIT-ATTEMPT --dir verified-release
cd verified-release
sha256sum --check SHA256SUMS
gh attestation verify application.oci.tar --repo OWNER/REPOSITORY \
  --signer-workflow OWNER/REPOSITORY/.github/workflows/release.yml \
  --source-digest COMMIT --bundle provenance.sigstore.json
gh attestation verify scanner.oci.tar --repo OWNER/REPOSITORY \
  --signer-workflow OWNER/REPOSITORY/.github/workflows/release.yml \
  --source-digest COMMIT --bundle provenance.sigstore.json
gh attestation verify application.oci.tar --repo OWNER/REPOSITORY \
  --predicate-type https://spdx.dev/Document/v2.3 \
  --bundle application-sbom.sigstore.json
```

Verify every source archive and evidence file against the provenance bundle as well. The same bundle covers all subjects in SHA256SUMS. Confirm release.json contains the intended HTTPS origin and image configuration digests. The checksums file itself is not a substitute for signature verification. [GitHub CLI verification](https://cli.github.com/manual/gh_attestation_verify) documents identity, predicate, source-digest, and offline-bundle policies.

An attestation establishes the workflow identity and its signed claim. It does not establish the correctness of arbitrary build recipes or constitute production approval. Keep required checks, branch protection, workflow review, dependency controls, and release access restrictions in place.

## Optional private ECR publication

Publication defaults to false. Only select it after configuring the protected release environment and the narrowly scoped [GitHub publication role](../deploy/aws/github-release-role.json). Supply an existing application repository and a distinct scanner repository in the same AWS account/region plus the exact role ARN. Set the role template's GitHubEnvironment parameter to release. The trust policy must constrain the actual GitHub owner/repository and protected environment; no account identifiers are guessed.

Both repositories must have immutable tags. The job verifies the issued provenance before assuming the role, imports the OCI archives, confirms configuration digests, then pushes commit/run-specific tags and records the resolved registry digest references. It grants no infrastructure, database, provider, secret-read, or runtime deployment permission. The output digest references belong in reviewed deployment configuration; publication does not update running services.

The workflow does not create public GitHub releases, push to public registries, infer an AWS identity, send external mail, activate payment providers, or assert operator policy readiness. Those remain explicit account and product release settings.
