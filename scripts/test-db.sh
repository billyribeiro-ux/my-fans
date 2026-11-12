#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -n "${TEST_DATABASE_URL:-}" ]]; then
  for task_migration in "$task_root"/db/migrations/*.sql; do
    psql "$TEST_DATABASE_URL" -v ON_ERROR_STOP=1 -f "$task_migration"
  done
  psql "$TEST_DATABASE_URL" -v ON_ERROR_STOP=1 -f "$task_root/db/tests/invariants.sql"
  exit 0
fi
for task_command in initdb pg_ctl psql; do
  command -v "$task_command" >/dev/null || { echo "Missing $task_command; use TEST_DATABASE_URL pointing to an empty disposable PostgreSQL database." >&2; exit 1; }
done
task_dir="$(mktemp -d "${TMPDIR:-/tmp}/myfans-db.XXXXXX")"
cleanup() {
  pg_ctl -D "$task_dir/data" -m immediate stop >/dev/null 2>&1 || true
  case "$task_dir" in */myfans-db.*) rm -rf -- "$task_dir" ;; esac
}
trap cleanup EXIT
initdb -D "$task_dir/data" -U postgres -A trust --no-locale --encoding=UTF8 >/dev/null
pg_ctl -D "$task_dir/data" -l "$task_dir/postgres.log" -o "-F -c listen_addresses='' -k $task_dir -p 54339" -w start >/dev/null
for task_migration in "$task_root"/db/migrations/*.sql; do
  psql -h "$task_dir" -p 54339 -U postgres -d postgres -v ON_ERROR_STOP=1 -f "$task_migration"
done
psql -h "$task_dir" -p 54339 -U postgres -d postgres -v ON_ERROR_STOP=1 -f "$task_root/db/tests/invariants.sql"
