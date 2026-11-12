# Private staging deployment

This procedure deploys the complete packaged sandbox to a private, single-host Docker environment. It does not enable a public production platform. Payments and identity decisions remain simulated; SMTP remains in sandbox mode. See [production readiness](production-readiness.md) for production configuration and account acceptance. Public AWS deployment is documented in [AWS runtime](aws-runtime.md).

The staging configuration is separate from the development Compose file. It uses independently generated 256-bit database passwords, separate bootstrap/migration/runtime accounts, mounted connection secret files, immutable application image references, a read-only unprivileged application, bounded logs/memory/processes, and database/media volumes. PostgreSQL has no published port. The web listener binds only to `127.0.0.1`. App health checks probe both the Rust readiness endpoint and the packaged SvelteKit listener. Database readiness alone does not prove provider health.

## Build and start

Use a supported Docker Engine with Compose v2 and the pinned Node LTS for the deployment helper. Commands below run from the repository root. Never regenerate passwords over an existing initialized database volume.

```sh
docker build --build-arg APP_ORIGIN=http://127.0.0.1:3100 --tag myfans:staging .
docker image inspect myfans:staging --format '{{.Id}}'
# Use the full immutable sha256 ID printed above, or an already-pulled registry digest.
pnpm deploy:private prepare myfans-staging sha256:FULL_IMAGE_ID 3100
pnpm deploy:private up .local/deployments/myfans-staging
pnpm deploy:private status .local/deployments/myfans-staging
```

Preparation creates a new private directory, `.local/deployments/myfans-staging`, with `deployment.json` and separate secret files. It refuses to reuse an existing directory. Credentials are not printed or passed as command arguments. The bootstrap and migration credentials are never mounted into the application container. The runtime account receives the reviewed grants in `db/roles/runtime.sql`; it cannot own tables or manage schema/financial immutability triggers. Startup verifies the exact embedded migration versions and checksums with `APP_RUN_MIGRATIONS=false`.

Secret files are mode `0444` inside host directories with mode `0700`. This permits differing non-root container UIDs to read only their individually mounted files without granting other host users access through the directory. Retain private directory permissions and restrict Docker daemon access. Compose secret files are local mounts, not an encrypted cloud secret manager. The database connection on the isolated container network is not TLS. These are deliberate private staging boundaries, not production infrastructure assurances. Never copy or publish this directory with source or release artifacts.

The application starts without demo accounts. To install the documented public sandbox fixtures for testing, explicitly run:

```sh
pnpm deploy:private seed .local/deployments/myfans-staging
PLAYWRIGHT_BASE_URL=http://127.0.0.1:3100 pnpm test:e2e
```

For a private remote host, use an authenticated SSH tunnel and keep the same local port on both ends:

```sh
ssh -N -L 3100:127.0.0.1:3100 user@staging-host
```

Open `http://127.0.0.1:3100`. SvelteKit fixes `APP_ORIGIN` at build time; the image must be built for this exact browser address. The helper verifies the image's origin metadata against the configured port before stopping an existing deployment. A runtime origin variable cannot change this build. Keep the tunnel port identical. Do not change the listener to `0.0.0.0`, proxy this sandbox publicly, or attach real merchant credentials.

## Backup and restore rehearsal

The app runs all background workers in its container. Stopping it drains active work before taking a database/media snapshot. The backup command refuses a running app and creates a new private directory without replacing existing backups.

```sh
pnpm deploy:private stop .local/deployments/myfans-staging
pnpm deploy:private backup .local/deployments/myfans-staging
# Use the exact newly created backup directory printed by the command.
pnpm deploy:private verify-backup .local/deployments/myfans-staging BACKUP_DIRECTORY
pnpm deploy:private up .local/deployments/myfans-staging
```

Backups include a custom PostgreSQL dump, uploaded media, the encryption key, and a manifest with hashes and the image reference. They contain sensitive data and are not encrypted at rest by this script. Encrypt them before off-host transfer; production needs managed encrypted backup/PITR, retention, and restore acceptance. Protect the deployment credentials separately; they are not in the database dump or media archive.

Verification requires the matching release image. It rejects altered checksums, path traversal, and links before extraction. It restores into a fresh randomly named database, checks ledger balance and media references, and runs the matching Rust binary without HTTP listeners/workers to verify migration checksums and authenticate stored encrypted MFA/mail data. It removes only that isolated database and temporary extraction. It never replaces the live database or files. An empty new deployment can also be verified. A passing restore is evidence of recoverability for that snapshot, not a measured production RPO/RTO.

## Upgrade and recovery

