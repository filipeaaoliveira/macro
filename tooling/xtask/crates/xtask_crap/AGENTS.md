# xtask_crap

## What it is

`cargo x crap-rust [--base <commit>]`: the `crap-rust` check in
`.macro/checks.json`. It scores the Rust functions a change touched with
`cargo crap` and fails when one is new or more complex than at the base and
scores above the threshold in `.cargo-crap.toml`.

## How it works

1. Changed paths: `MACRO_CHECK_CHANGED`'s list when its first line is
   `complete`, else every `*.rs` path `git diff <base> HEAD` names. The base
   is `--base`, else `MACRO_CHECK_BASE`; without one the check exits 2.
2. Keeps Rust source only (`changed::is_scored`): no tests, generated code,
   migrations, build scripts or `services/*/src/main.rs`.
3. Maps the files to their packages (`cargo metadata`), runs
   `cargo llvm-cov nextest -p <those> --lcov` and `cargo crap -p <those>`.
   A failing test does not stop it: the tests' own check reports that.
4. Scores the files' base versions (`git show`) for their complexity, and
   pairs functions by file (the earlier name of a renamed one) and name.
5. Keeps the functions whose span (`fn` keyword to closing brace) meets a
   diff hunk and prints one JSON line each, failing rows last, then a
   summary line. Exit code: 0 passed, 1 a function failed, 2 not run.

## Start here

- `src/rows.rs`: the row, its `change` (`new`, `rose`, `same`, `fell`) and
  when it `fails`.
- `src/main.rs`: the commands it runs, in order.

## Checks

- `cargo test -p xtask_crap`
- A real run needs `cargo-llvm-cov` and `cargo-crap` installed (see
  `docs/development/factory-crap.md`).

## Gotchas

- The checks round keeps only the end of a check's output (4 KiB), so rows
  print on stdout after every build log, failing and worst last.
- Functions named `main` get no row: they wire a binary up.
- It diffs and scores the committed `HEAD`, but reads spans from the working
  tree: commit before running it by hand.

## Related

- Docs: `docs/development/factory-crap.md`
