# CRAP scores for the factory code

CRAP (change risk anti-patterns) scores a function by its cyclomatic
complexity (CC) and how much of it the tests run:

```text
CRAP = CC² × (1 − coverage)³ + CC
```

An untested function with CC 12 scores 156; covered fully it scores 12. A
score never drops below CC, so a function whose CC is above a threshold passes
only by being split.

This page records the first measurement of the factory code: what it scored,
and what measuring it costs. Later checks set their thresholds and timeouts
from these numbers.

## What is measured

| Code | Coverage | Scorer | Baseline |
| --- | --- | --- | --- |
| `crates/factory_records` | `cargo llvm-cov nextest -p factory_records` | `cargo crap` | [`factory_records.json`](../../tooling/factory-crap/baseline/factory_records.json) |
| `crates/factory_engine` | `cargo llvm-cov nextest -p factory_engine` | `cargo crap` | [`factory_engine.json`](../../tooling/factory-crap/baseline/factory_engine.json) |
| `services/factory_service` | `cargo llvm-cov nextest -p factory_service` | `cargo crap` | [`factory_service.json`](../../tooling/factory-crap/baseline/factory_service.json) |
| `apps/web/src/features/factory-runs` | vitest, v8 provider, Istanbul JSON | `crap4ts` 1.0.1 | [`factory-runs.json`](../../tooling/factory-crap/baseline/factory-runs.json) |

[`tooling/scripts/factory-crap.sh`](../../tooling/scripts/factory-crap.sh)
writes the baselines: `rust [package ...]` for the crates, `web` for the
feature. Each crate is scored against its own tests' coverage, as a
per-package check would see it, so code that only another crate's tests reach
counts as uncovered. Line coverage is used in both languages.

Not scored:

- Rust test files (`test.rs`, `tests.rs`, `test/**`). cargo-crap skips
  `#[cfg(test)]` items but cannot see the attribute on the parent's
  `mod test;` line, so the script excludes the files.
- The `#[ignore]`d `factory_engine` tests (44, the ones that need Temporal);
  code that only they reach counts as uncovered.
- Web test files and `factory-runs/browser-test/`, the browser regression
  harness.

## Needs

- `cargo install --locked cargo-llvm-cov cargo-nextest cargo-crap` and
  `rustup component add llvm-tools-preview`.
- Postgres migrated at `DATABASE_URL` with `SQLX_OFFLINE` unset, as for
  `cargo test -p factory_records`.
- `@vitest/coverage-v8` at the installed vitest's version. It is not a
  dependency of the repository yet; the TypeScript check adds it.
  `crap4ts` runs through `bunx` at a pinned version.

## Baseline (base `bb371ab93`)

| Code | Functions | Median | p90 | p95 | p99 | Max | > 16 | > 30 | 0% covered |
| --- | --: | --: | --: | --: | --: | --: | --: | --: | --: |
| factory_records | 285 | 2 | 8 | 11.1 | 23.2 | 28.6 | 5 | 0 | 26 |
| factory_engine | 1269 | 2 | 10 | 14.8 | 34 | 182 | 52 | 14 | 159 |
| factory_service | see below | | | | | | | | |
| factory-runs (TS), as crap4ts scores it | 245 | 2 | 6 | 9.1 | 20 | 56 | 5 | 1 | 51 |
| factory-runs (TS), corrected | 245 | 2 | 4.1 | 6 | 12 | 18.4 | 1 | 0 | 32 |

crap4ts 1.0.1 finds no coverage for a function whose signature wraps onto
several lines: Istanbul places such a function at its body's first line, and
crap4ts looks for it at the declaration. It scores those 19 functions as
untested (the baseline's `unmatched` list), though the tests cover 18 of them
fully and `mergedCost` to 80%. The corrected row scores them from their own
lines' coverage.
`terminalState` (CC 7) is one: 56 as crap4ts scores it, 7 corrected. The
worst TypeScript function is then `toRun` in `queries/runs.ts` (CC 18, 89%,
18.4).

Two kinds of Rust function sit above 30:

- Untested ones: `FactoryServiceImpl::reply_to_gate` (CC 13, 0%, 182),
  `MainWatchWorkflow::run` (CC 9, 0%, 90), and the GitHub and Temporal
  adapters. Tests bring these down.
- Long, well-tested functions whose CC is the score: `start_run` (CC 56,
  100%), `merge_in_queue` (38), `walk_step` (39, 90%), `walk_round`,
  `verify_round`, `update_comment`. Only splitting them lowers the score.

## Cost

Measured on an 11-core Apple M3 with 18 GB of RAM, `CARGO_BUILD_JOBS=2`,
`nice -n 15`, the packages one after another into one coverage target. The
first package pays for the instrumented build of the dependencies it shares
with the others. Two runs each; the second ran through the script.

| Step | Wall | CPU (user + sys) | Peak RSS | Coverage target after |
| --- | --: | --: | --: | --: |
| `factory_records`, cold (build 2 m 48 s – 3 m 01 s, 189 tests 96–102 s) | 269–288 s | 370–396 s | 1.4 GB | 3.0 GB |
| `factory_engine`, after records (build 3 m 30 s, 992 tests 4 s) | 233–234 s | 440–445 s | 4.3 GB | 8.8 GB |
| `factory_service`, after engine (build 39 s, 1 test) | 54 s | 87 s | 1.6 GB | 8.6 GB |
| `cargo crap`, per package | < 1 s | < 1 s | 60 MB | – |
| factory-runs vitest with coverage (25 files, 181 tests, 4 workers) | 18–21 s | 91–112 s | 1.4 GB | – |
| `crap4ts` | 0.5 s (+1.5 s first `bunx` fetch) | 1 s | 230 MB | – |
| One-time install: llvm-cov, nextest, crap | 42 s, 210 s, 52 s | 6.5 min | 1.1 GB | 1.1 GB, removed |

All three crates from cold: about 9.5 minutes wall and 15 CPU-minutes, with
8.8 GB of coverage target beside the normal target. Instrumented artifacts
cannot share the normal target: building them there would replace the
normal artifacts. The web side costs about 20 seconds.

The whole workspace was not measured. `factory_service` reaches 828 of the
lockfile's 1747 packages and 3 of 377 workspace members, about 55 thousand
of 2.5 million lines of non-test Rust. Building and testing every member
instrumented would cost several CPU-hours and tens of GB of target on this
machine, so the checks should score changed packages only.

## Recommended thresholds

- Fail a change when a function it adds or changes scores above 30, or when a
  function's score rises against the committed baseline
  (`cargo crap --baseline … --fail-regression`). 30 is cargo-crap's default
  and crap4ts's lenient level; on this baseline it flags 14 of 1554 Rust
  functions (0.9%) and no TypeScript one once crap4ts's missed coverage is
  corrected. The TypeScript check has to correct it, or it fails on covered
  functions.
- Leave the functions already above 30 to the baseline; don't fail every
  change on them.
- Show reviewers the changed functions above 16 (crap4ts's default; 57 Rust
  functions and 1 TypeScript function today) as rows to look at, not failures.
- Time out the Rust check at 30 minutes cold at two build jobs (about three
  times the measured 9.5), and the TypeScript check at 5 minutes. Leave at
  least 12 GB of free disk for the coverage target.
