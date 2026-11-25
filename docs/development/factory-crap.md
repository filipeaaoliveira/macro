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
counts as uncovered. Both sides use line coverage, counted differently:
cargo-crap takes the lcov lines inside each function, while crap4ts's "line"
coverage counts the Istanbul statements inside each function's span.

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
  `cargo test -p factory_records`. `just setup_macrodb` creates and migrates
  it (see [Database development](database.md)).
- `@vitest/coverage-v8` at the installed vitest's version. It is not a
  dependency of the repository yet; the TypeScript check adds it.
  `crap4ts` runs through `bunx` at a pinned version.

## Baseline (base `bb371ab93`)

| Code | Functions | Median | p90 | p95 | p99 | Max | > 16 | > 30 | 0% covered |
| --- | --: | --: | --: | --: | --: | --: | --: | --: | --: |
| factory_records | 285 | 2 | 8 | 11.1 | 23.2 | 28.6 | 5 | 0 | 26 |
| factory_engine | 1269 | 2 | 10 | 14.8 | 34 | 182 | 52 | 14 | 159 |
| factory_service | 10 | 2 | – | – | – | 420 | 4 | 2 | 10 |
| factory-runs (TS), as crap4ts scores it | 245 | 2 | 6 | 9.1 | 20 | 56 | 5 | 1 | 51 |
| factory-runs (TS), corrected | 245 | 2 | 4.1 | 6 | 12 | 18.4 | 1 | 0 | 32 |

crap4ts 1.0.1 finds no coverage for a function whose signature wraps onto
several lines: Istanbul places such a function at its body's first line, and
crap4ts looks for it at the declaration. It scores those 19 functions as
untested (the baseline's `unmatched` list), though the tests cover 17 of them
fully, `reviewsAfresh` to 88.89% and `mergedCost` to 80%. The corrected row
scores them from their own lines' coverage.
The committed `factory-runs.json` holds crap4ts's uncorrected scores, so a
check must apply the same correction to both the baseline and the new run
and compare corrected against corrected.
`terminalState` (CC 7) is one: 56 as crap4ts scores it, 7 corrected. The
worst TypeScript function is then `toRun` in `queries/runs.ts` (CC 18, 89%,
18.4).

`factory_service` is the composition root: its `main`s, `run` (CC 20, 420)
and `serve` (CC 8, 72) wire the adapters and start the servers, and its one
test reaches none of them.

Two kinds of Rust function sit above 30 in the crates:

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
with the others. `factory_records` and `factory_engine` ran twice, the second
time through the script; `factory_service` once.

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

The whole workspace was not measured. Through its normal dependencies,
`factory_service` reaches 828 of the lockfile's 1747 packages, 96 of them
among the 377 workspace members; the factory crates
hold about 55 thousand lines of non-test Rust, against 2.5 million lines of
Rust (tests included) in all members' sources. Building and testing every member
instrumented would cost several CPU-hours and tens of GB of target on this
machine, so the checks should score changed packages only.

## Recommended thresholds

- Fail a change when a function it adds or changes scores above 30, or when a
  function's score rises against the committed baseline
  (`cargo crap --baseline … --fail-regression`). 30 is cargo-crap's default
  and crap4ts's lenient level; on this baseline it flags 14 of the 1554
  functions in the two factory crates (0.9%) and no TypeScript one once crap4ts's missed coverage is
  corrected. The TypeScript check has to correct it, or it fails on covered
  functions.
- Leave the functions already above 30 to the baseline; don't fail every
  change on them.
- Leave out service binaries' startup code (`services/*/src/main.rs` and the
  other `main`s): it wires adapters and is exercised by running the service,
  not by unit tests.
- Show reviewers the changed functions above 16 (crap4ts's default; 57 Rust
  functions and 1 TypeScript function today) as rows to look at, not failures.
- Time out the Rust check at 30 minutes cold at two build jobs (about three
  times the measured 9.5), and the TypeScript check at 5 minutes. Leave at
  least 12 GB of free disk for the coverage target.

## The `crap-rust` check

`tooling/xtask/crates/xtask_crap` scores the Rust functions a change
touched. In this repository it runs by hand as `cargo x crap-rust`; a product
repository runs the installed `xtask_crap` binary as a check in its Runs'
checks round. This repository declares no such check: Runs do not develop
it, and Benchmarks based on `feat/factory` would pay for the check without
using it.

### What it does

1. Base: `--base`, else `MACRO_CHECK_BASE`. When neither names a commit the
   repository holds (the agent runs the checks, GitHub's comparison failed,
   or the base was not fetched), it prints
   `crap-rust: skipped: no base commit to compare with` and exits 0: a
   comparison that fails never fails the round.
