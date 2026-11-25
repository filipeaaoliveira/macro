# xtask_crap

## What it is

The `crap-rust` check: `cargo x crap-rust` here, the installed `xtask_crap`
binary in a product repository's `.macro/checks.json`. It scores the Rust
functions a change touched with `cargo crap` and fails when one is new or
more complex than at the base and scores above the threshold in
`.cargo-crap.toml` (30 without one). This repository does not run it as a
check; `docs/development/factory-crap.md` says why and how a product
repository declares it.

## Start here

- `src/main.rs`: the steps, in order: base (skipped without one), tools,
  changed files, coverage, scores, base scores, rows.
- `src/rows.rs`: pairing with the base, `change`, and when a row `fails`
  or is `unproven`.
- `src/diff.rs`: hunks, and where a base line lands in the new file.

## Checks

- `cargo test -p xtask_crap`
- A real run needs `cargo-llvm-cov`, `cargo-nextest` and `cargo-crap`.

## Gotchas

- A missing base exits 0: a Run's failed comparison never fails its round.
- The checks round keeps only the end of a check's output (4 KiB), so rows
  print on stdout after every build log, failing and worst last.
- It diffs the committed `HEAD` but reads the working tree: commit before
  running it by hand.
- Functions named `main` get no row: they wire a binary up.

## Related

- Docs: `docs/development/factory-crap.md`
