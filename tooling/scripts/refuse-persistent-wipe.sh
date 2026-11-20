#!/usr/bin/env bash
# Refuse a recipe that deletes the default local stack's data when that stack
# is persistent (see "Run a Persistent Stack" in
# docs/development/running-locally.md), unless MACRO_LOCAL_WIPE_PERSISTENT_DATA=1.
# When Docker can't say whether the stack is persistent, it refuses too.
#
# Usage: refuse-persistent-wipe.sh <what the recipe deletes> [DATABASE_URL]
# With a DATABASE_URL, only a URL on this machine's default Postgres port
# (5432) is refused.
#
# Runs under macOS's /bin/bash 3.2: no arrays, no ${var,,}.
set -euo pipefail

what="$1"
database_url="${2:-}"

if [ "${MACRO_LOCAL_WIPE_PERSISTENT_DATA:-}" = 1 ]; then
  exit 0
fi

refuse() {
  echo "refusing to $what: $1 To delete it on purpose, back it up and rerun with MACRO_LOCAL_WIPE_PERSISTENT_DATA=1." >&2
  exit 1
}

# Whether the URL's host is this machine and its port is 5432 (or omitted,
# which means 5432).
on_local_postgres() {
  local rest host_port host port
  rest="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  rest="${rest#*://}"   # scheme
  rest="${rest%%/*}"    # path
  rest="${rest%%\?*}"   # query, for a URL without a path
  rest="${rest##*@}"    # userinfo
  host_port="$rest"
  case "$host_port" in
    \[*\]:*) host="${host_port%%]:*}]"; port="${host_port##*]:}" ;;
    \[*\]) host="$host_port"; port=5432 ;;
    *:*) host="${host_port%%:*}"; port="${host_port##*:}" ;;
    *) host="$host_port"; port=5432 ;;
  esac
  case "$host" in
    localhost | 127.0.0.1 | '[::1]' | 0.0.0.0 | host.docker.internal) ;;
    *) return 1 ;;
  esac
  [ "${port:-5432}" = 5432 ]
}

if [ -n "$database_url" ] && ! on_local_postgres "$database_url"; then
  exit 0
fi

case ",$(printf '%s' "${MACRO_LOCAL_PERSISTENT:-}" | tr -d '[:space:]')," in
  *,macro,*) refuse "the default local stack is persistent (MACRO_LOCAL_PERSISTENT names macro), so its data is kept." ;;
esac

if ! command -v docker >/dev/null 2>&1; then
  refuse "docker is not installed, so it can't tell whether the default local stack is persistent."
fi
if inspected="$(docker volume inspect macro_localstack_data 2>&1)"; then
  refuse "the default local stack is persistent (the volume macro_localstack_data exists), so its data is kept."
fi
case "$(printf '%s' "$inspected" | tr '[:upper:]' '[:lower:]')" in
  *"no such volume"*) ;;
  *) refuse "docker could not say whether the default local stack is persistent ($inspected)." ;;
esac
