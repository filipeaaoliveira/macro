#!/usr/bin/env bash
# Refuse a recipe that deletes the default local stack's data when that stack
# is persistent (see "Run a Persistent Stack" in
# docs/development/running-locally.md), unless MACRO_LOCAL_WIPE_PERSISTENT_DATA=1.
#
# Usage: refuse-persistent-wipe.sh <what the recipe deletes> [DATABASE_URL]
# With a DATABASE_URL, only a URL on the default stack's Postgres
# (localhost:5432) is refused.
set -euo pipefail

what="$1"
database_url="${2:-}"

if [ "${MACRO_LOCAL_WIPE_PERSISTENT_DATA:-}" = 1 ]; then
  exit 0
fi

if [ -n "$database_url" ]; then
  case "$database_url" in
    *@localhost:5432/* | *@127.0.0.1:5432/*) ;;
    *) exit 0 ;;
  esac
fi

persistent=false
IFS=',' read -ra names <<<"${MACRO_LOCAL_PERSISTENT:-}"
for name in "${names[@]}"; do
  if [ "$(echo "$name" | tr -d '[:space:]')" = macro ]; then
    persistent=true
  fi
done
if docker volume inspect macro_localstack_data >/dev/null 2>&1; then
  persistent=true
fi

if [ "$persistent" = true ]; then
  echo "refusing to $what: the default local stack is persistent (MACRO_LOCAL_PERSISTENT names macro, or the volume macro_localstack_data exists), so its data is kept. To delete it on purpose, back it up and rerun with MACRO_LOCAL_WIPE_PERSISTENT_DATA=1." >&2
  exit 1
fi
