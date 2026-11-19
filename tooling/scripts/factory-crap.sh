#!/usr/bin/env bash
# CRAP scores (complexity x uncovered) for the factory code, written as the
# JSON baselines under tooling/factory-crap/baseline/. See
# docs/development/factory-crap.md for what it measures, what it costs and
# what it needs installed.
#
#   tooling/scripts/factory-crap.sh rust [package ...]  # default: the three factory crates
#   tooling/scripts/factory-crap.sh web
#
# Each Rust package is scored against the coverage of its own tests only, as a
# per-package check sees it. Coverage builds go to COV_TARGET_DIR (default
# target/llvm-cov), apart from the normal target, because instrumented builds
# would otherwise replace the normal artifacts. Run it from the repository
# root with Postgres migrated at DATABASE_URL, as for `cargo test`.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
OUT="${CRAP_OUT_DIR:-$ROOT/tooling/factory-crap/baseline}"
WORK="${CRAP_WORK_DIR:-$ROOT/target/factory-crap}"
COV_TARGET_DIR="${COV_TARGET_DIR:-$ROOT/target/llvm-cov}"
RUST_PACKAGES=(factory_records factory_engine factory_service)
# Test-only modules (`#[cfg(test)] mod test;`) live in these files. cargo-crap
# skips `#[cfg(test)]` items, but cannot see the attribute on the parent's
# `mod` line, so it would score the helpers in them as untested code.
RUST_TEST_FILES=(--exclude '**/test.rs' --exclude '**/tests.rs' --exclude '**/test/**')
WEB_SRC="src/features/factory-runs"
CRAP4TS="crap4ts@1.0.1"

mkdir -p "$OUT" "$WORK"

rust() {
  local packages=("$@")
  [ ${#packages[@]} -eq 0 ] && packages=("${RUST_PACKAGES[@]}")
  cd "$ROOT"
  for pkg in "${packages[@]}"; do
    # Profiles from an earlier package would count as this one's coverage.
    find "$COV_TARGET_DIR" -name '*.profraw' -delete 2>/dev/null || true
    # --no-clean keeps the instrumented build of the shared dependencies.
    CARGO_TARGET_DIR="$COV_TARGET_DIR" cargo llvm-cov nextest \
      --no-clean -p "$pkg" --lcov --output-path "$WORK/$pkg.lcov"
    CARGO_TARGET_DIR="$COV_TARGET_DIR" cargo crap -p "$pkg" \
      --lcov "$WORK/$pkg.lcov" "${RUST_TEST_FILES[@]}" \
      --format json --sort file --output "$WORK/$pkg.json"
    # The diagnostics name this machine's absolute paths; a baseline needs
    # only the entries.
    jq 'del(.diagnostics)' "$WORK/$pkg.json" >"$OUT/$pkg.json"
  done
}

web() {
  cd "$ROOT/apps/web"
  rm -rf "$WORK/web-coverage"
  bunx vitest run --project app "$WEB_SRC" \
    --coverage.enabled --coverage.provider=v8 --coverage.reporter=json \
    --coverage.include="$WEB_SRC/**/*.{ts,tsx}" \
    --coverage.exclude='**/*.test.{ts,tsx}' \
    --coverage.exclude="$WEB_SRC/browser-test/**" \
    --coverage.reportsDirectory="$WORK/web-coverage"
  # Exit code 1 only means a function is above crap4ts's default threshold.
  bunx "$CRAP4TS" --coverage "$WORK/web-coverage/coverage-final.json" \
    --src "$WEB_SRC" --exclude '**/*.test.*' "$WEB_SRC/browser-test/**" \
    --format json >"$WORK/factory-runs.json" || [ $? -eq 1 ]
  # Kept: the scores and the functions crap4ts found no coverage for, which it
  # scores as untested (see the doc). Dropped: the run's time, so unchanged
  # scores leave the file unchanged, and the coverage entries with no
  # function of their own (inner callbacks), which only make it large.
  jq 'del(.timestamp, .warnings) | .unmatched |= map(select(.kind == "no-coverage"))' \
    "$WORK/factory-runs.json" >"$OUT/factory-runs.json"
}

case "${1:-}" in
  rust) shift; rust "$@" ;;
  web) web ;;
  *) echo "usage: $0 rust [package ...] | web" >&2; exit 2 ;;
esac
