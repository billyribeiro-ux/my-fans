#!/usr/bin/env bash
set -euo pipefail
# sqlx::test creates a fresh migrated database for every test and removes it afterward.
export DATABASE_URL="${TEST_DATABASE_URL:-postgresql://myfans:local-development-only@127.0.0.1:54328/myfans}"
cargo test --locked -p myfans-api -- --ignored
