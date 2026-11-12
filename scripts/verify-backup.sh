#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/.." && pwd)"
task_backup="${1:?Pass a directory created by backup-local.sh}"
task_backup="$(cd "$task_backup" && pwd)"
cd "$task_root"
(cd "$task_backup" && shasum -a 256 --check SHA256SUMS)
case "$(cat "$task_backup/MODE")" in host) task_service=postgres ;; bundle) task_service=bundle-postgres ;; *) echo 'Invalid backup mode.' >&2; exit 1 ;; esac
umask 077
task_temp="$(mktemp -d "${TMPDIR:-/tmp}/myfans-restore.XXXXXX")"
task_database="myfans_restore_$(date -u +%Y%m%d%H%M%S)_$RANDOM"
task_created=false
cleanup(){
  if [[ "$task_created" == true ]]; then docker compose --profile bundle exec -T "$task_service" dropdb -U myfans --if-exists "$task_database" >/dev/null; fi
  case "$task_temp" in */myfans-restore.*) rm -rf -- "$task_temp" ;; esac
}
trap cleanup EXIT
# Accept only directories and regular files inside media; reject links and traversal.
tar -tzf "$task_backup/media.tar.gz" > "$task_temp/names"
while IFS= read -r task_name; do
  case "$task_name" in media|media/|media/*) ;; *) echo 'Invalid media archive path.' >&2; exit 1 ;; esac
  case "/$task_name/" in */../*|*/./*) echo 'Invalid media archive path.' >&2; exit 1 ;; esac
done < "$task_temp/names"
if tar -tvzf "$task_backup/media.tar.gz" | awk 'substr($0,1,1)!="d" && substr($0,1,1)!="-" {bad=1} END {exit !bad}'; then
  echo 'Media archive contains a nonregular entry.' >&2; exit 1
fi
tar -xzf "$task_backup/media.tar.gz" -C "$task_temp"
docker compose --profile bundle exec -T "$task_service" createdb -U myfans "$task_database"
task_created=true
docker compose --profile bundle exec -T "$task_service" pg_restore -U myfans -d "$task_database" --exit-on-error < "$task_backup/database.dump"
docker compose --profile bundle exec -T "$task_service" psql -U myfans -d "$task_database" -v ON_ERROR_STOP=1 -c "DO \$\$ BEGIN IF EXISTS(SELECT 1 FROM ledger_transactions t LEFT JOIN ledger_postings p ON p.transaction_id=t.id WHERE t.status='posted' GROUP BY t.id HAVING COALESCE(sum(p.amount_minor),0)<>0 OR count(p.id)<2) THEN RAISE EXCEPTION 'Unbalanced restored ledger'; END IF; IF NOT EXISTS(SELECT 1 FROM app_users) THEN RAISE EXCEPTION 'No restored accounts'; END IF; END \$\$;"
docker compose --profile bundle exec -T "$task_service" psql -U myfans -d "$task_database" -At -v ON_ERROR_STOP=1 -c "SELECT storage_key FROM media_assets WHERE state IN ('processing','quarantine','approved') UNION SELECT preview_key FROM media_assets WHERE state IN ('quarantine','approved') AND preview_key IS NOT NULL" > "$task_temp/keys"
while IFS= read -r task_key; do
  case "$task_key" in ''|*/*|*..*) echo 'Invalid restored storage key.' >&2; exit 1 ;; esac
  if [[ ! -f "$task_temp/media/$task_key" ]]; then echo 'A restored media asset is missing.' >&2; exit 1; fi
done < "$task_temp/keys"
if [[ ! -f "$task_temp/media/.mfa-key" || "$(wc -c < "$task_temp/media/.mfa-key" | tr -d ' ')" != 32 ]]; then
  echo 'The 32-byte MFA encryption key is missing from the backup.' >&2; exit 1
fi
# Run the matching release as an unprivileged verifier; no listeners or workers start.
# Numeric archive ownership differs between host and bundle keys.
task_key_owner="$(ls -ln "$task_temp/media/.mfa-key" | awk '{print $3 ":" $4}')"
docker compose --profile bundle run --rm --no-deps -T --user "$task_key_owner" \
  --entrypoint myfans-api \
  -e "DATABASE_URL=postgresql://myfans:local-development-only@$task_service:5432/$task_database" \
  -e APP_RUN_MIGRATIONS=false -e MAIL_TRANSPORT=sandbox -e MEDIA_ROOT=/verify/media \
  -v "$task_temp/media:/verify/media:ro" app verify-storage
printf 'Database, ledger, media references, and authenticated stored ciphertext verified in isolated restore %s.\n' "$task_database"