2. Tools: `cargo llvm-cov`, `cargo nextest` and `cargo crap` must answer
   `--version`; otherwise it exits 2 at once with the install command.
3. Changed files: those `MACRO_CHECK_CHANGED` lists when its first line is
   `complete`, else every `*.rs` path `git diff <base> HEAD` names. Tests,
   generated code, migrations, the SQLx cache, build scripts and
   `services/*/src/main.rs` are left out.
4. Coverage: the packages holding the files run their tests under
   `cargo llvm-cov nextest --ignore-run-fail`. Each test is ended after
   three minutes. The build goes to `CARGO_LLVM_COV_TARGET_DIR`, else
   cargo-llvm-cov's folder in `CARGO_TARGET_DIR`, else
   `${XDG_CACHE_HOME:-$HOME/.cache}/macro-check/crap-target`: outside the
   round's worktree, so only the first round on a machine builds cold.
   Rounds on one machine take turns on it.
5. Scores: `cargo crap -p <those>`, and the base versions of the changed
   and removed files for their complexity.
6. Pairing: a function pairs with the base function of its name whose line
   the diff carries to its own; one left over pairs with one of its name
   left over in its file, in order; one still left over pairs with a base
   function no function took, in any changed or removed file, whose body is
   the same token for token: it moved or was renamed.

### What it prints

One JSON line per changed file it could not score, one per function whose
span meets a diff hunk, then a summary line:

```json
{"path":"tauri/src/lib.rs","warning":"not scored: no package of the workspace holds it"}
{"path":"crates/a/src/lib.rs","name":"Thing::run","line":12,"complexity":7.0,"coverage":40.0,"score":17.9,"change":"rose","base_complexity":6.0,"fails":false}
crap-rust: 1 touched functions; 0 new or more complex above 30 (failing); 0 unproven; 0 others above it
```

- `change` is `new`, `rose`, `same` or `fell` against the base.
- A row `fails` when the function is `new` or `rose` and scores above
  `threshold` in `.cargo-crap.toml` (30 without one); the check then exits 1.
  Other touched functions above 30 are listed without failing.
- When tests failed, the summary names how many and the first three, every
  row has `"coverage_uncertain":true`, and a row that would fail only
  because it is 0% covered has `"unproven":true` and does not fail. A
  function whose complexity alone is above 30 still fails.
- Failing rows come last, the worst last, since the round keeps only the
  last 4 KiB of a check's output.
- Exit code: 0 passed or skipped, 1 a row fails, 2 the check could not run.

A file of another Cargo workspace in the repository, such as
`apps/web/tauri`, gets a warning line: the check runs the root workspace's
packages only.

### Declare it in a product repository

Install the tools on each machine that runs the checks:

```sh
cargo install --locked cargo-llvm-cov cargo-nextest cargo-crap
rustup component add llvm-tools-preview
cargo install --locked --git https://github.com/filipeaaoliveira/macro --branch feat/factory xtask_crap
```

Then add the check to the repository's `.macro/checks.json`, with `paths`
naming its Rust folders:

```json
{
  "checks": [
    {
      "name": "crap-rust",
      "run": "xtask_crap",
      "paths": ["crates", "src", ".cargo-crap.toml"],
      "timeout": 1200
    }
  ]
}
```

It runs from the root of a repository whose Cargo workspace is at its root.
An optional `.cargo-crap.toml` there sets `threshold` and `try-weight`.

### Try-weight

This repository's `.cargo-crap.toml` sets `try-weight = 0.0`: a `?` adds
nothing to a function's complexity. Scored again at weight 0 with the
coverage above, the three factory crates have 6 functions above 30 instead
of 16, and 34 above 16 instead of 61. At weight 1 any untested function
with five `?` fails, though its branches are errors returned to the caller.
The baselines above were scored at weight 1; `factory-crap.sh` now writes
weight-0 scores.

### Run it by hand

It scores the committed `HEAD`, so commit first:

```sh
cargo x crap-rust --base "$(git merge-base origin/feat/factory HEAD)"
```

### Time

On the M3 at two build jobs, lock waits left out: a change to
`subsequence_match` alone took 5 s with its coverage build warm, and this
check's own crate under 2 minutes cold. A cold first round pays the
instrumented build of the touched packages: 4 to 5 minutes for
`factory_engine` or `factory_records` (see [Cost](#cost)), about 9.5 for all
three factory crates, inside the 1200-second timeout above, the most a check
may take. Later rounds reuse the build and pay only for what changed and the
tests. A first round that runs out of time leaves its build behind, so the
next round starts warmer.
