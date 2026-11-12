#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$task_root"
task_destination="${1:-$task_root/.local/backups/$(date -u +%Y%m%dT%H%M%SZ)}"
task_mode="${2:-host}"
case "$task_mode" in
  host)
    task_service=postgres
    if curl --silent --fail --max-time 2 http://127.0.0.1:8080/health/ready >/dev/null; then
      echo 'Stop the development app and all workers before backing up its database and media.' >&2; exit 1
    fi
    ;;
  bundle)
    task_service=bundle-postgres
    if [[ -n "$(docker compose --profile bundle ps --status running -q app)" ]]; then
      echo 'Run docker compose --profile bundle stop app before taking a consistent bundle backup.' >&2; exit 1
    fi
    ;;
  *) echo 'Backup mode must be host or bundle.' >&2; exit 1 ;;
esac
task_storage=local:v1
task_has_storage="$(docker compose --profile bundle exec -T "$task_service" psql -U myfans -d myfans -At -v ON_ERROR_STOP=1 -c "SELECT to_regclass('public.platform_storage') IS NOT NULL")"
if [[ "$task_has_storage" == t ]]; then
  task_storage="$(docker compose --profile bundle exec -T "$task_service" psql -U myfans -d myfans -At -v ON_ERROR_STOP=1 -c "SELECT media_store_identity FROM platform_storage WHERE id=1")"
fi
if [[ "$task_storage" != local:v1 || -n "${MFA_ENCRYPTION_KEY_FILE:-}" ]]; then
  echo 'This backup workflow requires local media and its local key; S3 and externally mounted keys require coordinated versioned recovery.' >&2; exit 1
fi
umask 077
mkdir -p "$task_destination"
task_destination="$(cd "$task_destination" && pwd)"
if [[ -e "$task_destination/database.dump" || -e "$task_destination/media.tar.gz" ]]; then
  echo 'Choose a new backup directory; existing backup files will not be overwritten.' >&2; exit 1
fi
task_temp="$(mktemp -d "$task_destination/.media.XXXXXX")"
cleanup(){ case "$task_temp" in "$task_destination"/.media.*) rm -rf -- "$task_temp" ;; esac; }
trap cleanup EXIT
docker compose --profile bundle exec -T "$task_service" pg_dump -U myfans -d myfans --format=custom > "$task_destination/database.dump"
if [[ "$task_mode" == bundle ]]; then
  docker compose --profile bundle cp app:/app/.local/media "$task_temp/media"
  tar -czf "$task_destination/media.tar.gz" -C "$task_temp" media
else
  tar -czf "$task_destination/media.tar.gz" -C "$task_root/.local" media
fi
printf '%s\n' "$task_mode" > "$task_destination/MODE"
(cd "$task_destination" && shasum -a 256 database.dump media.tar.gz MODE > SHA256SUMS)
printf 'Sandbox backup created: %s (%s)\n' "$task_destination" "$task_mode"