1. Build and test the candidate image. Pull it onto the deployment host by immutable digest, or load the private package below.
2. Stop staging writers and make a backup. Run `node scripts/deploy-private.mjs verify-upgrade DEPLOYMENT_DIRECTORY BACKUP_DIRECTORY IMMUTABLE_CANDIDATE_IMAGE`. It verifies the snapshot with the current release, migrates only the isolated restored database with the candidate, reapplies runtime grants there, and checks encrypted storage using the restricted runtime login. It leaves the live deployment and its image configuration unchanged. A mutable candidate tag is rejected before a database is created.
3. Record the current image reference and update only `image` in the existing `deployment.json`. Keep credentials, name, and port unchanged.
4. Run `up`. It stops writers, waits for PostgreSQL, executes migrations with the owner, reapplies runtime grants, and starts the application only after those steps succeed.
5. Repeat packaged browser verification and check health and application behavior. Test any forward migrations on an isolated restored copy before upgrading shared staging.

If migration or grants fail, the helper leaves the application stopped and returns nonzero. Do not start an older binary against a changed schema: its startup checksum check will reject incompatible migration histories. Preserve the failed database and logs, investigate, and use a tested forward repair. For a destructive failure, restore the verified pre-upgrade snapshot into a separate new deployment and use its matching image; validate it before changing access. There is intentionally no automatic destructive rollback or volume deletion command. This private Compose helper does not implement schema rollback or production traffic cutover. The separate [AWS recovery workflow](aws-runtime.md#promote-a-verified-restore) supports verified restore promotion followed by a reviewed runtime change set and service activation.

Successful startup adds a local `myfans-retained:IMAGE_ID` retention tag so Docker's containerd store keeps the image when a build tag is replaced. The deployment still executes its immutable reference. Retain that image or its exported archive for every retained backup; image pruning can still remove it. Backups reject a configuration changed to another release before the current container is backed up, and restore preflight rejects an unavailable matching image before creating a database.

## Offline private package

```sh
pnpm package:private sha256:FULL_IMAGE_ID .local/releases/staging-20261007
```

Use a new destination whose parent exists. The package contains `image.tar`, an allowlisted source snapshot and archive, `runtime.spdx.json`, `runtime-vulnerabilities.json`, and `manifest.json` identifying the immutable image, OS/architecture, origin, artifact hashes, source hashes, and scanner versions. It excludes uploads, databases, keys, environment files, personal tooling settings, generated outputs, backups, and browser traces. It is a private staging transfer artifact, not signed CI provenance, a production attestation, or completed license acceptance. It records the current source snapshot; independently establish that the image was built from those bytes before distributing a release.

The Alpine provider candidate passes the runtime image gate. Historical Debian findings remain in the [container record](container-findings.md); they were not suppressed. Only a directory containing a successful manifest after all gates pass is a package eligible for the verification procedure below.

The helper runs Docker Scout 1.25.0 from a pinned image digest with networking disabled to inventory the exported image. It then runs pinned Grype 0.120.1 against that SPDX inventory, downloading Anchore's public vulnerability database without uploading the application image. The database cache is private under `.local/image-scan-cache`; reserve several GB of disk for it. Scanner containers run as the host user with a read-only root and no Linux capabilities. Database/scanner failures or high/critical matches prevent creation of a successful manifest. Reports are retained for inspection when a vulnerability gate fails. Runtime inventory detection of stripped Rust binaries and bundled JavaScript is incomplete, so `cargo audit` and `pnpm audit` on the exact source lockfiles remain mandatory. Evaluate all report findings; lower-severity findings are recorded rather than silently removed.

Verify archive hashes against the manifest through a trusted channel, then use `docker image load --input image.tar` on a host supporting the recorded architecture. Extract `source.tar.gz`, install the pinned pnpm toolchain there, and prepare a new deployment with the recorded immutable image ID. Secrets are generated on that host. Do not publish the private package until distribution terms and all dependency/base-image/codec licenses are accepted; see [third-party notices](../THIRD_PARTY_NOTICES.md).

## Continuous verification

The GitHub Actions container job runs after application, migration, dependency, and infrastructure checks. It builds the image, initializes an isolated private staging database, applies owner migrations and runtime grants, seeds only test fixtures, runs the full browser suite through the restricted runtime account, rehearses backup/restore and restart, and generates the scanned package. It stops application writers even on failure. Browser fixtures replace external Unsplash stock imagery. Fonts are locally served with included licenses, and browser tests assert no Google Fonts resource requests. Actual application APIs, private media, database accounts, authorization and font loading are exercised. Stock-image fixture delivery does not establish external image availability or production image rights. No repository remote or hosted workflow execution has been verified locally; a workflow definition is not a passing remote run.

Compose secret and dependency ordering behavior follow [Docker's secrets documentation](https://docs.docker.com/compose/how-tos/use-secrets/) and [startup ordering documentation](https://docs.docker.com/compose/how-tos/startup-order/). The workflow uses a Linux runner for Docker, as required by [GitHub's workflow reference](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax).

Image inventory follows [Docker Scout's SPDX reference](https://docs.docker.com/reference/cli/docker/scout/sbom/) and [offline indexing configuration](https://docs.docker.com/scout/how-tos/configure-cli/). Grype's database behavior and freshness validation follow [Anchore's vulnerability database documentation](https://oss.anchore.com/docs/guides/vulnerability/database/); its pinned version was [released October 6](https://github.com/anchore/grype/releases/tag/v0.120.1).
